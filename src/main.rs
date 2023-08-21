use anyhow::Result;
use chrono::Utc;
use clap::Command;
use home::home_dir;
use k8s_openapi::api::core::v1::{Node, Pod};
use kube::{api::ListParams, Api, ResourceExt};
use logpv2::*;
use simplelog::{info, ConfigBuilder, LevelFilter, TermLogger, TerminalMode, __private::log::warn};
use std::{env::current_dir, fs, path, path::Path};
use time::macros::format_description;

fn read_config_file<P: AsRef<Path>>(path: P) -> Result<ConfigFile> {
    let content = std::fs::read_to_string(path)?;
    let config_file: ConfigFile = serde_json::from_str(&content)?;
    Ok(config_file)
}

fn folder_creation(c: ConfigFile) -> Result<Vec<String>> {
    let date = Utc::now().format("%Y%m%d%H%M%S");
    let folder_vec = vec!["pods", "infra", "helm", "apps"];
    let folder_vec = folder_vec
        .iter()
        .map(|f| {
            if !c.output_directory_path.is_empty() {
                let p = &c
                    .output_directory_path
                    .strip_suffix(path::is_separator)
                    .unwrap_or(&c.output_directory_path);
                format!("{}/info_{}_{}/{}", p, c.context_name, date, f)
            } else {
                let cd = current_dir().unwrap().display().to_string();
                format!("{}/info_{}_{}/{}", cd, c.context_name, date, f)
            }
        })
        .collect::<Vec<String>>();

    Ok(folder_vec)
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ConfigBuilder::new()
        .set_time_format_custom(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .build();
    TermLogger::init(
        LevelFilter::Info,
        config,
        TerminalMode::Stdout,
        simplelog::ColorChoice::Auto,
    )
    .unwrap();
    let kube_config_path = home_dir().unwrap().join(".kube/config").into_os_string();
    //Clap outin
    let m = Command::new("Gatther Debug Logs Tools.")
        .version("0.1.0")
        .author("tuxedo <wtuxedo@proton.me>")
        .about("Gather useful information for debugging issues raised by the support team.")
        .arg(
            clap::Arg::new("config")
                .short('c')
                .long("config")
                .value_name("CONFIG_FILE_PATH")
                .help("Config File Path")
                .required(true),
        )
        .arg(
            clap::Arg::new("kube_config_path")
                .short('k')
                .long("kube_config_path")
                .value_name("KUBE_CONFIG_PATH")
                .help("Kubernetes custom config file path.")
                .default_value(kube_config_path)
                .required(false),
        )
        .get_matches();
    //Pod
    let config_file_path = m.get_one::<String>("config").unwrap();

    let config_file = read_config_file(config_file_path)?;

    let kube_config_path = m.get_one::<String>("kube_config_path").unwrap();

    let client = kubernetes_client(kube_config_path, config_file.clone()).await?;

    let mut pods = vec![];
    config_file.context_namespace.iter().for_each(|cn| {
        let p: Api<Pod> = Api::namespaced(client.clone(), cn);
        pods.push(p);
    });

    std::process::Command::new("clear").status().unwrap();
    info!("<green>Starting Log collection...</>");
    info!(
        "The following kube config path will be use: {}",
        &kube_config_path
    );

    let folders = folder_creation(config_file.clone()).unwrap();

    folders.iter().for_each(|fo| match fs::create_dir_all(fo) {
        Ok(_) => info!("Directory has been created {}.", fo),
        Err(e) => {
            panic!("{}", e)
        }
    });
    info!("Context Name: {}.", &config_file.context_name);
    info!(
        "Context NameSpace: {}.",
        &config_file.context_namespace.join(", ")
    );
    if !config_file.strimzi_operator_namespace.is_empty() {
        info!(
            "strimzi_operator_namespace: {}",
            &config_file.strimzi_operator_namespace
        )
    }
    let mut cmdk = vec![];
    config_file.context_namespace.iter().for_each(|cn| {
        let mut cmd = std::process::Command::new("kubectl");
        cmd.args([
            "get",
            "pod",
            "-n",
            cn,
            "--context",
            &config_file.context_name,
            "-o",
            "wide",
        ]);
        let file_name = format!("kubernetes_pods_{}.list", cn);
        cmdk.push((cmd, file_name));
        let mut cmd = std::process::Command::new("kubectl");
        cmd.args([
            "get",
            "pod",
            "-n",
            cn,
            "--context",
            &config_file.context_name,
            "-o",
            "json",
        ]);
        let file_name = format!("kubernetes_pods_{}.json", cn);
        cmdk.push((cmd, file_name))
    });

    //Get list pods.

    let pods_list: Vec<(
        String,
        String,
        kube::Api<k8s_openapi::api::core::v1::Pod>,
        Vec<String>,
    )> = get_pod_list(pods).await?;

    pods_list.iter().for_each(|p| {
        let file_name = format!("{}_{}.description", p.1, p.0);
        let mut cmd = std::process::Command::new("kubectl");
        cmd.args([
            "describe",
            "pod",
            &p.0,
            "-n",
            &p.1,
            "--context",
            &config_file.context_name,
        ]);

        cmdk.push((cmd, file_name));
    });
    let mut fut_handle: Vec<tokio::task::JoinHandle<()>> = vec![];
    cmdk.into_iter().for_each(|mut c| {
        let folders = folders.clone();
        let task = tokio::task::spawn(async move {
            let o = c.0.output().expect("kubectl command failed to start");
            if !o.stdout.is_empty() {
                match write_file(&folders[0], &o.stdout, &c.1) {
                    Ok(_) => info!("File has been created {}/{}", &folders[0], &c.1),
                    Err(e) => panic!("{}", e),
                }
            }
            if !o.stderr.is_empty() {
                warn!("{}", String::from_utf8_lossy(&o.stderr))
            }
        });
        fut_handle.push(task);
    });

    if config_file.current_logs {
        pods_list.clone().into_iter().for_each(|pl| {
            let folders = folders.clone();
            let pname = pl.0.clone();
            let task = tokio::task::spawn(async move {
                let l = get_logs(pl.0, pl.3[0].to_string(), pl.2, false).await;
                match l {
                    Ok(l) => {
                        let filename = format!("logs_current_{}_{}.log", &pl.1, &pname);
                        match write_file(&folders[0], l.as_bytes(), &filename) {
                            Ok(_) => info!("File has been created {}/{}", &folders[0], filename),
                            Err(e) => {
                                warn!("{}", e)
                            }
                        }
                    }
                    Err(e) => {
                        warn!("{}", e)
                    }
                }
            });
            fut_handle.push(task);
        });
    }

    if config_file.previous_logs {
        pods_list.clone().into_iter().for_each(|pl| {
            let folders = folders.clone();
            let pname = pl.0.clone();
            let task = tokio::task::spawn(async move {
                let l = get_logs(pl.0, pl.3[0].to_string(), pl.2, true).await;
                match l {
                    Ok(l) => {
                        let filename = format!("logs_previous_{}_{}.log", &pl.1, &pname);
                        match write_file(&folders[0], l.as_bytes(), &filename) {
                            Ok(_) => info!("File has been created {}/{}", &folders[0], filename),
                            Err(e) => {
                                warn!("{}", e)
                            }
                        }
                    }
                    Err(e) => {
                        warn!("{}", e)
                    }
                }
            });
            fut_handle.push(task);
        });
    }

    for handle in fut_handle {
        match handle.await {
            Ok(_) => {}
            Err(e) => {
                warn!("{}", e)
            }
        }
    }

    // Infra

    let nodes: Api<Node> = Api::all(client.clone());

    let nodes_list = nodes.list(&ListParams::default()).await?;

    let nodes_list = nodes_list
        .items
        .iter()
        .map(|n| n.name_any())
        .collect::<Vec<String>>();

    let mut cmdki = vec![];
    let mut fut_handle_infra = vec![];
    let mut cmd = std::process::Command::new("kubectl");
    cmd.args([
        "get",
        "nodes",
        "--context",
        &config_file.context_name,
        "-o",
        "wide",
    ]);
    let file_name = "kubernetes_nodes.list".to_string();
    cmdki.push((cmd, file_name));

    let mut cmd = std::process::Command::new("kubectl");
    cmd.args([
        "get",
        "nodes",
        "--context",
        &config_file.context_name,
        "-o",
        "json",
    ]);
    let file_name = "kubernetes_nodes_list.json".to_string();
    cmdki.push((cmd, file_name));

    let mut cmd = std::process::Command::new("kubectl");
    cmd.args([
        "version",
        "--context",
        &config_file.context_name,
        "-o",
        "json",
    ]);
    let file_name = "kubernetes_version.json".to_string();
    cmdki.push((cmd, file_name));

    let mut cmd = std::process::Command::new("kubectl");
    cmd.args(["events", "-A", "--context", &config_file.context_name]);
    let file_name = "kubernetes_cluster.events".to_string();
    cmdki.push((cmd, file_name));

    nodes_list.iter().for_each(|n| {
        let mut cmd = std::process::Command::new("kubectl");
        cmd.args([
            "describe",
            "node",
            n,
            "--context",
            &config_file.context_name,
        ]);

        let file_name = format!("{}.description", n);
        cmdki.push((cmd, file_name));
    });

    cmdki.into_iter().for_each(|mut c| {
        let folders = folders.clone();
        let task = tokio::task::spawn(async move {
            let o = c.0.output().expect("kubectl command failed to start");
            if !o.stdout.is_empty() {
                match write_file(&folders[1], &o.stdout, &c.1) {
                    Ok(_) => info!("File has been created {}/{}", &folders[1], &c.1),
                    Err(e) => panic!("{}", e),
                }
            }
            if !o.stderr.is_empty() {
                warn!("{}", String::from_utf8_lossy(&o.stderr))
            }
        });
        fut_handle_infra.push(task);
    });

    for handle in fut_handle_infra {
        match handle.await {
            Ok(_) => {}
            Err(e) => {
                warn!("{}", e)
            }
        }
    }

    //helm

    info!("<yellow>LOG collection has been completed!!</>");
    Ok(())
}

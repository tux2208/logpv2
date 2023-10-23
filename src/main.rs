use anyhow::Result;
use chrono::Utc;
use clap::Command;
use home::home_dir;
use k8s_openapi::api::core::v1::{Node, Pod, Secret};
use kube::{api::ListParams, Api, ResourceExt};
use logpv2::*;
use serde_derive::Deserialize;
use serde_derive::Serialize;
use simplelog::{
    info, ColorChoice, CombinedLogger, ConfigBuilder, LevelFilter, TermLogger, TerminalMode,
    WriteLogger, __private::log::warn,
};
use std::{
    env::current_dir,
    fs::{self, File},
    path,
    path::Path,
};
use time::macros::format_description;

fn read_config_file<P: AsRef<Path>>(path: P) -> Result<ConfigFile> {
    let content = fs::read_to_string(path)?;
    let config_file: ConfigFile = serde_json::from_str(&content)?;
    Ok(config_file)
}

fn folder_creation(c: ConfigFile) -> Result<Vec<String>> {
    let date = Utc::now().format("%Y%m%d%H%M%S");
    let folder_vec = ["pods", "infra", "helm", "apps"];
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

pub type LsHelm = Vec<Helm>;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Helm {
    pub name: String,
    pub namespace: String,
    pub revision: String,
    pub updated: String,
    pub status: String,
    pub chart: String,
    #[serde(rename = "app_version")]
    pub app_version: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let config = ConfigBuilder::new()
        .set_time_format_custom(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .build();
    let now = chrono::offset::Local::now();
    let custom_datetime_format = now.format("%Y%m%y_%H%M%S");
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Info,
            config.clone(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Info,
            config.clone(),
            File::create(format!(
                "output_logpv2_gather_tool_{}.log",
                custom_datetime_format
            ))
            .unwrap(),
        ),
    ])
    .unwrap();
    let kube_config_path = home_dir().unwrap().join(".kube/config").into_os_string();
    //Clap outin
    let m = Command::new("Gather Debug Logs Tools.")
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

    let mut secret = vec![];
    config_file.context_namespace.iter().for_each(|cn| {
        let s: Api<Secret> = Api::namespaced(client.clone(), cn);
        secret.push(s);
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

    let pods_list: Vec<(String, String, Api<Pod>, Vec<String>)> =
        get_pod_list(pods.clone(), "".to_string(), "".to_string()).await?;

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
    //get helm version
    //list helm charts
    //get helm chart values.
    let mut cmdhelms = vec![];
    let mut fut_handle_helm = vec![];
    let context = config_file.context_name;
    let arg1 = format!("--kubeconfig={}", kube_config_path);
    let arg2 = format!("--kube-context={}", &context);
    let mut cmd = std::process::Command::new("helm");
    cmd.args([&arg1, &arg2, "version"]);
    let file_name = "helm_version.log".to_string();
    cmdhelms.push((cmd, file_name));

    config_file.context_namespace.iter().for_each(|n| {
        let mut cmd = std::process::Command::new("helm");
        cmd.args([&arg1, &arg2, "ls", "-n", n]);
        let file_name = format!("helm_list_{}.log", n);
        cmdhelms.push((cmd, file_name));
        let mut cmdt = std::process::Command::new("helm");
        cmdt.args([&arg1, &arg2, "ls", "-n", n, "-o", "json"]);
        let o = cmdt.output().unwrap();
        let o: LsHelm = serde_json::from_str(&String::from_utf8_lossy(&o.stdout)).unwrap();
        o.iter().for_each(|h| {
            let file_name = format!("helm_values_{}_{}.yaml", h.name, n);
            let mut cmd = std::process::Command::new("helm");
            cmd.args([
                &arg1,
                &arg2,
                "get",
                "values",
                "--all",
                h.name.as_str(),
                "-n",
                n,
                "-o",
                "yaml",
            ]);
            cmdhelms.push((cmd, file_name));
        })
    });

    cmdhelms.into_iter().for_each(|mut c| {
        let folders = folders.clone();
        let task = tokio::task::spawn(async move {
            let o = c.0.output().expect("helm command failed to start");
            if !o.stdout.is_empty() {
                match write_file(&folders[2], &o.stdout, &c.1) {
                    Ok(_) => info!("File has been created {}/{}", &folders[2], &c.1),
                    Err(e) => panic!("{}", e),
                }
            }
            if !o.stderr.is_empty() {
                warn!("{}", String::from_utf8_lossy(&o.stderr))
            }
        });
        fut_handle_helm.push(task);
    });

    for handle in fut_handle_helm {
        match handle.await {
            Ok(_) => {}
            Err(e) => {
                warn!("{}", e)
            }
        }
    }
    //Streaming Cores info.
    //ElasticSearch.
    //Hadoop hdfs info.
    //Hbase info.
    //Kafka info.
    //Prometheus info.

    //ElasticSearch
    let es_pods = get_pod_list(
        pods.clone(),
        "elasticsearch.k8s.elastic.co/node-master=true".to_string(),
        "".to_string(),
    )
    .await?;
    let mut secret_user = String::new();
    if !es_pods.clone().is_empty() {
        let mut secret_list = vec![];
        for sec in secret {
            let s = sec
            .list(&ListParams {
                label_selector: Some("eck.k8s.elastic.co/owner-kind=Elasticsearch, eck.k8s.elastic.co/credentials=true".to_string()),
                ..Default::default()
            })
            .await
            .unwrap()
            .items;
            secret_list.push(s);
        }

        secret_list.iter().for_each(|s| {
            s.iter().for_each(|s| {
                let es_user = s
                    .data
                    .as_ref()
                    .unwrap()
                    .get("elastic")
                    .unwrap()
                    .0
                    .to_owned();
                secret_user = String::from_utf8(es_user).unwrap();
            })
        });

        let command = [
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cluster/health?pretty\"", "health"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cat/indices?h=health,status,index,id,p,r,dc,dd,ss,creation.date.string,&v&s=creation.date:desc\"","indices"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cluster/settings?pretty\"","settings"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cluster/settings?include_defaults=true&pretty\"","defaults_settings")
        ];

        for c in command {
            let pod_name = &es_pods[0].0;
            let apipod = &es_pods[0].2;
            let container = &es_pods[0].3[0];
            let cmd = ["/bin/sh", "-c", &c.0];
            let filename = format!("elastic_search_{}.json", &c.1);
            let data =
                send_command(pod_name.clone(), apipod.clone(), container.clone(), cmd).await?;
            if !data.is_empty() {
                match write_file(&folders[3], data.as_bytes(), &filename) {
                    Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                    Err(e) => panic!("{}", e),
                }
            }
        }
    }

    //Streaming Cores info
    let _streaming_pods = get_pod_list(
        pods.clone(),
        "elasticsearch.k8s.elastic.co/node-master=true".to_string(),
        "".to_string(),
    )
    .await?;
    //Hadoop hdfs info
    let _hadoop_pods = get_pod_list(
        pods.clone(),
        "elasticsearch.k8s.elastic.co/node-master=true".to_string(),
        "".to_string(),
    )
    .await?;
    //Hbase info
    let _hbase_pods = get_pod_list(
        pods.clone(),
        "elasticsearch.k8s.elastic.co/node-master=true".to_string(),
        "".to_string(),
    )
    .await?;
    //Kafka info
    let _kafka_pods = get_pod_list(
        pods.clone(),
        "elasticsearch.k8s.elastic.co/node-master=true".to_string(),
        "".to_string(),
    )
    .await?;
    //Prometheus info
    let _prometheus_pods = get_pod_list(
        pods.clone(),
        "elasticsearch.k8s.elastic.co/node-master=true".to_string(),
        "".to_string(),
    )
    .await?;

    info!("<yellow>LOG collection has been completed!!</>");
    Ok(())
}

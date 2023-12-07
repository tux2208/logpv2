use anyhow::anyhow;
use anyhow::Result;
use chrono::Utc;
use clap::Command;
use flate2::write::GzEncoder;
use flate2::Compression;
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
    let file_name_gz = format!("info_{}_{}.tar.gz", c.context_name, date);
    let folder_to_save = if !c.output_directory_path.is_empty() {
        c.output_directory_path
            .strip_suffix(path::is_separator)
            .unwrap_or(&c.output_directory_path)
            .to_string()
    } else {
        current_dir().unwrap().display().to_string()
    };

    let folder_vec = ["pods", "infra", "helm", "apps"];

    let mut folder_vec = folder_vec
        .iter()
        .map(|f| format!("{}/info_{}_{}/{}", folder_to_save, c.context_name, date, f))
        .collect::<Vec<String>>();

    let folder_src_tar = format!("{}/info_{}_{}", folder_to_save, c.context_name, date);
    folder_vec.push(file_name_gz);
    folder_vec.push(folder_src_tar);
    folder_vec.push(folder_to_save);
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
    let date = Utc::now().format("%Y%m%d%H%M%S");
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
            File::create(format!("output_antlog_gather_tool_{}.log", date)).unwrap(),
        ),
    ])
    .unwrap();
    let kube_config_path = home_dir().unwrap().join(".kube/config").into_os_string();
    //Clap outin
    let value_name = clap::Arg::new("config")
        .short('c')
        .long("config")
        .value_name("CONFIG_FILE_PATH");
    let m = Command::new("Antlog its a Gather Debug Logs Tools.")
        .version("1.0.4")
        .author("tuxedo <wtuxedo@proton.me>")
        .about("Gather useful information for debugging issues raised by the support team.")
        .arg(value_name.help("Config File Path").required(true))
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

    folders.clone()[0..4]
        .iter()
        .for_each(|fo| match fs::create_dir_all(fo) {
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
    let mut fut_handle_kb: Vec<tokio::task::JoinHandle<()>> = vec![];
    cmdk.into_iter().for_each(|mut c| {
        let folders = folders.clone();
        let task = tokio::task::spawn(async move {
            let o = c.0.output().expect("kubectl command failed to start");
            let er = anyhow!("kubectl command empty response {:#?}", c.0);
            match write_file(&folders[0], &o.stdout, &c.1, er) {
                Ok(_) => info!("File has been created {}/{}", &folders[0], &c.1),
                Err(e) => warn!("{}", e),
            }

            if !o.stderr.is_empty() {
                warn!("{}", String::from_utf8_lossy(&o.stderr))
            }
        });
        fut_handle_kb.push(task);
    });

    for handle in fut_handle_kb {
        match handle.await {
            Ok(_) => {}
            Err(e) => {
                warn!("{}", e)
            }
        }
    }
    let mut fut_handle_lc: Vec<tokio::task::JoinHandle<()>> = vec![];
    if config_file.current_logs {
        pods_list.clone().into_iter().for_each(|pl| {
            let container = pl.3.clone();
            for c in container {
                let pl = pl.clone();
                let pname = pl.0.clone();
                let folders = folders.clone();
                let task = tokio::task::spawn(async move {
                    let l = get_logs(pname, c.to_string(), pl.2, false).await;
                    match l {
                        Ok(l) => {
                            let filename = format!("logs_current_{}_{}_{}.log", &pl.1, pl.0, c);
                            let er = anyhow!("No Log found {} on container {}.", pl.0, c);
                            match write_file(&folders[0], l.as_bytes(), &filename, er) {
                                Ok(_) => {
                                    info!("File has been created {}/{}", &folders[0], filename)
                                }
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

                fut_handle_lc.push(task);
            }
        });
    }
    for handle in fut_handle_lc {
        match handle.await {
            Ok(_) => {}
            Err(e) => {
                warn!("{}", e)
            }
        }
    }
    let mut fut_handle_lp: Vec<tokio::task::JoinHandle<()>> = vec![];
    if config_file.previous_logs {
        pods_list.clone().into_iter().for_each(|pl| {
            let container = pl.3.clone();
            for c in container {
                let pl = pl.clone();
                let folders = folders.clone();
                let pname = pl.0.clone();
                let task = tokio::task::spawn(async move {
                    let l = get_logs(pl.0, c.to_string(), pl.2, true).await;
                    match l {
                        Ok(l) => {
                            let filename = format!("logs_previous_{}_{}_{}.log", &pl.1, &pname, c);
                            let er = anyhow!("No Log found {} on container {}.", pname, c);
                            match write_file(&folders[0], l.as_bytes(), &filename, er) {
                                Ok(_) => {
                                    info!("File has been created {}/{}", &folders[0], filename)
                                }
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
                fut_handle_lp.push(task);
            }
        });
    }

    for handle in fut_handle_lp {
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
    cmd.args([
        "get",
        "events",
        "-A",
        "--context",
        &config_file.context_name,
    ]);
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
            let er = anyhow!("kubectl command empty response {:#?}", c.0);
            match write_file(&folders[1], &o.stdout, &c.1, er) {
                Ok(_) => info!("File has been created {}/{}", &folders[1], &c.1),
                Err(e) => warn!("{}", e),
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
            let er = anyhow!("kubectl command empty response {:#?}", c.0);
            match write_file(&folders[2], &o.stdout, &c.1, er) {
                Ok(_) => info!("File has been created {}/{}", &folders[2], &c.1),
                Err(e) => warn!("{}", e),
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
    let mut fut_handle_es = vec![];
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

        let command_es = [
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
                + " -X GET \"https://localhost:9200/_cluster/settings?include_defaults=true&pretty\"","defaults_settings"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cat/nodes?v&pretty\"","nodes"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cat/_cat/shards?v\"","shards"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cluster/state?pretty\"","state"),
            ("curl -k -u elastic:".to_string()
                + secret_user.as_str()
                + " -X GET \"https://localhost:9200/_cluster/stats?human&pretty\"","stats_human")
        ];

        for c in command_es {
            let folders = folders.clone();
            let es_pods = es_pods.clone();
            let task = tokio::task::spawn(async move {
                let pod_name = &es_pods[0].0;
                let apipod = &es_pods[0].2;
                let container = &es_pods[0].3[0];
                let cmd = ["/bin/sh", "-c", &c.0];
                let filename = format!("elastic_search_{}.json", &c.1);
                let data = send_command(pod_name.clone(), apipod.clone(), container.clone(), cmd)
                    .await
                    .unwrap();

                let er = anyhow!("kubectl command empty response {:#?}", c.0);
                match write_file(&folders[3], data.as_bytes(), &filename, er) {
                    Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                    Err(e) => warn!("{}", e),
                }
            });
            fut_handle_es.push(task);
        }
        for handle in fut_handle_es {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    warn!("{}", e)
                }
            }
        }
    }

    //Streaming Cores info
    let streaming_core_pods = get_pod_list(
        pods.clone(),
        "spark-role=driver,app.kubernetes.io/component=streaming-core-consumer".to_string(),
        "".to_string(),
    )
    .await?;
    let mut fut_handle_sc = vec![];
    if !streaming_core_pods.is_empty() {
        for sc in streaming_core_pods {
            let cmd = [
                "/bin/sh",
                "-c",
                "curl -s localhost:4040/api/v1/applications | jq -r  '.[0] | .id' | tr -d '\n'",
            ];

            let application_id = send_command(sc.0.clone(), sc.2.clone(), sc.3[0].to_string(), cmd)
                .await
                .unwrap();

            let command_sc = [
                (
                    format!(
                        "curl \"localhost:4040/api/v1/applications/{}/environment\"",
                        application_id
                    ),
                    "environment.json",
                ),
                (
                    format!(
                        "curl \"localhost:4040/api/v1/applications/{}/executors\"",
                        application_id
                    ),
                    "executors.json",
                ),
                (
                    format!(
                        "curl \"localhost:4040/api/v1/applications/{}/streaming/statistics\"",
                        application_id
                    ),
                    "streaming_statistics.json",
                ),
                (
                    format!(
                        "curl \"localhost:4040/api/v1/applications/{}/streaming/batches\"",
                        application_id
                    ),
                    "streaming_batches.json",
                ),
            ];

            for c in command_sc {
                let folders = folders.clone();
                let sc = sc.clone();
                let task = tokio::task::spawn(async move {
                    let cmd = ["/bin/sh", "-c", &c.0];
                    let filename = format!("{}_{}", sc.0, &c.1);
                    let data = send_command(sc.0, sc.2, sc.3[0].to_string(), cmd)
                        .await
                        .unwrap();
                    let data = jsonxf::pretty_print(&data).unwrap();
                    let er = anyhow!("kubectl command empty response {:#?}", c.0);
                    match write_file(&folders[3], data.as_bytes(), &filename, er) {
                        Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                        Err(e) => warn!("{}", e),
                    }
                });
                fut_handle_sc.push(task);
            }
        }
        for handle in fut_handle_sc {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    warn!("{}", e)
                }
            }
        }
    }

    //Hadoop hdfs info
    let hadoop_pods = get_pod_list(
        pods.clone(),
        "app.kubernetes.io/component=datanode".to_string(),
        "".to_string(),
    )
    .await?;
    let mut fut_handle_hd = vec![];
    if !hadoop_pods.is_empty() {
        let command_hd = [
            ("hdfs dfsadmin -report", "report_dfsadmin"),
            ("hdfs dfsadmin -safemode get", "safe_mode"),
            (
                "time dd if=/dev/zero of=/dfs/test conv=fsync bs=384k count=10K",
                "hdfs_diskwrite_perf",
            ),
        ];

        for c in command_hd {
            let folders = folders.clone();
            let hadoop_pods = hadoop_pods.clone();
            let task = tokio::task::spawn(async move {
                let pod_name = &hadoop_pods.first().as_ref().unwrap().0;
                let apipod = &hadoop_pods.first().as_ref().unwrap().2;
                let container = &hadoop_pods.first().as_ref().unwrap().3[0];
                let cmd = ["/bin/sh", "-c", &c.0];
                let filename = format!("hadoop_{}.log", &c.1);
                let data = send_command(pod_name.clone(), apipod.clone(), container.clone(), cmd)
                    .await
                    .unwrap();
                let er = anyhow!("kubectl command empty response {:#?}", c.0);
                match write_file(&folders[3], data.as_bytes(), &filename, er) {
                    Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                    Err(e) => warn!("{}", e),
                }
            });
            fut_handle_hd.push(task);
        }
        for handle in fut_handle_hd {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    warn!("{}", e)
                }
            }
        }
    }
    //Hbase info
    let hbase_pods = get_pod_list(
        pods.clone(),
        "app.kubernetes.io/name=hbase, app.kubernetes.io/component=master".to_string(),
        "".to_string(),
    )
    .await?;

    let mut fut_handle_hb = vec![];
    if !hbase_pods.is_empty() {
        let command_hb = [(
            "echo \"status 'detailed'\" | hbase shell",
            "status_detailed",
        )];

        for c in command_hb {
            let folders = folders.clone();
            let hbase_pods = hbase_pods.clone();
            let task = tokio::task::spawn(async move {
                let pod_name = &hbase_pods.first().as_ref().unwrap().0;
                let apipod = &hbase_pods.first().as_ref().unwrap().2;
                let container = &hbase_pods.first().as_ref().unwrap().3[0];
                let cmd = ["/bin/sh", "-c", &c.0];
                let filename = format!("hbase_{}.log", &c.1);
                let data = send_command(pod_name.clone(), apipod.clone(), container.clone(), cmd)
                    .await
                    .unwrap();
                let er = anyhow!("kubectl command empty response {:#?}", c.0);
                match write_file(&folders[3], data.as_bytes(), &filename, er) {
                    Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                    Err(e) => warn!("{}", e),
                }
            });
            fut_handle_hb.push(task);
        }
        for handle in fut_handle_hb {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    warn!("{}", e)
                }
            }
        }
    }

    //Kafka info
    let label_k = [
        "app.kubernetes.io/name=kafka",
        "app.kubernetes.io/name=eric-data-message-bus-kf",
    ];
    let mut kafka_pods = vec![];
    let mut p = "";
    for k in label_k {
        let kf = get_pod_list(pods.clone(), k.to_string(), "".to_string()).await?;
        if !kf.is_empty() {
            kafka_pods.push(kf);
            p = k;
        }
    }

    let mut fut_handle_kf = vec![];
    if !kafka_pods[0].is_empty() {
        let prefix = match p {
            "app.kubernetes.io/name=kafka" => "bin/",
            "app.kubernetes.io/name=eric-data-message-bus-kf" => "",
            _ => "",
        };

        let command_kf = [
            (
                prefix.to_owned() + "kafka-topics.sh --bootstrap-server localhost:9092 --list",
                "topics",
            ),
            (
                prefix.to_owned() + "kafka-topics.sh --bootstrap-server localhost:9092 --describe",
                "topics_description",
            ),
            (
                prefix.to_owned()
                    + "kafka-consumer-groups.sh --bootstrap-server localhost:9092 --list",
                "groups_list",
            ),
            (
                prefix.to_owned()
                    + "kafka-broker-api-versions.sh --bootstrap-server localhost:9092 | awk '/^[a-z]/ {print $1}'",
                "brokers_list",
            ),
            (
                prefix.to_owned()
                    + "kafka-consumer-groups.sh --bootstrap-server localhost:9092 --describe --all-groups",
                "groups_describe",
            ),
        ];
        for c in command_kf {
            let folders = folders.clone();
            let kafka_pods = kafka_pods.clone();
            let task = tokio::task::spawn(async move {
                let pod_name = &kafka_pods[0].first().as_ref().unwrap().0;
                let apipod = &kafka_pods[0].first().as_ref().unwrap().2;
                let container = &kafka_pods[0].first().as_ref().unwrap().3[0];
                let cmd = ["/bin/sh", "-c", &c.0];
                let filename = format!("kafka_{}.log", &c.1);
                let data = send_command(pod_name.clone(), apipod.clone(), container.clone(), cmd)
                    .await
                    .unwrap();
                let er = anyhow!("kubectl command empty response {:#?}", c.0);
                match write_file(&folders[3], data.as_bytes(), &filename, er) {
                    Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                    Err(e) => warn!("{}", e),
                }
            });
            fut_handle_kf.push(task);
        }
        for handle in fut_handle_kf {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    warn!("{}", e)
                }
            }
        }
    }
    //Prometheus info
    let mut fut_handle_pro = vec![];
    let prometheus_pods = get_pod_list(
        pods.clone(),
        "app.kubernetes.io/name=prometheus".to_string(),
        "".to_string(),
    )
    .await?;
    if !prometheus_pods.is_empty() {
        let pod_name = prometheus_pods.first().as_ref().unwrap().0.as_str();
        let mut path = ["midlayer", "session", "titan-ns"]
            .into_iter()
            .filter(|&i| pod_name.contains(i))
            .collect::<Vec<&str>>();
        if path.is_empty() {
            path.push(&prometheus_pods.first().as_ref().unwrap().1)
        }
        let command_prometheus = [
            (
                format!(
                    "wget -q 'http://127.0.0.1:9090/{}/prometheus/api/v1/rules' -O -",
                    path[0]
                ),
                "rules.json",
            ),
            (
                format!(
                    "wget -q 'http://127.0.0.1:9090/{}/prometheus/api/v1/alerts' -O -",
                    path[0]
                ),
                "alerts.json",
            ),
            (
                format!(
                    "wget -q 'http://127.0.0.1:9090/{}/prometheus/api/v1/targets' -O -",
                    path[0]
                ),
                "targets.json",
            ),
            (
                format!(
                    "wget -q 'http://127.0.0.1:9090/{}/prometheus/api/v1/status/runtimeinfo' -O -",
                    path[0]
                ),
                "runtime_info.json",
            ),
            (
                format!(
                    "wget -q 'http://127.0.0.1:9090/{}/prometheus/api/v1/status/buildinfo' -O -",
                    path[0]
                ),
                "build_info.json",
            ),
        ];
        for c in command_prometheus {
            let folders = folders.clone();
            let prometheus_pods = prometheus_pods.clone();
            let task = tokio::task::spawn(async move {
                let pod_name = &prometheus_pods.first().as_ref().unwrap().0;
                let apipod = &prometheus_pods.first().as_ref().unwrap().2;
                let container = &prometheus_pods.first().as_ref().unwrap().3[0];
                let namespace = &prometheus_pods.first().as_ref().unwrap().1;
                let cmd = ["/bin/sh", "-c", &c.0];
                let filename = format!("prometheus_{}_{}", namespace, &c.1);
                let data = send_command(pod_name.clone(), apipod.clone(), container.clone(), cmd)
                    .await
                    .unwrap();

                let data = jsonxf::pretty_print(&data).unwrap();
                let er = anyhow!("kubectl command empty response {:#?}", c.0);
                match write_file(&folders[3], data.as_bytes(), &filename, er) {
                    Ok(_) => info!("File has been created {}/{}", &folders[3], &filename),
                    Err(e) => warn!("{}", e),
                }
            });
            fut_handle_pro.push(task);
        }
        for handle in fut_handle_pro {
            match handle.await {
                Ok(_) => {}
                Err(e) => {
                    warn!("{}", e)
                }
            }
        }
    }
    //tar file process

    let path = format!("{}/{}", &folders[6], &folders[4]);
    info!(
        "tar file is being created and then then it will be copied to the following path ...{}",
        &path
    );
    info!("<yellow>this action will take few minutes...</>");
    let tar_gz = File::create(&path)?;
    let enc = GzEncoder::new(tar_gz, Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all(folders[6].split('/').last().unwrap(), &folders[5])?;
    info!("tar file has been created on ... {}", &path);

    //Finish log Collection Msg.
    info!("<green>LOG collection has been completed!!</>");
    info!("<yellow>Starting Cleaning Phase!!</>");

    let antlog = format!("output_antlog_gather_tool_{}.log", date);
    let mut log_antlog = File::open(format!("output_antlog_gather_tool_{}.log", date)).unwrap();

    match tar.append_file(&antlog, &mut log_antlog) {
        Ok(_) => info!(
            "output_antlog_gather_tool_{}.log has been add it to the tar file.",
            date
        ),
        Err(e) => warn!("{}", e),
    }

    match tar.finish() {
        Ok(_) => info!("tar file {} integrity its OK", path),
        Err(e) => warn!("{}", e),
    }

    match fs::remove_dir_all(&folders[5]) {
        Ok(_) => info!("Folder has been remove {}", folders[5]),
        Err(e) => warn!("{}", e),
    }
    info!("<yellow>Finishing Cleaning Phase!!</>");
    info!("<green>END!!</>");
    Ok(())
}

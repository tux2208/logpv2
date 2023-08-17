use anyhow::Result;
use chrono::Utc;
use clap::Command;
use home::home_dir;
use kube::{api::ListParams, ResourceExt};
use logpv2::*;
use simplelog::{info, ConfigBuilder, LevelFilter, TermLogger, TerminalMode};
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
            if c.output_directory_path != "" {
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

    let config_file_path = m.get_one::<String>("config").unwrap();

    let config_file = read_config_file(&config_file_path)?;

    let kube_config_path = m.get_one::<String>("kube_config_path").unwrap();

    let pods = kubernetes_client(kube_config_path, config_file.clone()).await?;

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
    if config_file.strimzi_operator_namespace != "" {
        info!(
            "strimzi_operator_namespace: {}",
            &config_file.strimzi_operator_namespace
        )
    }

    for lp in pods {
        lp.list(&ListParams::default())
            .await?
            .items
            .iter()
            .for_each(|p| {
                println!(
                    "Pod: {} runs: {}",
                    p.name_any(),
                    p.status.as_ref().unwrap().phase.as_ref().unwrap()
                );
            });
    }

    println!("{:?}", folders);

    Ok(())
}

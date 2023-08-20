use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{ListParams, LogParams},
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config, ResourceExt,
};
use serde::Deserialize;
use std::{
    fs,
    io::{BufWriter, Write},
};

#[derive(Default, Debug, Clone, PartialEq, Deserialize)]
pub struct ConfigFile {
    pub context_name: String,
    pub context_namespace: Vec<String>,
    pub output_directory_path: String,
    pub strimzi_operator_namespace: String,
    pub previous_logs: bool,
    pub current_logs: bool,
}

pub async fn kubernetes_client(
    kube_config_path: &String,
    config_file: ConfigFile,
) -> Result<Vec<Api<Pod>>> {
    let kube_config = Kubeconfig::read_from(kube_config_path)?;

    //options for the kubernetes configuration.
    let kube_config_options = KubeConfigOptions {
        //context name.
        context: Some(config_file.context_name),
        ..Default::default()
    };
    let mut vpods = vec![];
    //create kubernetes configuration.
    let k_config = Config::from_custom_kubeconfig(kube_config, &kube_config_options).await?;

    //create kubernetes client.
    let client: Client =
        Client::try_from(k_config).expect("Expected a valid KUBECONFIG environment variable.");

    config_file.context_namespace.iter().for_each(|cn| {
        let pods: Api<Pod> = Api::namespaced(client.clone(), cn);
        vpods.push(pods);
    });

    Ok(vpods)
}

pub fn write_file(folder: &str, data: &[u8], filename: &str) -> Result<()> {
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(folder.to_owned() + "/" + filename)?;
    let mut file = BufWriter::new(file);
    file.write_all(data)?;
    Ok(())
}

pub async fn get_pod_list(
    pods: Vec<Api<Pod>>,
) -> Result<Vec<(String, String, Api<Pod>, Vec<String>)>> {
    let mut plns = vec![];
    for p in pods {
        p.list(&ListParams::default())
            .await?
            .items
            .iter()
            .for_each(|i| {
                let pl = (
                    i.name_any(),
                    i.namespace().as_ref().unwrap().to_string(),
                    p.clone(),
                    i.spec
                        .as_ref()
                        .unwrap()
                        .containers
                        .iter()
                        .map(|c| c.clone().name)
                        .collect::<Vec<String>>(),
                );
                plns.push(pl);
            })
    }
    Ok(plns)
}

pub async fn get_logs(
    pname: String,
    pcontainer: String,
    pods: Api<Pod>,
    previous: bool,
) -> Result<String> {
    let l = pods
        .logs(
            &pname,
            &LogParams {
                container: Some(pcontainer),
                pretty: (true),
                previous: (previous),
                ..Default::default()
            },
        )
        .await?;

    Ok(l)
}

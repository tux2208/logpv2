use anyhow::Result;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config,
};
use serde::Deserialize;

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
        let pods: Api<Pod> = Api::namespaced(client.clone(), &cn);
        vpods.push(pods);
    });

    Ok(vpods)
}

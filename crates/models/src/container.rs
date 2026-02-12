use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ContainerInfo {
    pub id: String,
    pub names: Vec<String>,
    pub image: String,
    pub state: String,
    pub status: Option<String>,
    pub created: i64,
    pub ports: Vec<(u16, Option<u16>, Option<String>)>,
}

#[derive(Serialize)]
pub struct ImageInfo {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
    pub exposed_ports: Vec<u16>,
    pub env: Vec<String>,
    pub volumes: Vec<String>,
}

#[derive(Serialize)]
pub struct VolumeInfo {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    pub created_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkInfo {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub internal: bool,
    pub attachable: bool,
    pub ingress: bool,
    #[serde(rename = "IPAM")]
    pub ipam: NetworkIpam,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkIpam {
    pub driver: String,
    pub config: Vec<NetworkIpamConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkIpamConfig {
    pub subnet: Option<String>,
    pub gateway: Option<String>,
}

#[derive(Deserialize)]
pub struct IdReq {
    pub id: String,
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct PullReq {
    pub image: String,
    pub tag: Option<String>,
}

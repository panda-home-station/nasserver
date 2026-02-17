use serde::Deserialize;

#[derive(Deserialize, Clone, Debug)]
pub struct IdReq {
    pub id: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CreateContainerReq {
    pub image_id: String,
    pub name: Option<String>,
    pub cpu_limit: Option<f64>,
    pub memory_limit: Option<f64>,
    pub auto_start: Option<bool>,
    pub ports: Option<Vec<PortMapping>>,
    pub volumes: Option<Vec<String>>,
    pub env: Option<Vec<String>>,
    pub gpu_id: Option<String>,
    pub privileged: Option<bool>,
    pub cap_add: Option<Vec<String>>,
    pub network_mode: Option<String>,
    pub cmd: Option<Vec<String>>,
    // Added by backend
    #[serde(skip_deserializing)]
    pub username: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PortMapping {
    pub host: String,
    pub container: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PullImageReq {
    pub image: String,
    pub tag: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CreateVolumeReq {
    pub name: String,
    pub driver: Option<String>,
    pub labels: Option<std::collections::HashMap<String, String>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct RemoveVolumeReq {
    pub name: String,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppStatus {
    Installing,
    Running,
    Stopped,
    Error(String),
    Uninstalling,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AppType {
    Docker,
    Binary,
    Script,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub version: String,
    pub app_type: AppType,
    pub status: AppStatus,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub port: Option<u16>,
    pub entrypoint: String,
}

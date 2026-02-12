use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use sqlx::{Pool, Sqlite};
// use sysinfo::{System, Disks, Networks, Components};
use auth::AuthService;
use system::SystemService;
use storage::StorageService;
use container::{ContainerService, AppManager};
use downloader::DownloaderService;
use agent::AgentService;
use task::TaskService;

#[derive(Clone)]
pub struct AppState {
    pub device_codes: &'static Lazy<Mutex<HashMap<String, i64>>>,
    pub db: Pool<Sqlite>,
    pub jwt_secret: String,
    pub storage_path: String,
    pub app_manager: Arc<dyn AppManager>,
    pub auth_service: Arc<dyn AuthService>,
    pub system_service: Arc<dyn SystemService>,
    pub storage_service: Arc<dyn StorageService>,
    pub container_service: Arc<dyn ContainerService>,
    pub downloader_service: Arc<dyn DownloaderService>,
    pub agent_service: Arc<dyn AgentService>,
    pub task_service: Arc<dyn TaskService>,
}


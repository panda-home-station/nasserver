use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use chrono::Utc;
use once_cell::sync::Lazy;
use sqlx::{Pool, Sqlite};
// use sysinfo::{System, Disks, Networks, Components};
use crate::services::{AppManager, AuthService, SystemService, StorageService, ContainerService, DownloaderService, AgentService, TaskService};

pub static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
pub static START_TIME: Lazy<chrono::DateTime<Utc>> = Lazy::new(|| Utc::now());

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


use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use once_cell::sync::Lazy;
use sqlx::{Pool, Postgres};
use domain::auth::AuthService;
use domain::system::SystemService;
use domain::storage::StorageService;
use domain::container::{ContainerService, AppManager};
use domain::downloader::DownloaderService;
use domain::agent::AgentService;
use domain::blobfs::BlobFsService;
use domain::task::TaskService;
use chrono::Utc;

pub static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
pub static START_TIME: Lazy<chrono::DateTime<Utc>> = Lazy::new(|| Utc::now());

#[derive(Clone)]
pub struct AppState {
    pub db: Pool<Postgres>, // 默认保留 sys 连接池，兼容旧代码
    pub db_storage: Pool<Postgres>, // 专门给存储相关 Service 使用
    pub db_agent: Pool<Postgres>, // 专门给 agent 相关 Service 使用
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
    pub blob_fs_service: Arc<dyn BlobFsService>,
}

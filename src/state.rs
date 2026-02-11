use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use chrono::Utc;
use once_cell::sync::Lazy;
use sqlx::{Pool, Sqlite};
use sysinfo::{System, Disks, Networks, Components};

pub static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
pub static START_TIME: Lazy<chrono::DateTime<Utc>> = Lazy::new(|| Utc::now());

#[derive(Clone)]
pub struct AppState {
    pub device_codes: &'static Lazy<Mutex<HashMap<String, i64>>>,
    pub db: Pool<Sqlite>,
    pub jwt_secret: String,
    pub storage_path: String,
    pub sys: Arc<Mutex<System>>,
    pub disks: Arc<Mutex<Disks>>,
    pub networks: Arc<Mutex<Networks>>,
    pub components: Arc<Mutex<Components>>,
    pub download_tasks: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
    pub torrent_session: Arc<librqbit::Session>,
    pub magnet_cache: Arc<Mutex<HashMap<String, bytes::Bytes>>>,
    pub last_stats: Arc<Mutex<Option<crate::models::system::SystemStats>>>,
}


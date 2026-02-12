pub mod state;
pub mod config;
pub mod db;
pub mod watcher;

pub use state::AppState;
use std::sync::Arc;
use common::DEVICE_CODES;

use downloader::DownloaderServiceImpl;
use container::{DockerAppManager, ContainerServiceImpl};
use auth::AuthServiceImpl;
use system::SystemServiceImpl;
use storage::StorageServiceImpl;
use agent::AgentServiceImpl;
use task::TaskServiceImpl;

pub async fn init() -> AppState {
    dotenvy::dotenv().ok();

    let storage_path = std::env::var("PNAS_DEV_STORAGE_PATH")
        .or_else(|_| config::read_env_var_from_file("PNAS_DEV_STORAGE_PATH"))
        .unwrap_or_else(|_| "/var/panda/system".to_string());
        
    let _ = std::fs::create_dir_all(&storage_path);
    let _ = std::fs::create_dir_all(format!("{}/vol1", &storage_path));
    let _ = std::fs::create_dir_all(format!("{}/vol1/User", &storage_path));
    let _ = std::fs::create_dir_all(format!("{}/vol1/AppData", &storage_path));

    let mut db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        let db_dir = format!("{}/db", storage_path);
        let _ = std::fs::create_dir_all(&db_dir);
        format!("sqlite://{}/pnas.db", db_dir)
    });
    
    if db_url.starts_with("sqlite:/") && !db_url.starts_with("sqlite:///") {
        db_url = db_url.replacen("sqlite:/", "sqlite:///", 1);
    }

    let pool = db::init_db(&db_url).await;
    
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    
    let app_manager = Arc::new(DockerAppManager::new());
    let auth_service = Arc::new(AuthServiceImpl::new(pool.clone(), jwt_secret.clone(), storage_path.clone()));
    let system_service = Arc::new(SystemServiceImpl::new(pool.clone()));
    let storage_service = Arc::new(StorageServiceImpl::new(pool.clone(), storage_path.clone()));
    let container_service = Arc::new(ContainerServiceImpl::new());
    let agent_service = Arc::new(AgentServiceImpl::new());
    let task_service = Arc::new(TaskServiceImpl::new(pool.clone()));

    let torrent_dir = format!("{}/torrents", storage_path);
    let _ = std::fs::create_dir_all(&torrent_dir);
    let mut session_opts = librqbit::SessionOptions::default();
    session_opts.enable_upnp_port_forwarding = true;
    let session = librqbit::Session::new_with_opts(torrent_dir.into(), session_opts).await.expect("Failed to init torrent session");

    let downloader_service = Arc::new(DownloaderServiceImpl::new(
        pool.clone(),
        storage_path.clone(),
        session.clone(),
    ));

    AppState {
        device_codes: &DEVICE_CODES,
        db: pool,
        jwt_secret,
        storage_path,
        app_manager,
        auth_service,
        system_service,
        storage_service,
        container_service,
        downloader_service,
        agent_service,
        task_service,
    }
}

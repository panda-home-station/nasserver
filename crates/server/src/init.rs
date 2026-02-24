use std::sync::Arc;
use infra::{AppState, db, config, state::START_TIME};

use downloader::DownloaderServiceImpl;
use container::{DockerAppManager, ContainerServiceImpl};
use auth::AuthServiceImpl;
use system::SystemServiceImpl;
use storage::StorageServiceImpl;
use agent::AgentServiceImpl;
use task::TaskServiceImpl;
// use fuse_fs::BlobFsServiceImpl;

pub async fn init() -> AppState {
    dotenvy::dotenv().ok();

    let storage_path = std::env::var("PNAS_DEV_STORAGE_PATH")
        .or_else(|_| config::read_env_var_from_file("PNAS_DEV_STORAGE_PATH"))
        .unwrap_or_else(|_| "/var/panda/system".to_string());
        
    let _ = std::fs::create_dir_all(&storage_path);
    let _ = std::fs::create_dir_all(format!("{}/vol1", &storage_path));
    let _ = std::fs::create_dir_all(format!("{}/vol1/User", &storage_path));
    let _ = std::fs::create_dir_all(format!("{}/vol1/User_Data", &storage_path));
    let _ = std::fs::create_dir_all(format!("{}/vol1/AppData", &storage_path));

    // Initialize database connection
    // Default to Unix socket connection which is more reliable for local setup
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://jolly:password@localhost/pnas_db?host=/var/run/postgresql".to_string());

    let pools = db::init_db(&db_url).await;
    
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    
    let app_manager = Arc::new(DockerAppManager::new());
    let system_service = Arc::new(SystemServiceImpl::new(pools.sys.clone(), *START_TIME));
    let storage_service = Arc::new(StorageServiceImpl::new(pools.storage.clone(), storage_path.clone()));
    // let blob_fs_service = Arc::new(BlobFsServiceImpl::new(storage_service.clone(), format!("{}/vol1", storage_path)));
    let auth_service = Arc::new(AuthServiceImpl::new(pools.sys.clone(), jwt_secret.clone(), storage_path.clone()));
    let container_service = Arc::new(ContainerServiceImpl::new(storage_path.clone()));
    let task_service = Arc::new(TaskServiceImpl::new(pools.storage.clone()));

    let torrent_dir = format!("{}/torrents", storage_path);
    let _ = std::fs::create_dir_all(&torrent_dir);
    let mut session_opts = librqbit::SessionOptions::default();
    session_opts.enable_upnp_port_forwarding = true;
    let session = librqbit::Session::new_with_opts(torrent_dir.into(), session_opts).await.expect("Failed to init torrent session");

    let downloader_service = Arc::new(DownloaderServiceImpl::new(
        pools.storage.clone(),
        storage_path.clone(),
        session.clone(),
    ));

    let agent_service = Arc::new(AgentServiceImpl::new(
        pools.agent.clone(),
        system_service.clone(),
        storage_service.clone(),
        auth_service.clone(),
        downloader_service.clone(),
        container_service.clone(),
        app_manager.clone(),
        task_service.clone(),
        // blob_fs_service.clone(),
        format!("{}/vol1", storage_path)
    ));

    let agent_service_dyn: Arc<dyn agent::AgentService> = agent_service.clone();
    agent_service.set_self_ref(Arc::downgrade(&agent_service_dyn));

    AppState {
        db: pools.sys,
        db_storage: pools.storage,
        db_agent: pools.agent,
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
        // blob_fs_service,
    }
}

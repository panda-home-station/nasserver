mod init;

use std::net::SocketAddr;
// use infra::sqlx; // Import sqlx from infra
// use infra::sqlx::Row; // Import Row trait

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .init();

    // 1. Initialize infrastructure (Config, DB, Services, AppState)
    let state = init::init().await;

    // 2. Start background tasks

    let sys_service = state.system_service.clone();
    tokio::spawn(async move {
        sys_service.run_background_stats_collector().await;
    });

    let storage_service = state.storage_service.clone();
    tokio::spawn(async move {
        storage_service.run_trash_purger().await;
    });

    /*
    // 2.5 Mount FUSE for all existing users
    let db = state.db.clone();
    let blob_fs = state.blob_fs_service.clone();
    tokio::spawn(async move {
        // Wait a short time for DB to be fully ready (though init() already ensures connection)
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        
        match sqlx::query("SELECT username FROM users").fetch_all(&db).await {
            Ok(rows) => {
                for row in rows {
                    let username: String = row.get("username");
                    if let Err(e) = blob_fs.mount_for_user(&username).await {
                        tracing::error!("Failed to mount blob storage for user {}: {}", username, e);
                    } else {
                        tracing::info!("Mounted blob storage for user {}", username);
                    }
                }
            }
            Err(e) => {
                tracing::error!("Failed to fetch users for FUSE mounting: {}", e);
            }
        }
    });
    */

    // 3. Start Static Server
    let static_port: u16 = std::env::var("PNAS_STATIC_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8080);
    println!("Static listening on port {}", static_port);
    let static_addr: SocketAddr = ([0, 0, 0, 0], static_port).into();
    let static_app = api::static_app();
    
    tokio::spawn(async move {
        let static_listener = tokio::net::TcpListener::bind(static_addr).await.unwrap();
        let _ = axum::serve(static_listener, static_app).await;
    });

    // 4. Start API Server
    let port: u16 = std::env::var("PNAS_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8000);
    println!("Backend listening on port {}", port);
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let api_app = api::api_app(state);
    
    axum::serve(listener, api_app).await.unwrap();
}

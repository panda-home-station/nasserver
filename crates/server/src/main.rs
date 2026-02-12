use dotenvy::dotenv;
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;

mod state;
mod handlers;
mod middleware;
mod routes;
mod watcher;
mod api;

use state::AppState;
use common::DEVICE_CODES;

use std::sync::Arc;
use downloader::DownloaderServiceImpl;
use container::{DockerAppManager, ContainerServiceImpl};
use auth::AuthServiceImpl;
use system::SystemServiceImpl;
use storage::StorageServiceImpl;
use agent::AgentServiceImpl;
use task::TaskServiceImpl;


#[tokio::main]
async fn main() {
    dotenv().ok();

    let storage_path = std::env::var("PNAS_DEV_STORAGE_PATH")
        .or_else(|_| read_env_var_from_file("PNAS_DEV_STORAGE_PATH"))
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
    // Normalize sqlite absolute path URLs: ensure "sqlite:///" for absolute paths
    if db_url.starts_with("sqlite:/") && !db_url.starts_with("sqlite:///") {
        let fixed = db_url.replacen("sqlite:/", "sqlite:///", 1);
        println!("Adjusted DATABASE_URL to {}", fixed);
        db_url = fixed;
    }
    // Proactively create DB file if missing to avoid SQLITE_CANTOPEN (code 14)
    if let Some(path) = db_url.strip_prefix("sqlite://") {
        let db_path = if path.starts_with('/') { path.to_string() } else { format!("/{}", path) };
        if let Some(parent) = std::path::Path::new(&db_path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !std::path::Path::new(&db_path).exists() {
            let _ = std::fs::File::create(&db_path);
        }
    }
    
    // Try to connect to database directly for better error handling
    let db = match SqlitePoolOptions::new().max_connections(1).connect(&db_url).await {
        Ok(pool) => {
            println!("Database connected successfully: {}", db_url);
            // Enable WAL mode for better concurrency
            let _ = sqlx::query("PRAGMA journal_mode=WAL;")
                .execute(&pool)
                .await;
            pool
        },
        Err(e) => {
            eprintln!("Failed to connect to database: {}. Error: {}", db_url, e);
            std::process::exit(1);
        }
    };
    
    // Init DB schema
    println!("Initializing database schema...");
    sqlx::query(
        "create table if not exists users (
            id text primary key,
            username text unique,
            email text,
            password_hash text not null,
                created_at timestamp default CURRENT_TIMESTAMP
            )",
    )
    .execute(&db)
    .await
    .expect("Failed to create users table");
    println!("Users table created/verified");

    // Check if 'role' column exists before attempting to add it
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'role'")
        .fetch_one(&db)
        .await
        .expect("Failed to query pragma_table_info for users table");

    if row.0 == 0 {
        sqlx::query("ALTER TABLE users ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")
            .execute(&db)
            .await
            .expect("Failed to alter users table to add role column");
        println!("Users role column added");
    } else {
        println!("Users role column already exists");
    }

    // Add wallpaper column for per-user desktop settings if missing
    let row_wp: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('users') WHERE name = 'wallpaper'")
        .fetch_one(&db)
        .await
        .expect("Failed to query pragma_table_info for users table (wallpaper)");
    if row_wp.0 == 0 {
        sqlx::query("ALTER TABLE users ADD COLUMN wallpaper TEXT")
            .execute(&db)
            .await
            .expect("Failed to alter users table to add wallpaper column");
        println!("Users wallpaper column added");
    } else {
        println!("Users wallpaper column already exists");
    }

    sqlx::query(
        "create table if not exists system_config (
            key text primary key,
            value text
        )",
    )
    .execute(&db)
    .await
    .expect("Failed to create system_config table");
    println!("System config table created/verified");
    
    // Create file_tasks table for task management
    println!("Creating file_tasks table...");
    sqlx::query(
        "create table if not exists file_tasks (
            id text primary key,
            type text not null,
            name text not null,
            dir text,
            progress integer default 0,
            status text not null,
            created_at timestamp default CURRENT_TIMESTAMP,
            updated_at timestamp default CURRENT_TIMESTAMP
        )",
    )
    .execute(&db)
    .await
    .expect("Failed to create file_tasks table");
    println!("File tasks table created/verified");

    // Create downloads table
    println!("Creating downloads table...");
    sqlx::query(
        "create table if not exists downloads (
            id text primary key,
            url text not null,
            path text not null,
            filename text not null,
            status text not null,
            progress real default 0,
            total_bytes integer default 0,
            downloaded_bytes integer default 0,
            speed integer default 0,
            created_at timestamp default CURRENT_TIMESTAMP,
            updated_at timestamp default CURRENT_TIMESTAMP,
            error_msg text
        )"
    )
    .execute(&db)
    .await
    .expect("Failed to create downloads table");
    println!("Downloads table created/verified");
    
    sqlx::query(
        "create table if not exists cloud_files (
            id text primary key,
            user_id text not null,
            name text not null,
            dir text,
            size integer default 0,
            mime text,
            checksum text,
            storage text not null default 'file',
            created_at timestamp default CURRENT_TIMESTAMP,
            updated_at timestamp default CURRENT_TIMESTAMP
        )",
    )
    .execute(&db)
    .await
    .expect("Failed to create cloud_files table");

    sqlx::query(
        "create table if not exists system_stats (
            id integer primary key autoincrement,
            cpu_usage real,
            memory_usage real,
            gpu_usage real,
            net_recv_kbps real,
            net_sent_kbps real,
            disk_usage real,
            disk_read_kbps real,
            disk_write_kbps real,
            created_at timestamp default CURRENT_TIMESTAMP
        )"
    )
    .execute(&db)
    .await
    .expect("Failed to create system_stats table");
    // Indexes for fast listing and lookup
    sqlx::query("create index if not exists idx_cloud_files_user_dir on cloud_files(user_id, dir)")
        .execute(&db)
        .await
        .expect("Failed to create idx_cloud_files_user_dir");
    sqlx::query("create index if not exists idx_cloud_files_user_dir_name on cloud_files(user_id, dir, name)")
        .execute(&db)
        .await
        .expect("Failed to create idx_cloud_files_user_dir_name");
    sqlx::query("create index if not exists idx_cloud_files_checksum on cloud_files(checksum)")
        .execute(&db)
        .await
        .expect("Failed to create idx_cloud_files_checksum");


    // Check if 'gpu_memory_usage' column exists before attempting to add it
    let row_gpu_mem: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM pragma_table_info('system_stats') WHERE name = 'gpu_memory_usage'")
        .fetch_one(&db)
        .await
        .expect("Failed to query pragma_table_info for system_stats table");

    if row_gpu_mem.0 == 0 {
        sqlx::query("ALTER TABLE system_stats ADD COLUMN gpu_memory_usage REAL")
            .execute(&db)
            .await
            .expect("Failed to alter system_stats table to add gpu_memory_usage column");
        println!("system_stats gpu_memory_usage column added");
    }

    sqlx::query(
        "create table if not exists app_permissions (
            id integer primary key autoincrement,
            app_name text not null,
            username text not null,
            created_at timestamp default CURRENT_TIMESTAMP,
            unique(app_name, username)
        )",
    )
    .execute(&db)
    .await
    .expect("Failed to create app_permissions table");
    println!("App permissions table created/verified");

    // Seed default permissions for jellyfin
    let _ = sqlx::query("insert or ignore into app_permissions (app_name, username) values ('jellyfin', 'zac')")
        .execute(&db)
        .await;
    println!("Seeded default app permissions");

    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    
    let app_manager = Arc::new(DockerAppManager::new());
    let auth_service = Arc::new(AuthServiceImpl::new(db.clone(), jwt_secret.clone(), storage_path.clone()));
    let system_service = Arc::new(SystemServiceImpl::new(db.clone()));
    let storage_service = Arc::new(StorageServiceImpl::new(db.clone(), storage_path.clone()));
    let container_service = Arc::new(ContainerServiceImpl::new());
    let agent_service = Arc::new(AgentServiceImpl::new());
    let task_service = Arc::new(TaskServiceImpl::new(db.clone()));

    let torrent_dir = format!("{}/torrents", storage_path);
    let _ = std::fs::create_dir_all(&torrent_dir);
    let mut session_opts = librqbit::SessionOptions::default();
    session_opts.enable_upnp_port_forwarding = true;
    let session: Arc<librqbit::Session> = librqbit::Session::new_with_opts(torrent_dir.into(), session_opts).await.expect("Failed to init torrent session");

    let downloader_service = Arc::new(DownloaderServiceImpl::new(
        db.clone(),
        storage_path.clone(),
        session.clone(),
    ));

    let state = AppState {
        device_codes: &DEVICE_CODES,
        db,
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
    };

    // Helper functions to get internal Arcs from system_service if needed, 
    // or just re-use the ones we passed in.
    // For now, let's simplify and just use the ones we already have.

    // Start background tasks
    watcher::init(state.clone()).await;

    let sys_service = state.system_service.clone();
    tokio::spawn(async move {
        sys_service.run_background_stats_collector().await;
    });

    let storage_service = state.storage_service.clone();
    tokio::spawn(async move {
        storage_service.run_trash_purger().await;
    });

    let port: u16 = std::env::var("PNAS_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8000);
    println!("Backend listening on port {}", port);
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    let static_port: u16 = std::env::var("PNAS_STATIC_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(6000);
    println!("Static listening on port {}", static_port);
    let static_addr: SocketAddr = ([0, 0, 0, 0], static_port).into();
    let static_listener = tokio::net::TcpListener::bind(static_addr).await.unwrap();
    let static_app = routes::static_app();
    let s = axum::serve(static_listener, static_app);
    tokio::spawn(async move {
        let _ = s.await;
    });
    let api = routes::api_app(state.clone());
    axum::serve(listener, api).await.unwrap();
}

/// Read a specific environment variable directly from the .env file
fn read_env_var_from_file(var_name: &str) -> Result<String, std::io::Error> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    
    let file = File::open(".env")?;
    let reader = BufReader::new(file);
    
    for line in reader.lines() {
        let line = line?;
        // Skip comments and empty lines
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        
        // Parse KEY=VALUE format
        if let Some(pos) = line.find('=') {
            let key = line[..pos].trim();
            if key == var_name {
                let value = line[pos + 1..].trim();
                // Remove surrounding quotes if present
                let value = if value.starts_with('"') && value.ends_with('"') && value.len() > 1 {
                    &value[1..value.len() - 1]
                } else if value.starts_with('\'') && value.ends_with('\'') && value.len() > 1 {
                    &value[1..value.len() - 1]
                } else {
                    value
                };
                return Ok(value.to_string());
            }
        }
    }
    
    Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("Variable {} not found in .env file", var_name)))
}

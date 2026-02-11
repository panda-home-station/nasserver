use dotenvy::dotenv;
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;

mod state;
mod models;
mod handlers;
mod middleware;
mod routes;
mod watcher;

use state::{AppState, DEVICE_CODES};

use std::sync::{Arc, Mutex};
use sysinfo::{System, Disks, Networks, Components};
use sqlx::Row;


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

    // Initialize system info components
    // We use System::new() to avoid loading all processes which is slow
    let mut sys = System::new();
    sys.refresh_cpu();
    sys.refresh_memory();
    
    let disks = Disks::new_with_refreshed_list();
    let networks = Networks::new_with_refreshed_list();
    let components = Components::new_with_refreshed_list();

    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    
    let torrent_dir = format!("{}/torrents", storage_path);
    let _ = std::fs::create_dir_all(&torrent_dir);
    
    // Enable listener and UPnP for better connectivity
    let mut session_opts = librqbit::SessionOptions::default();
    session_opts.enable_upnp_port_forwarding = true;
    
    let session = librqbit::Session::new_with_opts(torrent_dir.into(), session_opts).await.expect("Failed to init torrent session");

    let state = AppState {
        device_codes: &DEVICE_CODES,
        db,
        jwt_secret,
        storage_path,
        sys: Arc::new(Mutex::new(sys)),
        disks: Arc::new(Mutex::new(disks)),
        networks: Arc::new(Mutex::new(networks)),
        components: Arc::new(Mutex::new(components)),
        download_tasks: Arc::new(Mutex::new(std::collections::HashMap::new())),
        torrent_session: session,
        magnet_cache: Arc::new(Mutex::new(std::collections::HashMap::new())),
        last_stats: Arc::new(Mutex::new(None)),
    };

    // Start filesystem watcher to sync DB with disk changes
    watcher::init(state.clone()).await;

    let api = routes::api_app(state.clone());

    // Spawn background task to refresh system info
    let state_clone = state.clone();
    tokio::spawn(async move {
        let mut last_record = tokio::time::Instant::now();
        let mut last_disk_read = 0u64;
        let mut last_disk_write = 0u64;
        let mut first_disk_run = true;

        loop {
            // Refresh interval
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            
            let mut cpu_usage = 0.0;
            let mut mem_usage = 0.0;
            let mut mem_used = 0u64;
            let mut mem_total = 0u64;
            let mut net_recv = 0.0;
            let mut net_sent = 0.0;
            let mut disk_read_kbps = 0.0;
            let mut disk_write_kbps = 0.0;

            // 1. CPU & Memory - fast, must be done in-place for CPU usage calc
            {
                if let Ok(mut sys) = state_clone.sys.lock() {
                    sys.refresh_cpu();
                    sys.refresh_memory();
                    cpu_usage = sys.global_cpu_info().cpu_usage() as f64;
                    mem_total = sys.total_memory();
                    mem_used = sys.used_memory();
                    if mem_total > 0 {
                        mem_usage = (mem_used as f64 / mem_total as f64) * 100.0;
                    }
                }
            }

            // 2. Disks - slow (I/O), do it outside lock
            let new_disks = Disks::new_with_refreshed_list();
            let disk_usage = if let Some(d) = new_disks.iter().find(|d| d.mount_point() == std::path::Path::new("/")) {
                let total = d.total_space();
                if total > 0 {
                    ((total - d.available_space()) as f64 / total as f64) * 100.0
                } else { 0.0 }
            } else { 0.0 };

            // Calculate disk R/W speed
            
            // Re-evaluating disk speed collection. If sysinfo doesn't support it easily, 
            // I'll check if I can use a simpler approach or just use 0 for now to keep the UI working.
            // Wait, sysinfo's DiskUsage is per process. 
            // For system-wide, we can read /proc/diskstats on Linux.
            
            if cfg!(target_os = "linux") {
                if let Ok(content) = tokio::fs::read_to_string("/proc/diskstats").await {
                    let mut total_read_sectors = 0u64;
                    let mut total_write_sectors = 0u64;
                    for line in content.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 14 {
                            // Column 6: sectors read, Column 10: sectors written
                            total_read_sectors += parts[5].parse::<u64>().unwrap_or(0);
                            total_write_sectors += parts[9].parse::<u64>().unwrap_or(0);
                        }
                    }
                    let current_read_bytes = total_read_sectors * 512;
                    let current_write_bytes = total_write_sectors * 512;
                    
                    if !first_disk_run {
                        disk_read_kbps = (current_read_bytes.saturating_sub(last_disk_read)) as f64 / 1024.0 / 2.0;
                        disk_write_kbps = (current_write_bytes.saturating_sub(last_disk_write)) as f64 / 1024.0 / 2.0;
                    }
                    last_disk_read = current_read_bytes;
                    last_disk_write = current_write_bytes;
                    first_disk_run = false;
                }
            }

            {
                if let Ok(mut disks) = state_clone.disks.lock() {
                    *disks = new_disks;
                }
            }

            // 3. Networks - can be slow, do it outside lock
            let new_networks = Networks::new_with_refreshed_list();
            for (_name, data) in &new_networks {
                net_recv += data.received() as f64 / 1024.0; // KB
                net_sent += data.transmitted() as f64 / 1024.0; // KB
            }
            {
                if let Ok(mut networks) = state_clone.networks.lock() {
                    *networks = new_networks;
                }
            }

            let gpu_stats = handlers::gpu::get_gpu_usage();

            let stats = crate::models::system::SystemStats {
                cpu_usage,
                memory_usage: mem_usage,
                memory_used: Some(mem_used as i64),
                memory_total: Some(mem_total as i64),
                gpu_usage: gpu_stats.usage,
                gpu_memory_usage: gpu_stats.mem_usage,
                gpu_memory_used: gpu_stats.mem_used,
                gpu_memory_total: gpu_stats.mem_total,
                net_recv_kbps: net_recv,
                net_sent_kbps: net_sent,
                disk_usage,
                disk_read_kbps: Some(disk_read_kbps),
                disk_write_kbps: Some(disk_write_kbps),
                created_at: Some(chrono::Utc::now()),
            };

            if let Ok(mut last) = state_clone.last_stats.lock() {
                *last = Some(stats.clone());
            }

            // Record every 10 seconds
            if last_record.elapsed().as_secs() >= 10 {
                let _ = sqlx::query("insert into system_stats (cpu_usage, memory_usage, gpu_usage, gpu_memory_usage, net_recv_kbps, net_sent_kbps, disk_usage, disk_read_kbps, disk_write_kbps) values ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
                    .bind(stats.cpu_usage)
                    .bind(stats.memory_usage)
                    .bind(stats.gpu_usage)
                    .bind(stats.gpu_memory_usage)
                    .bind(stats.net_recv_kbps)
                    .bind(stats.net_sent_kbps)
                    .bind(stats.disk_usage)
                    .bind(stats.disk_read_kbps)
                    .bind(stats.disk_write_kbps)
                    .execute(&state_clone.db)
                    .await;
                last_record = tokio::time::Instant::now();
            }
        }
    });

    // Spawn background task to purge trash older than 30 days
    {
        let db = state.db.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(24 * 3600)).await;
                let rows = sqlx::query("select id from cloud_files where dir like '/Trash%' and updated_at < datetime('now', '-30 day')")
                    .fetch_all(&db)
                    .await
                    .unwrap_or_default();
                for row in rows {
                    let id: String = row.try_get("id").unwrap_or_default();
                    let _ = sqlx::query("delete from cloud_files where id = $1").bind(&id).execute(&db).await;
                }
            }
        });
    }

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

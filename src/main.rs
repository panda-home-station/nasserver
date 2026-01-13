use dotenvy::dotenv;
use sqlx::sqlite::SqlitePoolOptions;
use std::net::SocketAddr;

mod state;
mod models;
mod handlers;
mod middleware;
mod routes;

use state::{AppState, DEVICE_CODES};

use std::sync::{Arc, Mutex};
use sysinfo::{System, Disks, Networks, Components};


#[tokio::main]
async fn main() {
    dotenv().ok();

    // Read PNAS_DEV_STORAGE_PATH from env or .env file
    let storage_path = std::env::var("PNAS_DEV_STORAGE_PATH")
        .or_else(|_| read_env_var_from_file("PNAS_DEV_STORAGE_PATH"))
        .unwrap_or_else(|_| "./volume".to_string());
    // Ensure the base storage directory exists
    let _ = std::fs::create_dir_all(&storage_path);

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        format!("sqlite:{}/pnas.db", storage_path)
    });
    
    // Try to connect to database directly for better error handling
    let db = match SqlitePoolOptions::new().max_connections(1).connect(&db_url).await {
        Ok(pool) => {
            println!("Database connected successfully: {}", db_url);
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

    // Initialize system info components
    // We use System::new() to avoid loading all processes which is slow
    let mut sys = System::new();
    sys.refresh_cpu();
    sys.refresh_memory();
    
    let disks = Disks::new_with_refreshed_list();
    let networks = Networks::new_with_refreshed_list();
    let components = Components::new_with_refreshed_list();

    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    
    let state = AppState {
        device_codes: &DEVICE_CODES,
        db,
        jwt_secret,
        storage_path,
        sys: Arc::new(Mutex::new(sys)),
        disks: Arc::new(Mutex::new(disks)),
        networks: Arc::new(Mutex::new(networks)),
        components: Arc::new(Mutex::new(components)),
    };

    let api = routes::api_app(state.clone());

    // Spawn background task to refresh system info
    let state_clone = state.clone();
    tokio::spawn(async move {
        loop {
            // Refresh interval
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            
            // 1. CPU & Memory - fast, must be done in-place for CPU usage calc
            {
                if let Ok(mut sys) = state_clone.sys.lock() {
                    sys.refresh_cpu();
                    sys.refresh_memory();
                }
            }

            // 2. Disks - slow (I/O), do it outside lock
            let new_disks = Disks::new_with_refreshed_list();
            {
                if let Ok(mut disks) = state_clone.disks.lock() {
                    *disks = new_disks;
                }
            }

            // 3. Networks - can be slow, do it outside lock
            let new_networks = Networks::new_with_refreshed_list();
            {
                if let Ok(mut networks) = state_clone.networks.lock() {
                    *networks = new_networks;
                }
            }
        }
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

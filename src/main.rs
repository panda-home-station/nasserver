use dotenvy::dotenv;
use sqlx::postgres::PgPoolOptions;
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
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:postgres@postgres:5432/pnas".to_string());
    
    // Use lazy connection to avoid crashing when DB is unavailable in dev/test
    let db = PgPoolOptions::new().max_connections(5).connect_lazy(&db_url).unwrap();
    
    // Init DB schema
    let _ = sqlx::query(
        "create table if not exists users (
            id uuid primary key,
            username text unique,
            email text,
            password_hash text not null,
            created_at timestamptz default now()
        )",
    )
    .execute(&db)
    .await;
    let _ = sqlx::query("alter table users add column if not exists role text not null default 'user'")
        .execute(&db)
        .await;
    let _ = sqlx::query("alter table users add column if not exists username text unique")
        .execute(&db)
        .await;
    let _ = sqlx::query(
        "create table if not exists system_config (
            key text primary key,
            value text
        )",
    )
    .execute(&db)
    .await;

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
        sys: Arc::new(Mutex::new(sys)),
        disks: Arc::new(Mutex::new(disks)),
        networks: Arc::new(Mutex::new(networks)),
        components: Arc::new(Mutex::new(components)),
    };

    let app = routes::app(state.clone());

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
    axum::serve(listener, app).await.unwrap();
}

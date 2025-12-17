use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use chrono::Utc;
use password_hash::{SaltString, PasswordHasher};
use argon2::Argon2;
use uuid::Uuid;
use sysinfo::{System, Disks, Networks, Components};
use local_ip_address::local_ip;

use crate::state::{AppState, START_TIME};
use crate::models::system::{InitStateResp, InitReq, DeviceInfoResp, DiskUsage, HardwareInfo, NetworkInfo, HealthResp, VersionResp};

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub async fn health() -> impl IntoResponse {
    Json(HealthResp {
        status: "ok".to_string(),
        ts: Utc::now().timestamp(),
    })
}

pub async fn version() -> impl IntoResponse {
    Json(VersionResp {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

pub async fn init_state(State(st): State<AppState>) -> impl IntoResponse {
    let cnt: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);
    Json(InitStateResp { initialized: cnt > 0 })
}

pub async fn init_system(State(st): State<AppState>, Json(req): Json<InitReq>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let cnt: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);
    if cnt > 0 {
        return Err((StatusCode::CONFLICT, Json(serde_json::json!({ "error": "already_initialized" }))));
    }
    let salt = SaltString::generate(&mut rand_core::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(req.password.as_bytes(), &salt).unwrap().to_string();
    let uid = Uuid::new_v4();
    let _ = sqlx::query("insert into users (id, username, password_hash, role) values ($1, $2, $3, 'admin')")
        .bind(uid)
        .bind(&req.username)
        .bind(&hash)
        .execute(&st.db)
        .await;

    // Generate device ID (40 chars hex)
    let mut bytes = [0u8; 20];
    for i in 0..20 {
        bytes[i] = fastrand::u8(..);
    }
    let device_id = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();

    let _ = sqlx::query("insert into system_config (key, value) values ('device_name', $1) on conflict (key) do update set value = $1")
        .bind(&req.device_name)
        .execute(&st.db)
        .await;
    let _ = sqlx::query("insert into system_config (key, value) values ('device_id', $1) on conflict (key) do update set value = $1")
        .bind(&device_id)
        .execute(&st.db)
        .await;

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| "/srv/nas".to_string());
    let user_root = std::path::Path::new(&base_root).join("users").join(uid.to_string());
    let _ = std::fs::create_dir_all(&user_root);
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_device_info(State(st): State<AppState>) -> impl IntoResponse {
    let name: String = sqlx::query_scalar("select value from system_config where key = 'device_name'")
        .fetch_optional(&st.db)
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "PNAS-Server".to_string());
    let id: String = sqlx::query_scalar("select value from system_config where key = 'device_id'")
        .fetch_optional(&st.db)
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "Unknown".to_string());
    
    let now = Utc::now();
    let uptime_duration = now.signed_duration_since(*START_TIME);
    let days = uptime_duration.num_days();
    let hours = uptime_duration.num_hours() % 24;
    let minutes = uptime_duration.num_minutes() % 60;
    
    let uptime_str = format!("{}天 {}小时 {}分", days, hours, minutes);
    let time_str = now.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string();
    let time_ts = now.timestamp();

    // Gather real system info
    // We just read from shared state which is updated by background task
    let sys = st.sys.lock().unwrap();
    // We can also refresh components if needed, but temperature usually changes slowly
    let components = st.components.lock().unwrap();
    
    // Disks usually don't change often, but usage does
    let disks = st.disks.lock().unwrap();
    
    // Networks traffic changes constantly
    let networks = st.networks.lock().unwrap();

    // System Disk (/)
    let root_disk = disks.iter().find(|d| d.mount_point() == std::path::Path::new("/"));
    let system_disk = if let Some(d) = root_disk {
        let total = d.total_space();
        let used = total - d.available_space();
        let percent = if total > 0 { ((used as f64 / total as f64) * 100.0) as u8 } else { 0 };
        DiskUsage {
            total: format_bytes(total),
            used: format!("{} ({}%)", format_bytes(used), percent),
            percent,
        }
    } else {
        DiskUsage { total: "N/A".into(), used: "N/A".into(), percent: 0 }
    };

    // Data Disk (Largest non-root or /home or just duplicate root if none)
    // For simplicity, find largest disk that is NOT root, or fallback to root
    let data_d = disks.iter()
        .filter(|d| d.mount_point() != std::path::Path::new("/"))
        .max_by_key(|d| d.total_space());
    
    let data_disk = if let Some(d) = data_d {
        let total = d.total_space();
        let used = total - d.available_space();
        let percent = if total > 0 { ((used as f64 / total as f64) * 100.0) as u8 } else { 0 };
        DiskUsage {
            total: format_bytes(total),
            used: format!("{} ({}%)", format_bytes(used), percent),
            percent,
        }
    } else {
        // Fallback to same as system if no other disk found
        system_disk.clone()
    };
    
    // Hardware
    let cpu_brand = sys.cpus().first().map(|c| c.brand()).unwrap_or("Unknown CPU");
    let cpu_count = sys.cpus().len();
    let total_mem = sys.total_memory();
    // For temp, try to find something with 'core' or 'cpu'
    let temp_val = components.iter()
        .find(|c| c.label().to_lowercase().contains("cpu") || c.label().to_lowercase().contains("core"))
        .map(|c| c.temperature())
        .map(|t| format!("{:.0}°C", t))
        .unwrap_or("N/A".to_string());

    let hardware = HardwareInfo {
        cpu: format!("{} ({}核)", cpu_brand.trim(), cpu_count),
        memory: format!("{} DDR4", format_bytes(total_mem)), // DDR4 is hardcoded guess, sysinfo doesn't give type
        temperature: format!("CPU {}", temp_val),
    };

    // Network
    // Find first non-loopback with IP
    let ip_addr = local_ip().map(|ip| ip.to_string()).unwrap_or("127.0.0.1".to_string());
    let mut total_rx = 0;
    let mut total_tx = 0;
    
    for (name, data) in networks.iter() {
        total_rx += data.total_received();
        total_tx += data.total_transmitted();
    }

    let network = NetworkInfo {
        ip: ip_addr,
        speed: "Unknown".to_string(), // Placeholder, hard to get without specific OS calls
        transfer: format!("↑ {} ↓ {}", format_bytes(total_tx), format_bytes(total_rx)),
    };

    Json(DeviceInfoResp {
        device_name: name,
        device_id: id,
        system_version: format!("PNAS Lite {}", env!("CARGO_PKG_VERSION")),
        system_time: time_str,
        system_time_ts: time_ts,
        uptime: uptime_str,
        system_disk,
        data_disk,
        hardware,
        network,
    })
}

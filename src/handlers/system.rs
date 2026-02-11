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
use local_ip_address::local_ip;

use crate::state::{AppState, START_TIME};
use crate::models::system::{InitStateResp, InitReq, DeviceInfoResp, DiskUsage, HardwareInfo, NetworkInfo, HealthResp, VersionResp, PhysicalDisk, PortCheckReq, PortCheckResp, PortStatus, SystemStats, StatsHistoryQuery};
use crate::handlers::gpu::get_gpu_usage;
use axum::extract::Query;

pub async fn get_current_stats(State(st): State<AppState>) -> impl IntoResponse {
    if let Ok(last) = st.last_stats.lock() {
        if let Some(stats) = &*last {
            return Json(stats.clone());
        }
    }
    
    // Fallback if loop hasn't run yet
    Json(SystemStats {
        cpu_usage: 0.0,
        memory_usage: 0.0,
        gpu_usage: None,
        net_recv_kbps: 0.0,
        net_sent_kbps: 0.0,
        disk_usage: 0.0,
        disk_read_kbps: Some(0.0),
        disk_write_kbps: Some(0.0),
        created_at: Some(Utc::now()),
    })
}

pub async fn get_stats_history(
    State(st): State<AppState>,
    Query(query): Query<StatsHistoryQuery>,
) -> impl IntoResponse {
    let limit = query.limit.unwrap_or(100);
    let start = query.start.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(1));
    let end = query.end.unwrap_or_else(|| Utc::now());

    let stats: Vec<SystemStats> = sqlx::query_as(
        "select cpu_usage, memory_usage, gpu_usage, net_recv_kbps, net_sent_kbps, disk_usage, disk_read_kbps, disk_write_kbps, created_at 
         from system_stats 
         where created_at >= $1 and created_at <= $2 
         order by created_at desc 
         limit $3"
    )
    .bind(start)
    .bind(end)
    .bind(limit as i64)
    .fetch_all(&st.db)
    .await
    .unwrap_or_default();

    Json(stats)
}

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

use ash::{Entry, vk};
use std::fs;

fn get_gpu_info() -> String {
    // Try Vulkan via ash (library API)
    unsafe {
        if let Ok(entry) = Entry::load() {
            // Validate if we can create instance
            let app_info = vk::ApplicationInfo {
                api_version: vk::make_api_version(0, 1, 0, 0),
                ..Default::default()
            };
            
            let create_info = vk::InstanceCreateInfo {
                p_application_info: &app_info,
                ..Default::default()
            };
            
            if let Ok(instance) = entry.create_instance(&create_info, None) {
                if let Ok(devices) = instance.enumerate_physical_devices() {
                    for device in devices {
                        let props = instance.get_physical_device_properties(device);
                        let name = std::ffi::CStr::from_ptr(props.device_name.as_ptr())
                            .to_string_lossy()
                            .into_owned();
                        
                        if !name.to_lowercase().contains("llvmpipe") {
                            return name;
                        }
                    }
                }
            }
        }
    }
    
    // Fallback: simple default if library fails
    "Integrated Graphics".to_string()
}

fn get_physical_disks() -> Vec<PhysicalDisk> {
    let mut disks = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/block") {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            
            // Filter: must not be partition (check if 'partition' file exists)
            if path.join("partition").exists() { continue; }
            // Filter: must be real device (usually has 'device' symlink)
            if !path.join("device").exists() { continue; }
            
            // Read vendor/model
            let vendor = fs::read_to_string(path.join("device/vendor")).unwrap_or_default().trim().to_string();
            let model = fs::read_to_string(path.join("device/model")).unwrap_or_default().trim().to_string();
            
            // If both empty, skip (likely virtual device)
            if vendor.is_empty() && model.is_empty() { continue; }
            
            // Size (in 512-byte sectors)
            let size_sectors = fs::read_to_string(path.join("size"))
                .unwrap_or("0".to_string())
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            let size_bytes = size_sectors * 512;
            let size_str = format_bytes(size_bytes);
            
            // Rotational (0 = SSD, 1 = HDD)
            let is_rotational = fs::read_to_string(path.join("queue/rotational"))
                .unwrap_or("1".to_string())
                .trim() == "1";
                
            // Serial (best effort)
            let serial = fs::read_to_string(path.join("device/serial"))
                .unwrap_or_default()
                .trim()
                .to_string();

            disks.push(PhysicalDisk {
                name,
                model,
                vendor,
                size: size_str,
                serial,
                is_rotational,
            });
        }
    }
    disks
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

pub async fn check_ports(Json(req): Json<PortCheckReq>) -> impl IntoResponse {
    let mut results = Vec::new();
    for port in req.ports {
        let addr_v4 = format!("0.0.0.0:{}", port);
        let addr_v6 = format!("[::]:{}", port);
        
        let mut in_use = false;
        let mut error_msg = None;

        // Try v4
        match std::net::TcpListener::bind(&addr_v4) {
            Ok(l) => {
                drop(l);
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::AddrInUse {
                    in_use = true;
                    error_msg = Some(e.to_string());
                } else {
                    println!("Port check (v4) failed for {}: {}", addr_v4, e);
                    // For non-AddrInUse errors, we don't definitively know if it's in use
                    // But we should probably report the error
                    error_msg = Some(e.to_string());
                }
            }
        }

        // Try v6 if v4 didn't already prove it's in use
        if !in_use {
            match std::net::TcpListener::bind(&addr_v6) {
                Ok(l) => {
                    drop(l);
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::AddrInUse {
                        in_use = true;
                        error_msg = Some(e.to_string());
                    } else if e.kind() != std::io::ErrorKind::AddrNotAvailable {
                        // Skip AddrNotAvailable (e.g. IPv6 disabled)
                        println!("Port check (v6) failed for {}: {}", addr_v6, e);
                        if error_msg.is_none() {
                            error_msg = Some(e.to_string());
                        }
                    }
                }
            }
        }

        results.push(PortStatus { port, in_use, error: error_msg });
    }
    Json(PortCheckResp { results })
}

pub async fn init_state(State(st): State<AppState>) -> impl IntoResponse {
    let cnt: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);
    Json(InitStateResp { initialized: cnt > 0 })
}

pub async fn init_system(State(st): State<AppState>, Json(req): Json<InitReq>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let user_count: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
        })?;
    if user_count > 0 {
        return Err((StatusCode::CONFLICT, Json(serde_json::json!({ "error": "already_initialized" }))));
    }
    let salt = SaltString::generate(&mut rand_core::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(req.password.as_bytes(), &salt).unwrap().to_string();
    let uid = Uuid::new_v4();
    sqlx::query("insert into users (id, username, password_hash, role) values ($1, $2, $3, 'admin')")
        .bind(uid)
        .bind(&req.username)
        .bind(&hash)
        .execute(&st.db)
        .await
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
        })?;

    // Generate device ID (40 chars hex)
    let mut bytes = [0u8; 20];
    for i in 0..20 {
        bytes[i] = fastrand::u8(..);
    }
    let device_id = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();

    sqlx::query("insert or replace into system_config (key, value) values ('device_name', $1)")
        .bind(&req.device_name)
        .execute(&st.db)
        .await
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
        })?;
    sqlx::query("insert or replace into system_config (key, value) values ('device_id', $1)")
        .bind(&device_id)
        .execute(&st.db)
        .await
        .map_err(|e| {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "error": e.to_string() })))
        })?;

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| "/var/panda/nas".to_string());
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

    // GPU detection (Vulkan -> lspci fallback)
    let gpu_info = get_gpu_info();

    let hardware = HardwareInfo {
        cpu: format!("{} ({}核)", cpu_brand.trim(), cpu_count),
        gpu: gpu_info,
        memory: format!("{} DDR4", format_bytes(total_mem)), // DDR4 is hardcoded guess, sysinfo doesn't give type
        temperature: format!("CPU {}", temp_val),
    };

    // Network
    // Find first non-loopback with IP
    let ip_addr = local_ip().map(|ip| ip.to_string()).unwrap_or("127.0.0.1".to_string());
    let mut total_rx = 0;
    let mut total_tx = 0;
    
    for (_name, data) in networks.iter() {
        total_rx += data.total_received();
        total_tx += data.total_transmitted();
    }

    let network = NetworkInfo {
        ip: ip_addr,
        speed: "Unknown".to_string(), // Placeholder, hard to get without specific OS calls
        transfer: format!("↑ {} ↓ {}", format_bytes(total_tx), format_bytes(total_rx)),
    };

    // Physical Disks (via sysfs)
    let phy_disks = get_physical_disks();

    Json(DeviceInfoResp {
        device_name: name,
        device_id: id,
        system_version: format!("PNAS Lite {}", env!("CARGO_PKG_VERSION")),
        system_time: time_str,
        system_time_ts: time_ts,
        uptime: uptime_str,
        system_disk,
        data_disk,
        phy_disks,
        hardware,
        network,
    })
}

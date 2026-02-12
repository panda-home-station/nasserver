use crate::SystemService;
use domain::entities::system::{
    SystemStats, StatsHistoryQuery, DeviceInfo, HardwareInfo, NetworkInfo, 
    DiskUsage, PhysicalDisk, InitReq, PortStatus
};
use domain::{Result, Error as DomainError};
use async_trait::async_trait;
use sqlx::{Pool, Sqlite};
use sysinfo::{System, Disks, Networks, Components};
use std::sync::{Arc, Mutex};
use chrono::{Utc, DateTime};
use password_hash::{SaltString, PasswordHasher};
use argon2::Argon2;
use uuid::Uuid;
use local_ip_address::local_ip;
use std::fs;
use ash::{Entry, vk};

pub struct SystemServiceImpl {
    db: Pool<Sqlite>,
    pub start_time: DateTime<Utc>,
    sys: Arc<Mutex<System>>,
    disks: Arc<Mutex<Disks>>,
    networks: Arc<Mutex<Networks>>,
    components: Arc<Mutex<Components>>,
    last_stats: Arc<Mutex<Option<SystemStats>>>,
}

impl SystemServiceImpl {
    pub fn new(db: Pool<Sqlite>, start_time: DateTime<Utc>) -> Self {
        let mut sys = System::new();
        sys.refresh_cpu();
        sys.refresh_memory();

        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();
        let components = Components::new_with_refreshed_list();

        Self {
            db,
            start_time,
            sys: Arc::new(Mutex::new(sys)),
            disks: Arc::new(Mutex::new(disks)),
            networks: Arc::new(Mutex::new(networks)),
            components: Arc::new(Mutex::new(components)),
            last_stats: Arc::new(Mutex::new(None)),
        }
    }
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

fn get_gpu_info() -> String {
    unsafe {
        if let Ok(entry) = Entry::load() {
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
    "Integrated Graphics".to_string()
}

fn get_physical_disks() -> Vec<PhysicalDisk> {
    let mut disks = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/block") {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            if path.join("partition").exists() { continue; }
            if !path.join("device").exists() { continue; }
            let vendor = fs::read_to_string(path.join("device/vendor")).unwrap_or_default().trim().to_string();
            let model = fs::read_to_string(path.join("device/model")).unwrap_or_default().trim().to_string();
            if vendor.is_empty() && model.is_empty() { continue; }
            let size_sectors = fs::read_to_string(path.join("size"))
                .unwrap_or("0".to_string())
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            let size_bytes = size_sectors * 512;
            let size_str = format_bytes(size_bytes);
            let is_rotational = fs::read_to_string(path.join("queue/rotational"))
                .unwrap_or("1".to_string())
                .trim() == "1";
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

#[async_trait]
impl SystemService for SystemServiceImpl {
    async fn get_current_stats(&self) -> Result<SystemStats> {
        if let Ok(last) = self.last_stats.lock() {
            if let Some(stats) = &*last {
                return Ok(stats.clone());
            }
        }

        let mut cpu_usage = 0.0;
        let mut mem_usage = 0.0;
        let mut mem_used = 0;
        let mut mem_total = 0;
        let mut net_recv = 0.0;
        let mut net_sent = 0.0;
        let mut disk_usage = 0.0;

        {
            if let Ok(mut sys) = self.sys.lock() {
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

        {
            if let Ok(networks) = self.networks.lock() {
                for (_name, data) in &*networks {
                    net_recv += data.received() as f64 / 1024.0;
                    net_sent += data.transmitted() as f64 / 1024.0;
                }
            }
        }

        {
            if let Ok(disks) = self.disks.lock() {
                if let Some(d) = disks.iter().find(|d| d.mount_point() == std::path::Path::new("/")) {
                    let total = d.total_space();
                    if total > 0 {
                        disk_usage = ((total - d.available_space()) as f64 / total as f64) * 100.0;
                    }
                }
            }
        }

        let gpu_stats = gpu::get_gpu_usage();

        Ok(SystemStats {
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
            disk_read_kbps: None,
            disk_write_kbps: None,
            created_at: Some(Utc::now()),
        })
    }

    async fn get_stats_history(&self, query: StatsHistoryQuery) -> Result<Vec<SystemStats>> {
        let limit = query.limit.unwrap_or(100);
        let start = query.start.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(1));
        let end = query.end.unwrap_or_else(|| Utc::now());

        let stats: Vec<SystemStats> = sqlx::query_as(
            "select cpu_usage, memory_usage, NULL as memory_used, NULL as memory_total, gpu_usage, gpu_memory_usage, NULL as gpu_memory_used, NULL as gpu_memory_total, net_recv_kbps, net_sent_kbps, disk_usage, disk_read_kbps, disk_write_kbps, created_at 
             from system_stats 
             where created_at >= $1 and created_at <= $2 
             order by created_at desc 
             limit $3"
        )
        .bind(start)
        .bind(end)
        .bind(limit as i64)
        .fetch_all(&self.db)
        .await?;

        Ok(stats)
    }

    async fn health(&self) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "status": "ok",
            "ts": Utc::now().timestamp(),
        }))
    }

    async fn is_initialized(&self) -> Result<bool> {
        let cnt: i64 = sqlx::query_scalar("select count(*) from users")
            .fetch_one(&self.db)
            .await?;
        Ok(cnt > 0)
    }

    async fn init_system(&self, req: InitReq) -> Result<()> {
        if self.is_initialized().await? {
            return Err(DomainError::Internal("already_initialized".to_string()));
        }

        let salt = SaltString::generate(&mut rand_core::OsRng);
        let argon2 = Argon2::default();
        let hash = argon2.hash_password(req.password.as_bytes(), &salt)
            .map_err(|e| DomainError::Internal(e.to_string()))?
            .to_string();
        let uid = Uuid::new_v4();
        
        sqlx::query("insert into users (id, username, password_hash, role) values ($1, $2, $3, 'admin')")
            .bind(uid)
            .bind(&req.username)
            .bind(&hash)
            .execute(&self.db)
            .await?;

        let mut bytes = [0u8; 20];
        for i in 0..20 {
            bytes[i] = fastrand::u8(..);
        }
        let device_id = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();

        sqlx::query("insert or replace into system_config (key, value) values ('device_name', $1)")
            .bind(&req.device_name)
            .execute(&self.db)
            .await?;
        sqlx::query("insert or replace into system_config (key, value) values ('device_id', $1)")
            .bind(&device_id)
            .execute(&self.db)
            .await?;

        let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| "/var/panda/nas".to_string());
        let user_root = std::path::Path::new(&base_root).join("users").join(uid.to_string());
        let _ = std::fs::create_dir_all(&user_root);
        
        Ok(())
    }

    async fn get_device_info(&self) -> Result<DeviceInfo> {
        let name: String = sqlx::query_scalar("select value from system_config where key = 'device_name'")
            .fetch_optional(&self.db)
            .await?
            .unwrap_or_else(|| "PNAS-Server".to_string());
        let id: String = sqlx::query_scalar("select value from system_config where key = 'device_id'")
            .fetch_optional(&self.db)
            .await?
            .unwrap_or_else(|| "Unknown".to_string());
        
        let now = Utc::now();
        let uptime_duration = now.signed_duration_since(self.start_time);
        let days = uptime_duration.num_days();
        let hours = uptime_duration.num_hours() % 24;
        let minutes = uptime_duration.num_minutes() % 60;
        
        let uptime_str = format!("{}天 {}小时 {}分", days, hours, minutes);
        let time_str = now.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string();
        let time_ts = now.timestamp();

        let (hardware, network, system_disk) = {
            let sys = self.sys.lock().map_err(|_| DomainError::Internal("mutex lock failed".to_string()))?;
            let components = self.components.lock().map_err(|_| DomainError::Internal("mutex lock failed".to_string()))?;
            let disks = self.disks.lock().map_err(|_| DomainError::Internal("mutex lock failed".to_string()))?;
            let networks = self.networks.lock().map_err(|_| DomainError::Internal("mutex lock failed".to_string()))?;

            let cpu_model = sys.global_cpu_info().brand().to_string();
            let cpu_cores = sys.cpus().len() as u32;
            let mem_total = sys.total_memory();
            let gpu_model = get_gpu_info();
            let mut cpu_temp = 0.0;
            for component in components.iter() {
                if component.label().contains("CPU") || component.label().contains("Package") {
                    cpu_temp = component.temperature();
                    break;
                }
            }

            let hw = HardwareInfo {
                cpu: format!("{} ({}核)", cpu_model, cpu_cores),
                memory: format_bytes(mem_total),
                gpu: gpu_model,
                temperature: format!("{:.1}°C", cpu_temp),
            };

            let ip = local_ip().map(|i| i.to_string()).unwrap_or_else(|_| "Unknown".to_string());
            let mut traffic_in = 0;
            let mut traffic_out = 0;
            for (_name, data) in &*networks {
                traffic_in += data.total_received();
                traffic_out += data.total_transmitted();
            }

            let nw = NetworkInfo {
                ip,
                speed: format!("↓{} ↑{}", format_bytes(traffic_in), format_bytes(traffic_out)),
                transfer: format!("总计: {}", format_bytes(traffic_in + traffic_out)),
            };

            let root_disk = disks.iter().find(|d| d.mount_point() == std::path::Path::new("/"));
            let sd = if let Some(d) = root_disk {
                let total = d.total_space();
                let used = total - d.available_space();
                let percent = if total > 0 { ((used as f64 / total as f64) * 100.0) as u8 } else { 0 };
                DiskUsage {
                    total: format_bytes(total),
                    used: format!("{} ({}%)", format_bytes(used), percent),
                    percent,
                }
            } else {
                DiskUsage { total: "0 B".to_string(), used: "0 B (0%)".to_string(), percent: 0 }
            };

            (hw, nw, sd)
        };

        let physical_disks = get_physical_disks();

        Ok(DeviceInfo {
            device_name: name,
            device_id: id,
            system_version: "0.0.1".to_string(),
            system_time: time_str,
            system_time_ts: time_ts,
            uptime: uptime_str,
            system_disk: system_disk.clone(),
            data_disk: system_disk,
            phy_disks: physical_disks,
            hardware,
            network,
        })
    }

    async fn check_ports(&self, ports: Vec<u16>) -> Result<Vec<PortStatus>> {
        let mut results = Vec::new();
        for port in ports {
            let addr_v4 = format!("0.0.0.0:{}", port);
            let addr_v6 = format!("[::]:{}", port);
            
            let mut in_use = false;
            let mut error_msg = None;

            match std::net::TcpListener::bind(&addr_v4) {
                Ok(l) => drop(l),
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::AddrInUse {
                        in_use = true;
                        error_msg = Some(e.to_string());
                    } else {
                        error_msg = Some(e.to_string());
                    }
                }
            }

            if !in_use {
                match std::net::TcpListener::bind(&addr_v6) {
                    Ok(l) => drop(l),
                    Err(e) => {
                        if e.kind() == std::io::ErrorKind::AddrInUse {
                            in_use = true;
                            error_msg = Some(e.to_string());
                        } else if e.kind() != std::io::ErrorKind::AddrNotAvailable {
                            if error_msg.is_none() {
                                error_msg = Some(e.to_string());
                            }
                        }
                    }
                }
            }

            results.push(PortStatus {
                port,
                in_use,
                error: error_msg,
            });
        }
        Ok(results)
    }

    async fn get_gpus(&self) -> Vec<gpu::GpuInfo> {
        gpu::get_system_gpus()
    }

    async fn get_docker_mirrors(&self) -> Result<Vec<serde_json::Value>> {
        let list_json: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirrors'")
            .fetch_optional(&self.db)
            .await?;
        
        let mirrors: Vec<serde_json::Value> = list_json
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        
        Ok(mirrors)
    }

    async fn set_docker_mirrors(&self, mirrors: Vec<serde_json::Value>) -> Result<()> {
        let json = serde_json::to_string(&mirrors)
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        sqlx::query("insert or replace into system_config (key, value) values ('docker_mirrors', $1)")
            .bind(json)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn get_docker_settings(&self) -> Result<serde_json::Value> {
        let v: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirror'")
            .fetch_optional(&self.db)
            .await?;
        let settings: serde_json::Value = v
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({ "mode": "none", "host": null }));
        Ok(settings)
    }

    async fn set_docker_settings(&self, settings: serde_json::Value) -> Result<()> {
        let json = serde_json::to_string(&settings)
            .map_err(|e| DomainError::Internal(e.to_string()))?;
        sqlx::query("insert or replace into system_config (key, value) values ('docker_mirror', $1)")
            .bind(json)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    async fn run_background_stats_collector(&self) {
        let mut last_record = tokio::time::Instant::now();
        let mut last_disk_read = 0u64;
        let mut last_disk_write = 0u64;
        let mut first_disk_run = true;

        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
            
            let mut cpu_usage = 0.0;
            let mut mem_usage = 0.0;
            let mut mem_used = 0u64;
            let mut mem_total = 0u64;
            let mut net_recv = 0.0;
            let mut net_sent = 0.0;
            let mut disk_read_kbps = 0.0;
            let mut disk_write_kbps = 0.0;

            {
                if let Ok(mut sys) = self.sys.lock() {
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

            let new_disks = Disks::new_with_refreshed_list();
            let disk_usage_pct = if let Some(d) = new_disks.iter().find(|d| d.mount_point() == std::path::Path::new("/")) {
                let total = d.total_space();
                if total > 0 {
                    ((total - d.available_space()) as f64 / total as f64) * 100.0
                } else { 0.0 }
            } else { 0.0 };

            if cfg!(target_os = "linux") {
                if let Ok(content) = tokio::fs::read_to_string("/proc/diskstats").await {
                    let mut total_read_sectors = 0u64;
                    let mut total_write_sectors = 0u64;
                    for line in content.lines() {
                        let parts: Vec<&str> = line.split_whitespace().collect();
                        if parts.len() >= 14 {
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
                if let Ok(mut networks) = self.networks.lock() {
                    networks.refresh_list();
                    networks.refresh();
                    for (_name, data) in &*networks {
                        net_recv += data.received() as f64 / 1024.0 / 2.0;
                        net_sent += data.transmitted() as f64 / 1024.0 / 2.0;
                    }
                }
            }

            let gpu_stats = gpu::get_gpu_usage();

            let stats = SystemStats {
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
                disk_usage: disk_usage_pct,
                disk_read_kbps: Some(disk_read_kbps),
                disk_write_kbps: Some(disk_write_kbps),
                created_at: Some(Utc::now()),
            };

            if let Ok(mut last) = self.last_stats.lock() {
                *last = Some(stats.clone());
            }

            if last_record.elapsed() >= tokio::time::Duration::from_secs(60) {
                let _ = sqlx::query(
                    "insert into system_stats (cpu_usage, memory_usage, gpu_usage, gpu_memory_usage, net_recv_kbps, net_sent_kbps, disk_usage, disk_read_kbps, disk_write_kbps, created_at) 
                     values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)"
                )
                .bind(stats.cpu_usage)
                .bind(stats.memory_usage)
                .bind(stats.gpu_usage)
                .bind(stats.gpu_memory_usage)
                .bind(stats.net_recv_kbps)
                .bind(stats.net_sent_kbps)
                .bind(stats.disk_usage)
                .bind(stats.disk_read_kbps)
                .bind(stats.disk_write_kbps)
                .bind(stats.created_at)
                .execute(&self.db)
                .await;
                
                last_record = tokio::time::Instant::now();
            }
        }
    }
}

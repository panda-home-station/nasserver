use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct InitStateResp {
    pub initialized: bool,
}

#[derive(Deserialize)]
pub struct InitReq {
    pub username: String,
    pub password: String,
    pub device_name: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiskUsage {
    pub total: String,
    pub used: String,
    pub percent: u8,
}

#[derive(Serialize, Clone, Debug)]
pub struct PhysicalDisk {
    pub name: String,
    pub model: String,
    pub size: String,
    pub serial: String,
    pub vendor: String,
    pub is_rotational: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct DeviceInfoResp {
    pub device_name: String,
    pub device_id: String,
    pub system_version: String,
    pub system_time: String,
    pub system_time_ts: i64,
    pub uptime: String,
    pub system_disk: DiskUsage,
    pub data_disk: DiskUsage,
    pub phy_disks: Vec<PhysicalDisk>,
    pub hardware: HardwareInfo,
    pub network: NetworkInfo,
}

#[derive(Serialize, Clone, Debug)]
pub struct HardwareInfo {
    pub cpu: String,
    pub gpu: String,
    pub memory: String,
    pub temperature: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct NetworkInfo {
    pub ip: String,
    pub speed: String,
    pub transfer: String,
}

#[derive(Serialize)]
pub struct HealthResp {
    pub status: String,
    pub ts: i64,
}

#[derive(Serialize)]
pub struct VersionResp {
    pub version: String,
}

#[derive(Deserialize)]
pub struct PortCheckReq {
    pub ports: Vec<u16>,
}

#[derive(Serialize)]
pub struct PortCheckResp {
    pub results: Vec<PortStatus>,
}

#[derive(Serialize)]
pub struct PortStatus {
    pub port: u16,
    pub in_use: bool,
    pub error: Option<String>,
}

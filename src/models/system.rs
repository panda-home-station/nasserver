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
pub struct DeviceInfoResp {
    pub device_name: String,
    pub device_id: String,
    pub system_version: String,
    pub system_time: String,
    pub system_time_ts: i64,
    pub uptime: String,
    pub system_disk: DiskUsage,
    pub data_disk: DiskUsage,
    pub hardware: HardwareInfo,
    pub network: NetworkInfo,
}

#[derive(Serialize, Clone, Debug)]
pub struct HardwareInfo {
    pub cpu: String,
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

use async_trait::async_trait;
use crate::Result;
pub use crate::entities::system::{
    SystemStats, DeviceInfo, DiskUsage, PhysicalDisk, HardwareInfo, NetworkInfo,
    StatsHistoryQuery, InitReq, PortStatus, InitStateResp, VersionResp, PortCheckReq, PortCheckResp
};
// Remove models import
use gpu::GpuInfo;

#[async_trait]
pub trait SystemService: Send + Sync {
    async fn get_current_stats(&self) -> Result<SystemStats>;
    async fn get_stats_history(&self, query: StatsHistoryQuery) -> Result<Vec<SystemStats>>;
    async fn health(&self) -> Result<serde_json::Value>;
    async fn is_initialized(&self) -> Result<bool>;
    async fn init_system(&self, req: InitReq) -> Result<()>;
    async fn get_device_info(&self) -> Result<DeviceInfo>;
    async fn check_ports(&self, ports: Vec<u16>) -> Result<Vec<PortStatus>>;
    async fn get_gpus(&self) -> Vec<GpuInfo>;
    async fn get_docker_mirrors(&self) -> Result<Vec<serde_json::Value>>;
    async fn set_docker_mirrors(&self, mirrors: Vec<serde_json::Value>) -> Result<()>;
    async fn get_docker_settings(&self) -> Result<serde_json::Value>;
    async fn set_docker_settings(&self, settings: serde_json::Value) -> Result<()>;
    async fn run_background_stats_collector(&self);
}

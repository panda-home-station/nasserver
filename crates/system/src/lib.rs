use async_trait::async_trait;
pub use common::core::Result;
pub use models::system::{DeviceInfoResp, SystemStats, StatsHistoryQuery, InitReq, PortStatus};
pub use gpu::GpuInfo;
use serde_json;

#[async_trait]
pub trait SystemService: Send + Sync {
    async fn get_current_stats(&self) -> Result<SystemStats>;
    async fn get_stats_history(&self, query: StatsHistoryQuery) -> Result<Vec<SystemStats>>;
    async fn health(&self) -> Result<serde_json::Value>;
    async fn is_initialized(&self) -> Result<bool>;
    async fn init_system(&self, req: InitReq) -> Result<()>;
    async fn get_device_info(&self) -> Result<DeviceInfoResp>;
    async fn check_ports(&self, ports: Vec<u16>) -> Result<Vec<PortStatus>>;
    async fn get_gpus(&self) -> Vec<GpuInfo>;
    async fn run_background_stats_collector(&self);
}

pub mod system_service;
pub use system_service::SystemServiceImpl;

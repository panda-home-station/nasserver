use async_trait::async_trait;
use crate::Result;
pub use crate::entities::downloader::{DownloadTask, TorrentFileMetadata};
pub use crate::dtos::downloader::{
    CreateDownloadReq, ControlDownloadReq, ResolveMagnetReq, StartMagnetDownloadReq,
    ResolveMagnetResp, DownloadTaskResp, SubTaskResp
};
// Remove models import
use serde::Serialize;

#[derive(Serialize)]
pub struct DownloadStats {
    pub total_tasks: usize,
    pub active_tasks: usize,
    pub download_speed: i64,
}

#[async_trait]
pub trait DownloaderService: Send + Sync {
    async fn list_tasks(&self) -> Result<Vec<DownloadTaskResp>>;
    async fn create_task(&self, username: &str, req: CreateDownloadReq) -> Result<()>;
    async fn control_task(&self, id: &str, req: ControlDownloadReq) -> Result<()>;
    async fn resolve_magnet(&self, req: ResolveMagnetReq) -> Result<ResolveMagnetResp>;
    async fn start_magnet_download(&self, username: &str, req: StartMagnetDownloadReq) -> Result<()>;
    async fn get_stats(&self) -> Result<DownloadStats>;
}

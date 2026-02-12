use async_trait::async_trait;
use common::core::Result;
use models::downloader::{DownloadTask, CreateDownloadReq, ResolveMagnetReq, ResolveMagnetResp, StartMagnetDownloadReq};
use serde::Serialize;

#[derive(Serialize)]
pub struct SubTaskResp {
    pub filename: String,
    pub progress: f64,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed: u64,
    pub status: String,
}

#[derive(Serialize)]
pub struct DownloadTaskResp {
    #[serde(flatten)]
    pub task: DownloadTask,
    pub virtual_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_tasks: Option<Vec<SubTaskResp>>,
}

#[async_trait]
pub trait DownloaderService: Send + Sync {
    async fn list_downloads(&self) -> Result<Vec<DownloadTaskResp>>;
    async fn create_download(&self, username: &str, req: CreateDownloadReq) -> Result<()>;
    async fn pause_download(&self, id: &str) -> Result<()>;
    async fn resume_download(&self, id: &str) -> Result<()>;
    async fn delete_download(&self, id: &str) -> Result<()>;
    async fn resolve_magnet(&self, req: ResolveMagnetReq) -> Result<ResolveMagnetResp>;
    async fn start_magnet_download(&self, username: &str, req: StartMagnetDownloadReq) -> Result<()>;
}

pub mod downloader_service;
pub use downloader_service::DownloaderServiceImpl;

use serde::{Deserialize, Serialize};
use crate::entities::downloader::{DownloadTask, TorrentFileMetadata};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMagnetResp {
    pub token: String,
    pub files: Vec<TorrentFileMetadata>,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTaskResp {
    pub filename: String,
    pub progress: f64,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed: i64,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTaskResp {
    pub task: DownloadTask,
    pub virtual_path: String,
    pub sub_tasks: Option<Vec<SubTaskResp>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CreateDownloadReq {
    pub url: String,
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ControlDownloadReq {
    pub action: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ResolveMagnetReq {
    pub magnet_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StartMagnetDownloadReq {
    pub token: String,
    pub files: Vec<usize>,
    pub path: Option<String>,
}

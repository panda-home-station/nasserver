use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct DownloadTask {
    pub id: String,
    pub url: String,
    pub path: String,
    pub filename: String,
    pub status: String, // pending, downloading, paused, done, error
    pub progress: f64,
    pub total_bytes: i64,
    pub downloaded_bytes: i64,
    pub speed: i64,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
    pub error_msg: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentFileMetadata {
    pub index: usize,
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolveMagnetResp {
    pub token: String, // info_hash or unique id to retrieve cached metadata
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
    pub action: String, // pause, resume, delete
}

#[derive(Debug, Deserialize, Clone)]
pub struct ResolveMagnetReq {
    pub magnet_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct StartMagnetDownloadReq {
    pub token: String,
    pub files: Vec<usize>, // indices of files to download
    pub path: Option<String>,
}

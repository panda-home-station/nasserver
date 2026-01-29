use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
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

#[derive(Debug, Deserialize)]
pub struct CreateDownloadReq {
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct ControlDownloadReq {
    pub action: String, // pause, resume, delete
}

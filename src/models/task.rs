use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct FileTask {
    pub id: String,
    #[serde(rename = "type")]
    #[sqlx(rename = "type")]
    pub task_type: String,
    pub name: String,
    pub dir: Option<String>,
    pub progress: i32,
    pub status: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTaskReq {
    pub id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub name: String,
    pub dir: Option<String>,
    pub progress: i32,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTaskReq {
    pub progress: Option<i32>,
    pub status: Option<String>,
}

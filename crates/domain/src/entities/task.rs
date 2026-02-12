use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct FileTask {
    pub id: String,
    #[serde(rename = "type")]
    pub task_type: String,
    pub name: String,
    pub dir: Option<String>,
    pub progress: i32,
    pub status: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub updated_at: Option<chrono::DateTime<chrono::Utc>>,
}

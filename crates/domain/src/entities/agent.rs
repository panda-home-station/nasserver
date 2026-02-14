use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskStep {
    pub id: String,
    pub description: String,
    pub status: String, // pending, running, completed, failed
    pub result: Option<String>,
    pub tool_calls: Vec<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AgentTask {
    pub id: String,
    pub query: String,
    pub status: String, // processing, completed, failed
    pub plan: Vec<TaskStep>,
    pub logs: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ChatSession {
    pub id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub agent_id: String,
    pub title: String,
    pub last_message: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
pub struct ChatMessage {
    pub id: uuid::Uuid,
    pub session_id: uuid::Uuid,
    pub role: String,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

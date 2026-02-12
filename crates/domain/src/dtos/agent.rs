use serde::{Deserialize, Serialize};
use crate::entities::agent::TaskStep;

#[derive(Deserialize, Debug, Clone)]
pub struct TaskRequest {
    pub query: String,
    pub config: Option<serde_json::Value>,
}

#[derive(Serialize, Debug, Clone)]
pub struct TaskResponse {
    pub task_id: String,
    pub status: String,
    pub plan: Vec<TaskStep>,
    pub logs: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct ChatResponse {
    pub reply: String,
}

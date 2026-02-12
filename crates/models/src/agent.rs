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

#[derive(Deserialize, Debug)]
pub struct TaskRequest {
    pub query: String,
    #[allow(dead_code)]
    pub config: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct TaskResponse {
    pub task_id: String,
    pub status: String,
    pub plan: Vec<TaskStep>,
    pub logs: Vec<String>,
}

#[derive(Deserialize, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Debug)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
    pub endpoint: Option<String>,
}

#[derive(Serialize)]
#[allow(dead_code)]
pub struct ChatResponse {
    pub reply: String,
}

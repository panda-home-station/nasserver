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

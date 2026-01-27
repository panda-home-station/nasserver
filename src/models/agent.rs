use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Debug)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub model: Option<String>,
}

#[derive(Serialize)]
pub struct ChatResponse {
    pub reply: String,
}

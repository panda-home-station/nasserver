use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::process::Command;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    ProviderError(String),
    #[error("Tool execution error: {0}")]
    ToolError(String),
    #[error("Sandbox error: {0}")]
    SandboxError(String),
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Unknown error: {0}")]
    Unknown(String),
}

pub type Result<T> = std::result::Result<T, AgentError>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Role {
    User,
    Assistant,
    System,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>, // For Tool role messages
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String, // JSON string
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value, // JSON Schema
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
}

#[derive(Debug, Clone, Serialize)]
pub enum AgentEvent {
    Thought(String),
    ToolCall(ToolCall),
    ToolResult { id: String, result: String },
    Answer(String),
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub model: Option<String>,
    pub endpoint: Option<String>,
}

#[async_trait]
pub trait Agent: Send + Sync {
    async fn chat(&self, session_id: &str, input: &str, history: Vec<Message>, config: Option<AgentConfig>) -> Result<BoxStream<'static, Result<AgentEvent>>>;
}

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(&self, messages: &[Message], tools: &[ToolDefinition], config: Option<&AgentConfig>) -> Result<CompletionResponse>;
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: Value, sandbox: &dyn Sandbox) -> Result<String>;
}

pub trait Sandbox: Send + Sync {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()>;
}

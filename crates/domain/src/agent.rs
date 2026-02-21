use async_trait::async_trait;
use crate::Result;
pub use crate::entities::agent::{AgentTask, TaskStep, ChatSession, ChatMessage as ChatMessageEntity};
pub use crate::dtos::agent::{TaskRequest, TaskResponse, ChatRequest, ChatResponse, ChatMessage};
// Remove models import
use axum::response::sse::Event;
use futures_util::stream::BoxStream;
use uuid::Uuid;

#[async_trait]
pub trait AgentService: Send + Sync {
    async fn create_task(&self, req: TaskRequest) -> Result<TaskResponse>;
    async fn get_task(&self, task_id: &str) -> Result<TaskResponse>;
    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<Event>>>;
    async fn search(&self, query: &str) -> Result<serde_json::Value>;

    // Chat history management
    async fn list_sessions(&self, user_id: Uuid) -> Result<Vec<ChatSession>>;
    async fn get_session_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessageEntity>>;
    async fn create_session(&self, user_id: Uuid, agent_id: String, title: String) -> Result<ChatSession>;
    async fn save_message(&self, session_id: Uuid, role: String, content: String, tool_calls: Option<serde_json::Value>) -> Result<ChatMessageEntity>;
    async fn delete_session(&self, session_id: Uuid) -> Result<()>;
    async fn execute_command(&self, command: String) -> Result<serde_json::Value>;
}

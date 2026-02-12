use async_trait::async_trait;
use crate::Result;
pub use crate::entities::agent::{AgentTask, TaskRequest, TaskResponse, ChatRequest, ChatResponse, ChatMessage, TaskStep};
// Remove models import
use axum::response::sse::Event;
use futures_util::stream::BoxStream;

#[async_trait]
pub trait AgentService: Send + Sync {
    async fn create_task(&self, req: TaskRequest) -> Result<TaskResponse>;
    async fn get_task(&self, task_id: &str) -> Result<TaskResponse>;
    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<Event>>>;
}

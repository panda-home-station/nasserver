use async_trait::async_trait;
use common::core::Result;
use models::agent::{TaskRequest, TaskResponse, ChatRequest};
use axum::response::sse::Event;
use futures_util::stream::BoxStream;

#[async_trait]
pub trait AgentService: Send + Sync {
    async fn create_task(&self, req: TaskRequest) -> Result<TaskResponse>;
    async fn get_task(&self, task_id: &str) -> Result<TaskResponse>;
    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<Event>>>;
}

pub mod agent_service;
pub use agent_service::AgentServiceImpl;

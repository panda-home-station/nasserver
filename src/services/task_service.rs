use async_trait::async_trait;
use crate::core::Result;
use crate::models::task::{FileTask, CreateTaskReq, UpdateTaskReq};

#[async_trait]
pub trait TaskService: Send + Sync {
    async fn list_tasks(&self) -> Result<Vec<FileTask>>;
    async fn create_task(&self, req: CreateTaskReq) -> Result<()>;
    async fn update_task(&self, id: String, req: UpdateTaskReq) -> Result<()>;
    async fn clear_completed_tasks(&self) -> Result<()>;
}

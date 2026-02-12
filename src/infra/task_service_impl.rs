use async_trait::async_trait;
use sqlx::{Pool, Sqlite};
use crate::services::TaskService;
use crate::models::task::{FileTask, CreateTaskReq, UpdateTaskReq};
use crate::core::Result;

pub struct TaskServiceImpl {
    db: Pool<Sqlite>,
}

impl TaskServiceImpl {
    pub fn new(db: Pool<Sqlite>) -> Self {
        Self { db }
    }
}

#[async_trait]
impl TaskService for TaskServiceImpl {
    async fn list_tasks(&self) -> Result<Vec<FileTask>> {
        let tasks = sqlx::query_as::<_, FileTask>("select * from file_tasks order by created_at asc")
            .fetch_all(&self.db)
            .await?;
        Ok(tasks)
    }

    async fn create_task(&self, req: CreateTaskReq) -> Result<()> {
        sqlx::query(
            "insert into file_tasks (id, type, name, dir, progress, status) values ($1, $2, $3, $4, $5, $6)"
        )
        .bind(&req.id)
        .bind(&req.task_type)
        .bind(&req.name)
        .bind(&req.dir)
        .bind(req.progress)
        .bind(&req.status)
        .execute(&self.db)
        .await?;
        Ok(())
    }

    async fn update_task(&self, id: String, req: UpdateTaskReq) -> Result<()> {
        if let Some(p) = req.progress {
            sqlx::query("update file_tasks set progress = $1, updated_at = datetime('now') where id = $2")
                .bind(p)
                .bind(&id)
                .execute(&self.db)
                .await?;
        }
        
        if let Some(s) = &req.status {
            sqlx::query("update file_tasks set status = $1, updated_at = datetime('now') where id = $2")
                .bind(s)
                .bind(&id)
                .execute(&self.db)
                .await?;
        }
        
        Ok(())
    }

    async fn clear_completed_tasks(&self) -> Result<()> {
        sqlx::query("delete from file_tasks where status in ('done', 'error')")
            .execute(&self.db)
            .await?;
        Ok(())
    }
}

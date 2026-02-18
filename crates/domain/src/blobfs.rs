use async_trait::async_trait;

use crate::Result;

#[async_trait]
pub trait BlobFsService: Send + Sync {
    async fn mount_for_user(&self, username: &str) -> Result<()>;
}
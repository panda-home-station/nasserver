use async_trait::async_trait;
use common::core::Result;
use models::docs::{DocsListQuery, DocsListResp, DocsMkdirReq, DocsRenameReq, DocsDeleteQuery};
use std::path::{Path, PathBuf};

#[async_trait]
pub trait StorageService: Send + Sync {
    async fn list(&self, username: &str, query: DocsListQuery) -> Result<DocsListResp>;
    async fn mkdir(&self, username: &str, req: DocsMkdirReq) -> Result<()>;
    async fn rename(&self, username: &str, req: DocsRenameReq) -> Result<()>;
    async fn delete(&self, username: &str, query: DocsDeleteQuery) -> Result<()>;
    async fn get_file_path(&self, username: &str, virtual_path: &str) -> Result<PathBuf>;
    async fn save_file(&self, username: &str, parent_virtual_path: &str, name: &str, data: bytes::Bytes) -> Result<()>;
    async fn sync_external_change(&self, physical_path: &Path) -> Result<()>;
    async fn remove_external_change(&self, physical_path: &Path) -> Result<()>;
    async fn move_external_change(&self, from: &Path, to: &Path) -> Result<()>;
    async fn run_trash_purger(&self);
}

pub mod storage_service;
pub use storage_service::StorageServiceImpl;

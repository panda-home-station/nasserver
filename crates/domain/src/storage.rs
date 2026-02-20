use async_trait::async_trait;
use crate::Result;
pub use crate::entities::docs::DocsEntry;
pub use crate::dtos::docs::{
    DocsListQuery, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery,
    DocsListResp
};
use std::path::{Path, PathBuf};

#[derive(Debug, serde::Deserialize)]
pub struct InitiateMultipartReq {
    pub path: String,
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct UploadPartQuery {
    pub upload_id: String,
    pub part_number: i32,
}

#[derive(Debug, serde::Deserialize)]
pub struct Part {
    pub part_number: i32,
    pub etag: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct CompleteMultipartReq {
    pub upload_id: String,
    pub parts: Vec<Part>,
}

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
    async fn update_file_metadata(&self, username: &str, virtual_path: &str, size: i64) -> Result<()>;
    async fn commit_blob_change(&self, username: &str, virtual_path: &str, temp_path: &Path) -> Result<()>;
    async fn run_trash_purger(&self);

    async fn initiate_multipart_upload(&self, username: &str, parent_virtual_path: &str, name: &str) -> Result<String>;
    async fn save_file_part(&self, username: &str, upload_id: &str, part_number: i32, data: bytes::Bytes) -> Result<String>;
    async fn complete_multipart_upload(&self, username: &str, upload_id: &str, etags: Vec<(i32, String)>) -> Result<()>;
    async fn abort_multipart_upload(&self, username: &str, upload_id: &str) -> Result<()>;
}

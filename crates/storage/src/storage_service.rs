use async_trait::async_trait;
use crate::StorageService;
use domain::{Result, Error as DomainError, storage::{
    DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDeleteQuery
}};
use sqlx::{Pool, Postgres, Row};
use std::path::{Path, PathBuf};
use tokio::fs;

pub struct StorageServiceImpl {
    db: Pool<Postgres>,
    storage_path: String,
}

impl StorageServiceImpl {
    pub fn new(db: Pool<Postgres>, storage_path: String) -> Self {
        Self { db, storage_path }
    }

    fn normalize_path(&self, p: &str) -> String {
        let s = if p.starts_with('/') { &p[1..] } else { p };
        let s = s.replace("\\", "/");
        let parts: Vec<&str> = s.split('/').filter(|x| !x.is_empty() && *x != "." && *x != "..").collect();
        format!("/{}", parts.join("/"))
    }

    async fn check_app_access(&self, username: &str, app_name: &str) -> Result<bool> {
        if username == "admin" { return Ok(true); }
        let count: i64 = sqlx::query_scalar("select count(*) from app_permissions where app_name = $1 and username = $2")
            .bind(app_name)
            .bind(username)
            .fetch_one(&self.db)
            .await?;
        Ok(count > 0)
    }

    async fn resolve_physical_path(&self, username: &str, virtual_path: &str) -> Result<PathBuf> {
        let clean_path = self.normalize_path(virtual_path);
        if clean_path.starts_with("/AppData/") {
            let parts: Vec<&str> = clean_path.split('/').filter(|x| !x.is_empty()).collect();
            if parts.len() < 2 {
                return Ok(Path::new(&self.storage_path).join("vol1").join("AppData"));
            }
            let app_name = parts[1];
            if !self.check_app_access(username, app_name).await? {
                return Err(DomainError::Forbidden("Access denied".to_string()));
            }
            let mut p = Path::new(&self.storage_path).join("vol1").join("AppData").join(app_name);
            if parts.len() > 2 {
                let rel = parts[2..].join("/");
                p = p.join(rel);
            }
            Ok(p)
        } else if clean_path == "/AppData" {
             Ok(Path::new(&self.storage_path).join("vol1").join("AppData"))
        } else {
            let rel = if clean_path.starts_with('/') { &clean_path[1..] } else { &clean_path };
            Ok(Path::new(&self.storage_path).join("vol1").join("User").join(username).join(rel))
        }
    }
}

#[async_trait]
impl StorageService for StorageServiceImpl {
    async fn list(&self, username: &str, query: DocsListQuery) -> Result<DocsListResp> {
        let dir = self.normalize_path(&query.path.unwrap_or_else(|| "/".to_string()));
        
        if dir.starts_with("/AppData/") {
            let parts: Vec<&str> = dir.split('/').filter(|x| !x.is_empty()).collect();
            if parts.len() >= 2 {
                let app_name = parts[1];
                if !self.check_app_access(username, app_name).await? {
                    return Ok(DocsListResp { path: dir, entries: vec![], has_more: false, next_offset: 0 });
                }
            }
        }

        if dir == "/AppData" {
            let mut entries = Vec::new();
            let app_data_path = Path::new(&self.storage_path).join("vol1/AppData");
            if let Ok(mut read_dir) = fs::read_dir(app_data_path).await {
                while let Ok(Some(entry)) = read_dir.next_entry().await {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if !name.starts_with('.') {
                            if self.check_app_access(username, &name).await? {
                                entries.push(DocsEntry {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    name,
                                    is_dir: true,
                                    size: 0,
                                    modified_ts: 0,
                                    mime: "inode/directory".to_string(),
                                });
                            }
                        }
                    }
                }
            }
            return Ok(DocsListResp { path: dir, entries, has_more: false, next_offset: 0 });
        }

        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(100);

        let user_id: uuid::Uuid = sqlx::query_scalar("select id from users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await?;

        let rows = sqlx::query("select id, name, size, mime, updated_at, storage from cloud_files where user_id = $1 and dir = $2 order by name limit $3 offset $4")
            .bind(user_id)
            .bind(&dir)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.db)
            .await?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(DocsEntry {
                id: row.get("id"),
                name: row.get("name"),
                is_dir: row.get::<String, _>("mime") == "inode/directory",
                size: row.get("size"),
                modified_ts: 0, // Should parse from DB if needed
                mime: row.get("mime"),
            });
        }

        let total: i64 = sqlx::query_scalar("select count(*) from cloud_files where user_id = $1 and dir = $2")
            .bind(user_id)
            .bind(&dir)
            .fetch_one(&self.db)
            .await?;

        Ok(DocsListResp {
            path: dir,
            entries,
            has_more: total > (offset + limit) as i64,
            next_offset: offset + limit,
        })
    }

    async fn mkdir(&self, username: &str, req: DocsMkdirReq) -> Result<()> {
        let full_path = self.normalize_path(&req.path);
        let path_obj = Path::new(&full_path);
        let parent = path_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
        let name = path_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid path".to_string()))?;

        let physical_path = self.resolve_physical_path(username, &full_path).await?;

        fs::create_dir_all(&physical_path).await?;

        if !full_path.starts_with("/AppData") {
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await?;

            sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                .bind(uuid::Uuid::new_v4().to_string())
                .bind(user_id)
                .bind(name)
                .bind(parent)
                .bind(0i64)
                .bind("inode/directory")
                .bind("local")
                .execute(&self.db)
                .await?;
        }

        Ok(())
    }

    async fn rename(&self, username: &str, req: DocsRenameReq) -> Result<()> {
        let from = req.from.as_deref().ok_or_else(|| DomainError::BadRequest("Missing 'from' path".to_string()))?;
        let to = req.to.as_deref().ok_or_else(|| DomainError::BadRequest("Missing 'to' path".to_string()))?;

        let from_path = self.normalize_path(from);
        let to_path = self.normalize_path(to);

        let from_obj = Path::new(&from_path);
        let to_obj = Path::new(&to_path);

        let from_parent = from_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
        let from_name = from_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid from path".to_string()))?;

        let to_parent = to_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
        let to_name = to_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid to path".to_string()))?;

        let old_physical = self.resolve_physical_path(username, &from_path).await?;
        let new_physical = self.resolve_physical_path(username, &to_path).await?;

        if let Some(parent) = new_physical.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(old_physical, new_physical).await?;

        if !from_path.starts_with("/AppData") {
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await?;

            sqlx::query("update cloud_files set name = $1, dir = $2 where user_id = $3 and dir = $4 and name = $5")
                .bind(to_name)
                .bind(to_parent)
                .bind(user_id)
                .bind(from_parent)
                .bind(from_name)
                .execute(&self.db)
                .await?;
        }

        Ok(())
    }

    async fn delete(&self, username: &str, query: DocsDeleteQuery) -> Result<()> {
        let full_path = self.normalize_path(query.path.as_deref().ok_or_else(|| DomainError::BadRequest("Missing path".to_string()))?);
        let path_obj = Path::new(&full_path);
        let parent = path_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
        let name = path_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid path".to_string()))?;

        let physical_path = self.resolve_physical_path(username, &full_path).await?;

        let attr = fs::metadata(&physical_path).await?;
        if attr.is_dir() {
            fs::remove_dir_all(physical_path).await?;
        } else {
            fs::remove_file(physical_path).await?;
        }

        if !full_path.starts_with("/AppData") {
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await?;

            sqlx::query("delete from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user_id)
                .bind(parent)
                .bind(name)
                .execute(&self.db)
                .await?;
        }

        Ok(())
    }

    async fn get_file_path(&self, username: &str, virtual_path: &str) -> Result<PathBuf> {
        self.resolve_physical_path(username, virtual_path).await
    }

    async fn save_file(&self, username: &str, parent_virtual_path: &str, name: &str, data: bytes::Bytes) -> Result<()> {
        let physical_parent = self.resolve_physical_path(username, parent_virtual_path).await?;
        let physical_path = physical_parent.join(name);
        
        if let Some(parent) = physical_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        
        fs::write(&physical_path, data).await?;
        
        // If not AppData, sync to DB
        if !parent_virtual_path.starts_with("/AppData") {
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await?;
                
            let size = physical_path.metadata().map(|m| m.len() as i64).unwrap_or(0);
            let mime = mime_guess::from_path(&physical_path).first_or_octet_stream().to_string();

            // Use insert or replace or update if exists
            let exists: bool = sqlx::query_scalar("select count(*) > 0 from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user_id)
                .bind(parent_virtual_path)
                .bind(name)
                .fetch_one(&self.db)
                .await
                .unwrap_or(false);

            if exists {
                sqlx::query("update cloud_files set size = $1, mime = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                    .bind(size)
                    .bind(mime)
                    .bind(user_id)
                    .bind(parent_virtual_path)
                    .bind(name)
                    .execute(&self.db)
                    .await?;
            } else {
                sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, $6, 'local', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(user_id)
                    .bind(name)
                    .bind(parent_virtual_path)
                    .bind(size)
                    .bind(mime)
                    .execute(&self.db)
                    .await?;
            }
        }
        
        Ok(())
    }

    async fn run_trash_purger(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(24 * 3600)).await;
            let rows = sqlx::query("select id from cloud_files where dir like '/Trash%' and updated_at < CURRENT_TIMESTAMP - INTERVAL '30 days'")
                .fetch_all(&self.db)
                .await
                .unwrap_or_default();
            for row in rows {
                let id: String = row.try_get("id").unwrap_or_default();
                let _ = sqlx::query("delete from cloud_files where id = $1").bind(&id).execute(&self.db).await;
            }
        }
    }

    async fn sync_external_change(&self, physical_path: &Path) -> Result<()> {
        if let Some(info) = self.parse_physical_path(physical_path).await? {
            let exists: bool = sqlx::query_scalar("select count(*) > 0 from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(info.user_id)
                .bind(&info.parent_dir)
                .bind(&info.name)
                .fetch_one(&self.db)
                .await?;

            if exists {
                sqlx::query("update cloud_files set size = $1, mime = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                    .bind(info.size)
                    .bind(&info.mime)
                    .bind(info.user_id)
                    .bind(&info.parent_dir)
                    .bind(&info.name)
                    .execute(&self.db)
                    .await?;
            } else {
                sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                    .bind(uuid::Uuid::new_v4().to_string())
                    .bind(info.user_id)
                    .bind(&info.name)
                    .bind(&info.parent_dir)
                    .bind(info.size)
                    .bind(&info.mime)
                    .bind(&info.storage_type)
                    .execute(&self.db)
                    .await?;
            }
        }
        Ok(())
    }

    async fn remove_external_change(&self, physical_path: &Path) -> Result<()> {
        if let Some(info) = self.parse_physical_path(physical_path).await? {
            sqlx::query("delete from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(info.user_id)
                .bind(&info.parent_dir)
                .bind(&info.name)
                .execute(&self.db)
                .await?;
        }
        Ok(())
    }

    async fn move_external_change(&self, from: &Path, to: &Path) -> Result<()> {
        let from_info = self.parse_physical_path(from).await?;
        let to_info = self.parse_physical_path(to).await?;

        match (from_info, to_info) {
            (Some(f), Some(t)) if f.user_id == t.user_id => {
                let res = sqlx::query("update cloud_files set dir = $1, name = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                    .bind(&t.parent_dir)
                    .bind(&t.name)
                    .bind(f.user_id)
                    .bind(&f.parent_dir)
                    .bind(&f.name)
                    .execute(&self.db)
                    .await?;

                if res.rows_affected() == 0 {
                    self.sync_external_change(to).await?;
                }
            }
            (Some(_), _) => {
                self.remove_external_change(from).await?;
                self.sync_external_change(to).await?;
            }
            (_, Some(_)) => {
                self.sync_external_change(to).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

struct ExternalFileInfo {
    user_id: uuid::Uuid,
    parent_dir: String,
    name: String,
    size: i64,
    mime: String,
    storage_type: String,
}

impl StorageServiceImpl {
    async fn parse_physical_path(&self, path: &Path) -> Result<Option<ExternalFileInfo>> {
        let storage_base = Path::new(&self.storage_path).join("vol1/User");
        let rel_path = match path.strip_prefix(&storage_base) {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        let parts: Vec<String> = rel_path.iter().map(|s| s.to_string_lossy().into_owned()).collect();
        if parts.is_empty() { return Ok(None); }

        let username = &parts[0];
        let virtual_parts = if parts.len() > 1 { &parts[1..] } else { &[] };
        let virtual_path = format!("/{}", virtual_parts.join("/"));

        let user_id: Option<uuid::Uuid> = sqlx::query_scalar("select id from users where username = $1")
            .bind(username)
            .fetch_optional(&self.db)
            .await?;

        let user_id = match user_id {
            Some(id) => id,
            None => return Ok(None),
        };

        let name = Path::new(&virtual_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.is_empty() || name.starts_with('.') { return Ok(None); }

        let parent_dir = Path::new(&virtual_path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string());
        let parent_dir = if parent_dir.is_empty() { "/".to_string() } else if parent_dir.starts_with('/') { parent_dir } else { format!("/{}", parent_dir) };

        let is_dir = path.is_dir();
        let size = if is_dir { 0 } else { fs::metadata(path).await.map(|m| m.len() as i64).unwrap_or(0) };
        let storage_type = if is_dir { "dir" } else { "file" };
        let mime = if is_dir { "inode/directory".to_string() } else { mime_guess::from_path(path).first_or_octet_stream().to_string() };

        Ok(Some(ExternalFileInfo {
            user_id,
            parent_dir,
            name,
            size,
            mime,
            storage_type: storage_type.to_string(),
        }))
    }
}

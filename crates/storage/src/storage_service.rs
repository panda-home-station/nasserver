use async_trait::async_trait;
use crate::StorageService;
use domain::{Result, Error as DomainError, storage::{
    DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDeleteQuery
}};
use sqlx::{Pool, Postgres, Row};
use std::path::{Path, PathBuf};
use tokio::fs;
use sha2::{Sha256, Digest};
use std::ffi::OsStr;
use chrono::Utc;

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
        let count: i64 = sqlx::query_scalar("select count(*) from sys.app_permissions where app_name = $1 and username = $2")
            .bind(app_name)
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
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
                return Err(DomainError::Forbidden(format!("Access denied to app: {}", app_name)));
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
            Ok(Path::new(&self.storage_path).join("vol1").join("User_Data").join(username).join(rel))
        }
    }
}

#[async_trait]
impl StorageService for StorageServiceImpl {
    async fn list(&self, username: &str, query: DocsListQuery) -> Result<DocsListResp> {
        let dir = self.normalize_path(&query.path.unwrap_or_else(|| "/".to_string()));
        
        if dir == "/" {
            return Ok(DocsListResp {
                path: dir,
                entries: vec![
                    DocsEntry { id: "sys".to_string(), name: "System".to_string(), is_dir: true, size: 0, modified_ts: 0, mime: "inode/directory".to_string() },
                    DocsEntry { id: "usr".to_string(), name: "User".to_string(), is_dir: true, size: 0, modified_ts: 0, mime: "inode/directory".to_string() },
                    DocsEntry { id: "app".to_string(), name: "AppData".to_string(), is_dir: true, size: 0, modified_ts: 0, mime: "inode/directory".to_string() },
                ],
                has_more: false,
                next_offset: 0
            });
        }

        if dir == "/User" {
            // List users. For now, we can list the current user, or all users if admin.
            // Let's just list the current user to keep it simple and safe, 
            // unless we want to allow browsing other users.
            // The requirement says "/User/xxxx", implies multiple.
            // Let's query sys.users.
            
            // If not admin, maybe only show self?
            let users: Vec<String> = if username == "admin" {
                sqlx::query_scalar("select username from sys.users order by username")
                    .fetch_all(&self.db)
                    .await
                    .map_err(|e| DomainError::Database(e.to_string()))?
            } else {
                vec![username.to_string()]
            };

            let entries = users.into_iter().map(|u| DocsEntry {
                id: format!("user-{}", u),
                name: u,
                is_dir: true,
                size: 0,
                modified_ts: 0,
                mime: "inode/directory".to_string(),
            }).collect();

            return Ok(DocsListResp { path: dir, entries, has_more: false, next_offset: 0 });
        }

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

        if !self.check_app_access(username, "file-manager").await? {
            return Err(DomainError::Forbidden("file-manager".to_string()));
        }

        let (target_user_name, target_dir) = if dir.starts_with("/System") {
            ("admin".to_string(), dir.clone())
        } else if dir.starts_with("/User/") {
            let parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
            if parts.len() < 2 {
                return Err(DomainError::BadRequest("Invalid path".to_string()));
            }
            let u = parts[1];
            if u != username && username != "admin" {
                return Err(DomainError::Forbidden("Access denied".to_string()));
            }
            (u.to_string(), dir.clone())
        } else {
            return Err(DomainError::BadRequest("Invalid path".to_string()));
        };

        let limit = query.limit.unwrap_or(200) as i64;
        let offset = query.offset.unwrap_or(0) as i64;

        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(&target_user_name)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

        let rows = sqlx::query("select id, name, size, mime, updated_at, storage from storage.cloud_files where user_id = $1 and dir = $2 order by name limit $3 offset $4")
            .bind(user_id)
            .bind(&target_dir)
            .bind(limit)
            .bind(offset)
            .fetch_all(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        let mut entries = Vec::new();
        for row in rows {
            let id: uuid::Uuid = row.get("id");
            let updated_at: Option<chrono::DateTime<Utc>> = row.get("updated_at");
            entries.push(DocsEntry {
                id: id.to_string(),
                name: row.get("name"),
                is_dir: row.get::<String, _>("mime") == "inode/directory",
                size: row.get("size"),
                modified_ts: updated_at.map(|t| t.timestamp()).unwrap_or(0),
                mime: row.get("mime"),
            });
        }

        let total: i64 = sqlx::query_scalar("select count(*) from storage.cloud_files where user_id = $1 and dir = $2")
            .bind(user_id)
            .bind(&target_dir)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

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

        if full_path.starts_with("/AppData") {
            let physical_path = self.resolve_physical_path(username, &full_path).await?;
            fs::create_dir_all(&physical_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        } else {
            let (target_user_name, parent_dir) = if full_path.starts_with("/System") {
                if username != "admin" {
                    return Err(DomainError::Forbidden("Only admin can modify /System".to_string()));
                }
                if full_path == "/System" {
                     return Err(DomainError::BadRequest("Cannot create system root".to_string()));
                }
                ("admin".to_string(), parent.to_string())
            } else if full_path.starts_with("/User/") {
                let parts: Vec<&str> = full_path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() < 2 {
                     return Err(DomainError::BadRequest("Invalid path".to_string()));
                }
                let u = parts[1];
                if u != username && username != "admin" {
                     return Err(DomainError::Forbidden("Access denied".to_string()));
                }
                if parts.len() == 2 {
                     return Err(DomainError::BadRequest("Cannot create user root".to_string()));
                }
                (u.to_string(), parent.to_string())
            } else {
                 return Err(DomainError::BadRequest("Invalid path".to_string()));
            };

            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(&target_user_name)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            sqlx::query("insert into storage.cloud_files (id, user_id, name, dir, size, mime, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                .bind(uuid::Uuid::new_v4())
                .bind(user_id)
                .bind(name)
                .bind(&parent_dir)
                .bind(0i64)
                .bind("inode/directory")
                .bind("virtual")
                .execute(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
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

        if from_path.starts_with("/AppData") || to_path.starts_with("/AppData") {
            let old_physical = self.resolve_physical_path(username, &from_path).await?;
            let new_physical = self.resolve_physical_path(username, &to_path).await?;
            if let Some(parent) = new_physical.parent() {
                fs::create_dir_all(parent).await.map_err(|e| DomainError::Io(e.to_string()))?;
            }
            fs::rename(old_physical, new_physical).await.map_err(|e| DomainError::Io(e.to_string()))?;
        } else {
            let from_user = if from_path.starts_with("/System") {
                "admin".to_string()
            } else if from_path.starts_with("/User/") {
                let parts: Vec<&str> = from_path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid from path".to_string())); }
                parts[1].to_string()
            } else {
                return Err(DomainError::BadRequest("Invalid from path".to_string()));
            };

            let to_user = if to_path.starts_with("/System") {
                "admin".to_string()
            } else if to_path.starts_with("/User/") {
                let parts: Vec<&str> = to_path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid to path".to_string())); }
                parts[1].to_string()
            } else {
                return Err(DomainError::BadRequest("Invalid to path".to_string()));
            };

            if from_user != to_user {
                return Err(DomainError::Forbidden("Cannot move files between different users/system".to_string()));
            }

            if from_user != username && username != "admin" {
                return Err(DomainError::Forbidden("Access denied".to_string()));
            }

            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(&from_user)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            sqlx::query("update storage.cloud_files set name = $1, dir = $2 where user_id = $3 and dir = $4 and name = $5")
                .bind(to_name)
                .bind(to_parent)
                .bind(user_id)
                .bind(from_parent)
                .bind(from_name)
                .execute(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            let like_pat = format!("{}/%", from_path);

            // Update direct children
            sqlx::query("update storage.cloud_files set dir = $1 where user_id = $2 and dir = $3")
                .bind(&to_path)
                .bind(user_id)
                .bind(&from_path)
                .execute(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            // Update grandchildren
            sqlx::query("update storage.cloud_files set dir = $1 || substring(dir from length($2) + 1) where user_id = $3 and dir like $4")
                .bind(&to_path)
                .bind(&from_path)
                .bind(user_id)
                .bind(&like_pat)
                .execute(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
        }


        Ok(())
    }

    async fn delete(&self, username: &str, query: DocsDeleteQuery) -> Result<()> {
        let full_path = self.normalize_path(query.path.as_deref().ok_or_else(|| DomainError::BadRequest("Missing path".to_string()))?);
        let path_obj = Path::new(&full_path);
        let parent = path_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
        let name = path_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid path".to_string()))?;

        if full_path.starts_with("/AppData") {
            let physical_path = self.resolve_physical_path(username, &full_path).await?;
            let attr = fs::metadata(&physical_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            if attr.is_dir() {
                fs::remove_dir_all(physical_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            } else {
                fs::remove_file(physical_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            }
        } else if full_path == "/Trash" || full_path.starts_with("/Trash/") {
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            let row = sqlx::query("select id, storage, blob_hash, mime from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user_id)
                .bind(parent)
                .bind(name)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            if let Some(row) = row {
                let id: uuid::Uuid = row.get("id");
                let storage: String = row.get("storage");
                let blob_hash: Option<String> = row.try_get("blob_hash").ok();
                let mime: String = row.get("mime");

                if storage == "blob" {
                    if let Some(hash) = blob_hash.clone() {
                        let cnt: i64 = sqlx::query_scalar("select count(*) from storage.cloud_files where blob_hash = $1 and id <> $2 and dir not like '/Trash%'")
                            .bind(&hash)
                            .bind(id)
                            .fetch_one(&self.db)
                            .await
                            .unwrap_or(0);
                        if cnt == 0 {
                            let blob_path = Path::new(&self.storage_path)
                                .join("vol1/blobs")
                                .join(&hash[0..2])
                                .join(&hash[2..4])
                                .join(&hash);
                            let _ = fs::remove_file(blob_path).await;
                        }
                    }
                }

                let _ = sqlx::query("delete from storage.cloud_files where id = $1")
                    .bind(id)
                    .execute(&self.db)
                    .await;

                if mime == "inode/directory" {
                    let dir_path = format!("{}/{}", parent, name);
                    let like_pat = format!("{}/%", dir_path);
                    let rows = sqlx::query("select id, storage, blob_hash from storage.cloud_files where user_id = $1 and (dir = $2 or dir like $3)")
                        .bind(user_id)
                        .bind(&dir_path)
                        .bind(&like_pat)
                        .fetch_all(&self.db)
                        .await
                        .unwrap_or_default();
                    for r in rows {
                        let cid: uuid::Uuid = r.get("id");
                        let cstorage: String = r.get("storage");
                        let cblob_hash: Option<String> = r.try_get("blob_hash").ok();
                        if cstorage == "blob" {
                            if let Some(ch) = cblob_hash {
                                let cnt: i64 = sqlx::query_scalar("select count(*) from storage.cloud_files where blob_hash = $1 and id <> $2 and dir not like '/Trash%'")
                                    .bind(&ch)
                                    .bind(cid)
                                    .fetch_one(&self.db)
                                    .await
                                    .unwrap_or(0);
                                if cnt == 0 {
                                    let blob_path = Path::new(&self.storage_path)
                                        .join("vol1/blobs")
                                        .join(&ch[0..2])
                                        .join(&ch[2..4])
                                        .join(&ch);
                                    let _ = fs::remove_file(blob_path).await;
                                }
                            }
                        }
                        let _ = sqlx::query("delete from storage.cloud_files where id = $1")
                            .bind(cid)
                            .execute(&self.db)
                            .await;
                    }
                }
            }
        } else {
            let target_user = if full_path.starts_with("/System") {
                 if username != "admin" { return Err(DomainError::Forbidden("Only admin can modify /System".to_string())); }
                 "admin".to_string()
            } else if full_path.starts_with("/User/") {
                 let parts: Vec<&str> = full_path.split('/').filter(|s| !s.is_empty()).collect();
                 if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid path".to_string())); }
                 parts[1].to_string()
            } else {
                 return Err(DomainError::BadRequest("Invalid path".to_string()));
            };

            if target_user != username && username != "admin" {
                 return Err(DomainError::Forbidden("Access denied".to_string()));
            }

            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(&target_user)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            let mime_opt: Option<String> = sqlx::query_scalar("select mime from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user_id)
                .bind(parent)
                .bind(name)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            let trash_base = format!("/Trash/{}", Utc::now().format("%Y%m%d"));
            
            // Determine target directory in trash
            // If parent is "/", target is "/Trash/Date"
            // If parent is "/A", target is "/Trash/Date/A"
            let target_dir = if parent == "/" {
                trash_base.clone()
            } else {
                format!("{}{}", trash_base, parent)
            };

            // Check for collision and resolve name
            let mut final_name = name.to_string();
            let mut i = 1;

            loop {
                let exists: bool = sqlx::query_scalar("select count(*) > 0 from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                    .bind(user_id)
                    .bind(&target_dir)
                    .bind(&final_name)
                    .fetch_one(&self.db)
                    .await
                    .map_err(|e| DomainError::Database(e.to_string()))?;

                if !exists {
                    break;
                }

                let path = Path::new(name);
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                let ext = path.extension().map(|e| e.to_string_lossy().to_string());

                if let Some(e) = ext {
                    final_name = format!("{} ({}).{}", stem, i, e);
                } else {
                    final_name = format!("{} ({})", stem, i);
                }
                i += 1;
            }

            if let Some(mime) = mime_opt {
                if mime == "inode/directory" {
                    let from_dir = full_path.clone();
                    let like_pat = format!("{}/%", from_dir);
                    
                    // New root for children: target_dir / final_name
                    let new_subtree_root = if target_dir == "/" {
                        format!("/{}", final_name)
                    } else {
                        format!("{}/{}", target_dir, final_name)
                    };

                    // Update children (recursive)
                    // We use substring to replace the old prefix (from_dir) with new_subtree_root
                    sqlx::query("update storage.cloud_files set dir = $1 || substring(dir from length($2) + 1), updated_at = CURRENT_TIMESTAMP where user_id = $3 and (dir = $4 or dir like $5)")
                        .bind(&new_subtree_root)
                        .bind(&from_dir) 
                        .bind(user_id)
                        .bind(&from_dir)
                        .bind(&like_pat)
                        .execute(&self.db)
                        .await
                        .map_err(|e| DomainError::Database(e.to_string()))?;

                    // Update directory itself
                    sqlx::query("update storage.cloud_files set dir = $1, name = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                        .bind(&target_dir)
                        .bind(&final_name)
                        .bind(user_id)
                        .bind(parent)
                        .bind(name)
                        .execute(&self.db)
                        .await
                        .map_err(|e| DomainError::Database(e.to_string()))?;
                } else {
                    // File
                    sqlx::query("update storage.cloud_files set dir = $1, name = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                        .bind(&target_dir)
                        .bind(&final_name)
                        .bind(user_id)
                        .bind(parent)
                        .bind(name)
                        .execute(&self.db)
                        .await
                        .map_err(|e| DomainError::Database(e.to_string()))?;
                }
            } else {
                 // Fallback if mime not found (treat as file/safe update)
                 sqlx::query("update storage.cloud_files set dir = $1, name = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                    .bind(&target_dir)
                    .bind(&final_name)
                    .bind(user_id)
                    .bind(parent)
                    .bind(name)
                    .execute(&self.db)
                    .await
                    .map_err(|e| DomainError::Database(e.to_string()))?;
            }
        }

        Ok(())
    }

    async fn get_file_path(&self, username: &str, virtual_path: &str) -> Result<PathBuf> {
        let normalized_path = self.normalize_path(virtual_path);

        if normalized_path.starts_with("/AppData") {
            return self.resolve_physical_path(username, &normalized_path).await;
        }

        let path = Path::new(&normalized_path);
        let parent_dir = path.parent().and_then(Path::to_str).unwrap_or("/");
        let name = path.file_name().and_then(OsStr::to_str).unwrap_or("");

        if name.is_empty() {
            return Err(DomainError::NotFound("File name is empty.".to_string()));
        }

        let blob_hash: Option<String>;
        if normalized_path.starts_with("/System") {
             // System binaries (public read)
             blob_hash = sqlx::query_scalar(
                "SELECT blob_hash FROM storage.cloud_files WHERE dir = $1 AND name = $2 AND blob_hash IS NOT NULL"
            )
            .bind(parent_dir)
            .bind(name)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
        } else {
            let target_user = if normalized_path.starts_with("/User/") {
                 let parts: Vec<&str> = normalized_path.split('/').filter(|s| !s.is_empty()).collect();
                 if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid path".to_string())); }
                 parts[1].to_string()
            } else {
                 return Err(DomainError::BadRequest("Invalid path".to_string()));
            };
            
            if target_user != username && username != "admin" {
                  return Err(DomainError::Forbidden("Access denied".to_string()));
            }

            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(&target_user)
                .fetch_one(&self.db)
                .await
                .map_err(|_| DomainError::NotFound(format!("User {} not found", target_user)))?;

            blob_hash = sqlx::query_scalar(
                "SELECT blob_hash FROM storage.cloud_files WHERE user_id = $1 AND dir = $2 AND name = $3 AND blob_hash IS NOT NULL"
            )
            .bind(user_id)
            .bind(parent_dir)
            .bind(name)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
        }

        match blob_hash {
            Some(hash) => {
                let blob_path = Path::new(&self.storage_path)
                    .join("vol1/blobs")
                    .join(&hash[0..2])
                    .join(&hash[2..4])
                    .join(&hash);
                Ok(blob_path)
            }
            None => {
                // Fallback for files not yet migrated
                self.resolve_physical_path(username, &normalized_path).await
            }
        }
    }

    async fn save_file(&self, username: &str, parent_virtual_path: &str, name: &str, data: bytes::Bytes) -> Result<()> {
        let normalized_parent = self.normalize_path(parent_virtual_path);

        if normalized_parent.starts_with("/AppData") {
            let physical_parent = self.resolve_physical_path(username, &normalized_parent).await?;
            let physical_path = physical_parent.join(name);
            if let Some(parent) = physical_path.parent() {
                fs::create_dir_all(parent).await.map_err(|e| DomainError::Io(e.to_string()))?;
            }
            fs::write(&physical_path, &data).await.map_err(|e| DomainError::Io(e.to_string()))?;
            return Ok(());
        }

        let target_user = if normalized_parent.starts_with("/System") {
             if username != "admin" { return Err(DomainError::Forbidden("Only admin can modify /System".to_string())); }
             "admin".to_string()
        } else if normalized_parent.starts_with("/User/") {
             let parts: Vec<&str> = normalized_parent.split('/').filter(|s| !s.is_empty()).collect();
             if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid path".to_string())); }
             parts[1].to_string()
        } else {
             return Err(DomainError::BadRequest("Invalid path".to_string()));
        };
        
        if target_user != username && username != "admin" {
             return Err(DomainError::Forbidden("Access denied".to_string()));
        }

        // User files are stored in the blob store
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let blob_hash = format!("{:x}", hasher.finalize());

        let blob_dir = Path::new(&self.storage_path)
            .join("vol1/blobs")
            .join(&blob_hash[0..2])
            .join(&blob_hash[2..4]);
        
        fs::create_dir_all(&blob_dir).await.map_err(|e| DomainError::Io(e.to_string()))?;
        let blob_path = blob_dir.join(&blob_hash);

        if fs::metadata(&blob_path).await.is_err() {
            fs::write(&blob_path, &data).await.map_err(|e| DomainError::Io(e.to_string()))?;
        }

        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(&target_user)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        let size = data.len() as i64;
        let mime = mime_guess::from_path(name).first_or_octet_stream().to_string();

        sqlx::query(
            "insert into storage.cloud_files (id, user_id, name, dir, size, mime, storage, blob_hash) 
             values ($1, $2, $3, $4, $5, $6, 'blob', $7) 
             on conflict (user_id, dir, name) 
             do update set size = EXCLUDED.size, mime = EXCLUDED.mime, blob_hash = EXCLUDED.blob_hash, updated_at = CURRENT_TIMESTAMP"
        )
        .bind(uuid::Uuid::new_v4())
        .bind(user_id)
        .bind(name)
        .bind(&normalized_parent)
        .bind(size)
        .bind(mime)
        .bind(&blob_hash)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;
        
        Ok(())
    }

    async fn run_trash_purger(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(24 * 3600)).await;
            let rows = sqlx::query("select id, storage, blob_hash from storage.cloud_files where dir like '/Trash%' and updated_at < CURRENT_TIMESTAMP - INTERVAL '30 days'")
                .fetch_all(&self.db)
                .await
                .unwrap_or_default();
            for row in rows {
                let id = row.try_get::<uuid::Uuid, _>("id").ok();
                let storage: Option<String> = row.try_get("storage").ok();
                let blob_hash: Option<String> = row.try_get("blob_hash").ok();
                if let (Some(id), Some(storage)) = (id, storage) {
                    if storage == "blob" {
                        if let Some(hash) = blob_hash.clone() {
                            let cnt: i64 = sqlx::query_scalar("select count(*) from storage.cloud_files where blob_hash = $1 and id <> $2 and dir not like '/Trash%'")
                                .bind(&hash)
                                .bind(id)
                                .fetch_one(&self.db)
                                .await
                                .unwrap_or(0);
                            if cnt == 0 {
                                let blob_path = Path::new(&self.storage_path)
                                    .join("vol1/blobs")
                                    .join(&hash[0..2])
                                    .join(&hash[2..4])
                                    .join(&hash);
                                let _ = fs::remove_file(blob_path).await;
                            }
                        }
                    }
                    let _ = sqlx::query("delete from storage.cloud_files where id = $1").bind(id).execute(&self.db).await;
                }
            }
        }
    }

    async fn sync_external_change(&self, physical_path: &Path) -> Result<()> {
        if let Some(info) = self.parse_physical_path(physical_path).await? {
            let exists: bool = sqlx::query_scalar("select count(*) > 0 from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(info.user_id)
                .bind(&info.parent_dir)
                .bind(&info.name)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            if exists {
                sqlx::query("update storage.cloud_files set size = $1, mime = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                    .bind(info.size)
                    .bind(&info.mime)
                    .bind(info.user_id)
                    .bind(&info.parent_dir)
                    .bind(&info.name)
                    .execute(&self.db)
                    .await
                    .map_err(|e| DomainError::Database(e.to_string()))?;
            } else {
                sqlx::query("insert into storage.cloud_files (id, user_id, name, dir, size, mime, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, $6, $7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)")
                    .bind(uuid::Uuid::new_v4())
                    .bind(info.user_id)
                    .bind(&info.name)
                    .bind(&info.parent_dir)
                    .bind(info.size)
                    .bind(&info.mime)
                    .bind(&info.storage_type)
                    .execute(&self.db)
                    .await
                    .map_err(|e| DomainError::Database(e.to_string()))?;
            }
        }
        Ok(())
    }

    async fn remove_external_change(&self, physical_path: &Path) -> Result<()> {
        if let Some(info) = self.parse_physical_path(physical_path).await? {
            sqlx::query("delete from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(info.user_id)
                .bind(&info.parent_dir)
                .bind(&info.name)
                .execute(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
        }
        Ok(())
    }

    async fn move_external_change(&self, from: &Path, to: &Path) -> Result<()> {
        let from_info = self.parse_physical_path(from).await?;
        let to_info = self.parse_physical_path(to).await?;

        match (from_info, to_info) {
            (Some(f), Some(t)) if f.user_id == t.user_id => {
                let res = sqlx::query("update storage.cloud_files set dir = $1, name = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
                    .bind(&t.parent_dir)
                    .bind(&t.name)
                    .bind(f.user_id)
                    .bind(&f.parent_dir)
                    .bind(&f.name)
                    .execute(&self.db)
                    .await
                    .map_err(|e| DomainError::Database(e.to_string()))?;

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

    async fn update_file_metadata(&self, username: &str, virtual_path: &str, size: i64) -> Result<()> {
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|_| DomainError::NotFound(format!("User {} not found", username)))?;

        let clean_path = self.normalize_path(virtual_path);
        let name = Path::new(&clean_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent_dir = Path::new(&clean_path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string());
        let parent_dir = if parent_dir.is_empty() { "/".to_string() } else if parent_dir.starts_with('/') { parent_dir } else { format!("/{}", parent_dir) };

        sqlx::query("update storage.cloud_files set size = $1, updated_at = CURRENT_TIMESTAMP where user_id = $2 and dir = $3 and name = $4")
            .bind(size)
            .bind(user_id)
            .bind(parent_dir)
            .bind(name)
            .execute(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

        Ok(())
    }

    async fn commit_blob_change(&self, username: &str, virtual_path: &str, temp_path: &Path) -> Result<()> {
        // 1. Calculate SHA256 of temp file
        let mut file = tokio::fs::File::open(temp_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        let mut hasher = Sha256::new();
        let mut buffer = [0; 8192];
        use tokio::io::AsyncReadExt;
        loop {
            let n = file.read(&mut buffer).await.map_err(|e| DomainError::Io(e.to_string()))?;
            if n == 0 { break; }
            hasher.update(&buffer[..n]);
        }
        let hash = format!("{:x}", hasher.finalize());
        let size = file.metadata().await.map_err(|e| DomainError::Io(e.to_string()))?.len();
        
        // 2. Determine new blob path
        let prefix1 = &hash[0..2];
        let prefix2 = &hash[2..4];
        let blob_dir = Path::new(&self.storage_path).join("vol1/blobs").join(prefix1).join(prefix2);
        tokio::fs::create_dir_all(&blob_dir).await.map_err(|e| DomainError::Io(e.to_string()))?;
        let blob_path = blob_dir.join(&hash);

        // 3. Move temp file to blob path (or delete if exists)
        if blob_path.exists() {
            tokio::fs::remove_file(temp_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        } else {
            if let Err(e) = tokio::fs::rename(temp_path, &blob_path).await {
                let is_cross_device = e.raw_os_error() == Some(18); // EXDEV
                if is_cross_device {
                     tokio::fs::copy(temp_path, &blob_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
                     tokio::fs::remove_file(temp_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
                } else {
                    return Err(DomainError::Io(e.to_string()));
                }
            }
        }

        // 4. Update DB
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|_| DomainError::NotFound(format!("User {} not found", username)))?;

        let clean_path = self.normalize_path(virtual_path);
        let name = Path::new(&clean_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent_dir = Path::new(&clean_path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "/".to_string());
        let parent_dir = if parent_dir.is_empty() { "/".to_string() } else if parent_dir.starts_with('/') { parent_dir } else { format!("/{}", parent_dir) };

        sqlx::query("update storage.cloud_files set blob_hash = $1, size = $2, updated_at = CURRENT_TIMESTAMP where user_id = $3 and dir = $4 and name = $5")
            .bind(hash)
            .bind(size as i64)
            .bind(user_id)
            .bind(parent_dir)
            .bind(name)
            .execute(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

        Ok(())
    }

    async fn initiate_multipart_upload(&self, username: &str, parent_virtual_path: &str, name: &str) -> Result<String> {
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|_| DomainError::NotFound(format!("User {} not found", username)))?;

        let upload_id = uuid::Uuid::new_v4().to_string();
        let dir = self.normalize_path(parent_virtual_path);

        sqlx::query(
            "INSERT INTO storage.multipart_uploads (upload_id, user_id, dir, name) VALUES ($1, $2, $3, $4)"
        )
        .bind(&upload_id)
        .bind(user_id)
        .bind(dir)
        .bind(name)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;

        Ok(upload_id)
    }

    async fn save_file_part(&self, _username: &str, upload_id: &str, part_number: i32, data: bytes::Bytes) -> Result<String> {
        // 1. Verify upload_id exists
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM storage.multipart_uploads WHERE upload_id = $1)")
            .bind(upload_id)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

        if !exists {
            return Err(DomainError::NotFound(format!("Upload with ID {} not found", upload_id)));
        }

        // 2. Calculate ETag
        let mut hasher = Sha256::new();
        hasher.update(&data);
        let etag = format!("{:x}", hasher.finalize());

        // 3. Save part to temporary location
        let temp_dir = Path::new(&self.storage_path).join("vol1/tmp/multipart").join(upload_id);
        fs::create_dir_all(&temp_dir).await.map_err(|e| DomainError::Io(e.to_string()))?;
        let part_path = temp_dir.join(part_number.to_string());
        fs::write(&part_path, &data).await.map_err(|e| DomainError::Io(e.to_string()))?;

        // 4. Record part info in the database
        let size = data.len() as i64;
        sqlx::query(
            "INSERT INTO storage.upload_parts (upload_id, part_number, etag, size) VALUES ($1, $2, $3, $4)
             ON CONFLICT (upload_id, part_number) DO UPDATE SET etag = EXCLUDED.etag, size = EXCLUDED.size"
        )
        .bind(upload_id)
        .bind(part_number)
        .bind(&etag)
        .bind(size)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;

        Ok(etag)
    }

    async fn complete_multipart_upload(&self, _username: &str, upload_id: &str, etags: Vec<(i32, String)>) -> Result<()> {
        // 1. Fetch upload info and parts from DB
        let upload_info: (uuid::Uuid, String, String) = sqlx::query_as(
            "SELECT user_id, dir, name FROM storage.multipart_uploads WHERE upload_id = $1"
        )
        .bind(upload_id)
        .fetch_one(&self.db)
        .await
        .map_err(|_| DomainError::NotFound(format!("Upload with ID {} not found", upload_id)))?;

        let db_parts: Vec<(i32, String, i64)> = sqlx::query_as(
            "SELECT part_number, etag, size FROM storage.upload_parts WHERE upload_id = $1 ORDER BY part_number ASC"
        )
        .bind(upload_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;

        // 2. Validate parts
        if etags.len() != db_parts.len() {
            return Err(DomainError::BadRequest("Part number mismatch".to_string()));
        }
        let mut total_size = 0;
        let mut combined_hash_data = Vec::new();
        for (i, (part_number, etag, size)) in db_parts.iter().enumerate() {
            if etags[i].0 != *part_number || etags[i].1 != *etag {
                return Err(DomainError::BadRequest(format!("Mismatch on part #{}", part_number)));
            }
            total_size += size;
            combined_hash_data.extend_from_slice(&hex::decode(etag).map_err(|_| DomainError::BadRequest("Invalid ETag format".to_string()))?);
        }

        // 3. Calculate final blob hash
        let mut final_hasher = Sha256::new();
        final_hasher.update(&combined_hash_data);
        let final_hash = format!("{:x}", final_hasher.finalize());

        // 4. Merge parts into final blob
        let blob_dir = Path::new(&self.storage_path).join("vol1/blobs").join(&final_hash[0..2]).join(&final_hash[2..4]);
        fs::create_dir_all(&blob_dir).await.map_err(|e| DomainError::Io(e.to_string()))?;
        let blob_path = blob_dir.join(&final_hash);

        if fs::metadata(&blob_path).await.is_err() {
            let temp_dir = Path::new(&self.storage_path).join("vol1/tmp/multipart").join(upload_id);
            let mut dest_file = fs::File::create(&blob_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            for (part_number, _, _) in &db_parts {
                let part_path = temp_dir.join(part_number.to_string());
                let mut part_file = fs::File::open(&part_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
                tokio::io::copy(&mut part_file, &mut dest_file).await.map_err(|e| DomainError::Io(e.to_string()))?;
            }
        }

        // 5. Update metadata in cloud_files
        let (user_id, dir, name) = upload_info;
        let mime = mime_guess::from_path(&name).first_or_octet_stream().to_string();
        sqlx::query(
            "INSERT INTO storage.cloud_files (id, user_id, name, dir, size, mime, storage, blob_hash) 
             VALUES ($1, $2, $3, $4, $5, $6, 'blob', $7) 
             ON CONFLICT (user_id, dir, name) 
             DO UPDATE SET size = EXCLUDED.size, mime = EXCLUDED.mime, blob_hash = EXCLUDED.blob_hash, updated_at = CURRENT_TIMESTAMP"
        )
        .bind(uuid::Uuid::new_v4())
        .bind(user_id)
        .bind(name)
        .bind(dir)
        .bind(total_size)
        .bind(mime)
        .bind(&final_hash)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;

        // 6. Cleanup
        sqlx::query("DELETE FROM storage.multipart_uploads WHERE upload_id = $1").bind(upload_id).execute(&self.db).await.ok();
        let temp_dir = Path::new(&self.storage_path).join("vol1/tmp/multipart").join(upload_id);
        fs::remove_dir_all(temp_dir).await.ok();

        Ok(())
    }

    async fn abort_multipart_upload(&self, _username: &str, _upload_id: &str) -> Result<()> {
        todo!()
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

        let user_id: Option<uuid::Uuid> = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

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

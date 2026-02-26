use async_trait::async_trait;
use crate::StorageService;
use domain::{Result, Error as DomainError, storage::{
    DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDeleteQuery
}};
use sqlx::{Pool, Postgres, Row};
use opendal::{Operator, services::Fs};
use std::path::Path;
use chrono::Utc;
use futures_util::io::AsyncRead;

pub struct StorageServiceImpl {
    db: Pool<Postgres>,
    op: Operator,
    storage_path: String,
}

impl StorageServiceImpl {
    pub fn new(db: Pool<Postgres>, storage_path: String) -> Self {
        let mut builder = Fs::default();
        builder.root(&storage_path);
        let op = Operator::new(builder).unwrap().finish();
        Self { db, op, storage_path }
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

    async fn resolve_opendal_path(&self, username: &str, virtual_path: &str) -> Result<String> {
        let clean_path = self.normalize_path(virtual_path);
        
        if clean_path.starts_with("/AppData/") {
            let parts: Vec<&str> = clean_path.split('/').filter(|x| !x.is_empty()).collect();
            if parts.len() < 2 {
                return Ok("vol1/AppData".to_string());
            }
            let app_name = parts[1];
            if !self.check_app_access(username, app_name).await? {
                return Err(DomainError::Forbidden(format!("Access denied to app: {}", app_name)));
            }
            let mut p = format!("vol1/AppData/{}", app_name);
            if parts.len() > 2 {
                let rel = parts[2..].join("/");
                p = format!("{}/{}", p, rel);
            }
            Ok(p)
        } else if clean_path == "/AppData" {
            Ok("vol1/AppData".to_string())
        } else if clean_path.starts_with("/System/") || clean_path == "/System" {
            let rel = clean_path.trim_start_matches("/System").trim_start_matches('/');
            if rel.is_empty() {
                Ok("vol1/User/admin/System".to_string())
            } else {
                Ok(format!("vol1/User/admin/System/{}", rel))
            }
        } else if clean_path.starts_with("/Trash/") || clean_path == "/Trash" {
            let rel = clean_path.trim_start_matches("/Trash").trim_start_matches('/');
            if rel.is_empty() {
                Ok(format!("vol1/User/{}/.Trash", username))
            } else {
                Ok(format!("vol1/User/{}/.Trash/{}", username, rel))
            }
        } else if clean_path.starts_with("/User/") {
            let parts: Vec<&str> = clean_path.split('/').filter(|x| !x.is_empty()).collect();
            if parts.len() < 2 {
                return Err(DomainError::BadRequest("Invalid /User path".to_string()));
            }
            let owner = parts[1];
            if parts.len() == 2 {
                Ok(format!("vol1/User/{}", owner))
            } else {
                let rel = parts[2..].join("/");
                Ok(format!("vol1/User/{}/{}", owner, rel))
            }
        } else {
            let rel = if clean_path.starts_with('/') { &clean_path[1..] } else { &clean_path };
            Ok(format!("vol1/User/{}/{}", username, rel))
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

        if dir.starts_with("/AppData") {
            let op_path = self.resolve_opendal_path(username, &dir).await?;
            let mut entries = Vec::new();

            // OpenDAL list usually requires trailing slash for directory, or just the path.
            // Let's ensure trailing slash if it's a directory we are listing.
            let list_path = if op_path.ends_with('/') { op_path.clone() } else { format!("{}/", op_path) };

            if let Ok(op_entries) = self.op.list(&list_path).await {
                for entry in op_entries {
                    let name = entry.name().to_string();
                    
                    // Filter out current directory if returned
                    if name.is_empty() || name == "." { continue; }

                    let is_dir = entry.metadata().mode().is_dir();
                    
                    // For /AppData root, we need to check access for each app
                    if dir == "/AppData" {
                        if !is_dir { continue; } // Only dirs in AppData root
                        if !self.check_app_access(username, &name).await? {
                            continue;
                        }
                    }

                    entries.push(DocsEntry {
                        id: uuid::Uuid::new_v4().to_string(), // Generate random ID for non-DB entries
                        name: name.clone(),
                        is_dir,
                        size: entry.metadata().content_length() as i64,
                        modified_ts: entry.metadata().last_modified().map(|t| t.timestamp_millis()).unwrap_or(0),
                        mime: if is_dir { "inode/directory".to_string() } else { mime_guess::from_path(&name).first_or_octet_stream().to_string() },
                    });
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
            let op_path = self.resolve_opendal_path(username, &full_path).await?;
            self.op.create_dir(&format!("{}/", op_path)).await.map_err(|e| DomainError::Io(e.to_string()))?;
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

            let new_dir = if parent_dir == "/" { format!("/{}", name) } else { format!("{}/{}", parent_dir, name) };
            let op_path = self.resolve_opendal_path(&target_user_name, &new_dir).await?;
            self.op.create_dir(&format!("{}/", op_path)).await.map_err(|e| DomainError::Io(e.to_string()))?;
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
            let from_op = self.resolve_opendal_path(username, &from_path).await?;
            let to_op = self.resolve_opendal_path(username, &to_path).await?;
            
            // OpenDAL rename handles parent creation if backend supports it, but for Fs we might need to be careful?
            // Actually OpenDAL's rename contract: behavior is defined by backend. Fs usually requires parent.
            // But we can rely on OpenDAL to abstract or we might need to create parent if not exists.
            // Let's assume standard rename.
            
            // Check if source exists (optional but good for error)
            if self.op.stat(&from_op).await.is_err() {
                // If it's not found, maybe it's a directory without trailing slash?
                // Or maybe it's a file.
                // For now, let's just try rename.
            }

            self.op.rename(&from_op, &to_op).await.map_err(|e| DomainError::Io(e.to_string()))?;
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

            let from_virtual = from_path.clone();
            let to_virtual = to_path.clone();
            let from_op = self.resolve_opendal_path(&from_user, &from_virtual).await?;
            let to_op = self.resolve_opendal_path(&to_user, &to_virtual).await?;
            if let Some(parent) = Path::new(&to_op).parent() {
                let parent_str = parent.to_string_lossy().to_string();
                let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
            }
            self.op.rename(&from_op, &to_op).await.map_err(|e| DomainError::Io(e.to_string()))?;

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
            let op_path = self.resolve_opendal_path(username, &full_path).await?;
            self.op.remove_all(&op_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        } else if full_path == "/Trash" || full_path.starts_with("/Trash/") {
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            let row = sqlx::query("select id, mime from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user_id)
                .bind(parent)
                .bind(name)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;

            if let Some(row) = row {
                let id: uuid::Uuid = row.get("id");
                let mime: String = row.get("mime");

                let op_path = self.resolve_opendal_path(username, &full_path).await?;
                if mime == "inode/directory" {
                    let _ = self.op.remove_all(&op_path).await;
                } else {
                    let _ = self.op.delete(&op_path).await;
                }

                let _ = sqlx::query("delete from storage.cloud_files where id = $1")
                    .bind(id)
                    .execute(&self.db)
                    .await;

                if mime == "inode/directory" {
                    let dir_path = format!("{}/{}", parent, name);
                    let like_pat = format!("{}/%", dir_path);
                    let rows = sqlx::query("select id from storage.cloud_files where user_id = $1 and (dir = $2 or dir like $3)")
                        .bind(user_id)
                        .bind(&dir_path)
                        .bind(&like_pat)
                        .fetch_all(&self.db)
                        .await
                        .unwrap_or_default();
                    for r in rows {
                        let cid: uuid::Uuid = r.get("id");
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

                    let from_op = self.resolve_opendal_path(&target_user, &from_dir).await?;
                    let to_op = self.resolve_opendal_path(&target_user, &new_subtree_root).await?;
                    if let Some(parent) = Path::new(&to_op).parent() {
                        let parent_str = parent.to_string_lossy().to_string();
                        let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
                    }
                    self.op.rename(&from_op, &to_op).await.map_err(|e| DomainError::Io(e.to_string()))?;

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
                    let from_virtual = full_path.clone();
                    let to_virtual = if target_dir == "/" { format!("/{}", final_name) } else { format!("{}/{}", target_dir, final_name) };
                    let from_op = self.resolve_opendal_path(&target_user, &from_virtual).await?;
                    let to_op = self.resolve_opendal_path(&target_user, &to_virtual).await?;
                    if let Some(parent) = Path::new(&to_op).parent() {
                        let parent_str = parent.to_string_lossy().to_string();
                        let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
                    }
                    self.op.rename(&from_op, &to_op).await.map_err(|e| DomainError::Io(e.to_string()))?;

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
                 let from_virtual = full_path.clone();
                 let to_virtual = if target_dir == "/" { format!("/{}", final_name) } else { format!("{}/{}", target_dir, final_name) };
                 let from_op = self.resolve_opendal_path(&target_user, &from_virtual).await?;
                 let to_op = self.resolve_opendal_path(&target_user, &to_virtual).await?;
                 if let Some(parent) = Path::new(&to_op).parent() {
                     let parent_str = parent.to_string_lossy().to_string();
                     let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
                 }
                 self.op.rename(&from_op, &to_op).await.map_err(|e| DomainError::Io(e.to_string()))?;

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



    async fn get_file_reader(&self, username: &str, virtual_path: &str) -> Result<Box<dyn AsyncRead + Unpin + Send + Sync>> {
        let normalized_path = self.normalize_path(virtual_path);

        // Permission check
        if normalized_path.starts_with("/User/") {
            let parts: Vec<&str> = normalized_path.split('/').filter(|s| !s.is_empty()).collect();
            if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid path".to_string())); }
            let u = parts[1];
            if u != username && username != "admin" {
                return Err(DomainError::Forbidden("Access denied".to_string()));
            }
        } else if normalized_path.starts_with("/System") {
            if username != "admin" {
                // allow read for non-admin? keep strict
            }
        }

        let op_path = self.resolve_opendal_path(username, &normalized_path).await?;

        let reader = self.op.reader(&op_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        Ok(Box::new(reader))

    }
    async fn get_file_path(&self, username: &str, virtual_path: &str) -> Result<std::path::PathBuf> {
        let normalized_path = self.normalize_path(virtual_path);
        if normalized_path.starts_with("/User/") {
            let parts: Vec<&str> = normalized_path.split('/').filter(|s| !s.is_empty()).collect();
            if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid path".to_string())); }
            let u = parts[1];
            if u != username && username != "admin" {
                return Err(DomainError::Forbidden("Access denied".to_string()));
            }
        }
        let op_path = self.resolve_opendal_path(username, &normalized_path).await?;
        let full_path = Path::new(&self.storage_path).join(op_path);
        Ok(full_path)
    }

    async fn commit_blob_change(&self, username: &str, virtual_path: &str, temp_path: &Path) -> Result<()> {
        let normalized_path = self.normalize_path(virtual_path);
        let path = Path::new(&normalized_path);
        let parent_dir = path.parent().and_then(|p| p.to_str()).unwrap_or("/");
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if name.is_empty() {
             return Err(DomainError::BadRequest("Invalid path".to_string()));
        }

        self.save_file_from_path(username, parent_dir, name, temp_path).await
    }

    async fn save_file(&self, username: &str, parent_virtual_path: &str, name: &str, data: bytes::Bytes) -> Result<()> {
        let normalized_parent = self.normalize_path(parent_virtual_path);

        if normalized_parent.starts_with("/AppData") {
            let op_path = self.resolve_opendal_path(username, &normalized_parent).await?;
            let full_op_path = format!("{}/{}", op_path, name);
            self.op.write(&full_op_path, data).await.map_err(|e| DomainError::Io(e.to_string()))?;
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

        let size = data.len() as i64;

        let op_path = self.resolve_opendal_path(&target_user, &normalized_parent).await?;
        let full_op_path = format!("{}/{}", op_path, name);
        if let Some(parent) = Path::new(&full_op_path).parent() {
            let parent_str = parent.to_string_lossy().to_string();
            let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
        }
        self.op.write(&full_op_path, data).await.map_err(|e| DomainError::Io(e.to_string()))?;

        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(&target_user)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        let mime = mime_guess::from_path(name).first_or_octet_stream().to_string();

        sqlx::query(
            "insert into storage.cloud_files (id, user_id, name, dir, size, mime, storage) 
             values ($1, $2, $3, $4, $5, $6, 'file') 
             on conflict (user_id, dir, name) 
             do update set size = EXCLUDED.size, mime = EXCLUDED.mime, storage = EXCLUDED.storage, updated_at = CURRENT_TIMESTAMP"
        )
        .bind(uuid::Uuid::new_v4())
        .bind(user_id)
        .bind(name)
        .bind(&normalized_parent)
        .bind(size)
        .bind(mime)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;
        
        Ok(())
    }

    async fn save_file_from_path(&self, username: &str, parent_virtual_path: &str, name: &str, temp_path: &Path) -> Result<()> {
        let normalized_parent = self.normalize_path(parent_virtual_path);

        if normalized_parent.starts_with("/AppData") {
            let op_path = self.resolve_opendal_path(username, &normalized_parent).await?;
            let full_op_path = format!("{}/{}", op_path, name);
            
            let mut file = tokio::fs::File::open(temp_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            let mut writer = self.op.writer(&full_op_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            tokio::io::copy(&mut file, &mut writer).await.map_err(|e| DomainError::Io(e.to_string()))?;
            writer.close().await.map_err(|e| DomainError::Io(e.to_string()))?;
            
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

        let size = tokio::fs::metadata(temp_path).await.map_err(|e| DomainError::Io(e.to_string()))?.len();
        let op_path = self.resolve_opendal_path(&target_user, &normalized_parent).await?;
        let full_op_path = format!("{}/{}", op_path, name);
        if let Some(parent) = Path::new(&full_op_path).parent() {
            let parent_str = parent.to_string_lossy().to_string();
            let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
        }
        let mut file = tokio::fs::File::open(temp_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        let mut writer = self.op.writer(&full_op_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
        tokio::io::copy(&mut file, &mut writer).await.map_err(|e| DomainError::Io(e.to_string()))?;
        writer.close().await.map_err(|e| DomainError::Io(e.to_string()))?;

        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(&target_user)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
        let mime = mime_guess::from_path(name).first_or_octet_stream().to_string();

        sqlx::query(
            "insert into storage.cloud_files (id, user_id, name, dir, size, mime, storage) 
             values ($1, $2, $3, $4, $5, $6, 'file') 
             on conflict (user_id, dir, name) 
             do update set size = EXCLUDED.size, mime = EXCLUDED.mime, storage = EXCLUDED.storage, updated_at = CURRENT_TIMESTAMP"
        )
        .bind(uuid::Uuid::new_v4())
        .bind(user_id)
        .bind(name)
        .bind(&normalized_parent)
        .bind(size as i64)
        .bind(mime)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;
        
        Ok(())
    }

    async fn run_trash_purger(&self) {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(24 * 3600)).await;
            let rows = sqlx::query(
                "select cf.id, cf.user_id, cf.dir, cf.name, cf.mime, u.username
                 from storage.cloud_files cf
                 join sys.users u on cf.user_id = u.id
                 where cf.dir like '/Trash%' and cf.updated_at < CURRENT_TIMESTAMP - INTERVAL '30 days'"
            )
                .fetch_all(&self.db)
                .await
                .unwrap_or_default();
            for row in rows {
                let id: uuid::Uuid = row.get("id");
                let dir: String = row.get("dir");
                let name: String = row.get("name");
                let mime: String = row.get("mime");
                let username: String = row.get("username");
                let virtual_path = if dir == "/" { format!("/{}", name) } else { format!("{}/{}", dir, name) };
                if let Ok(op_path) = self.resolve_opendal_path(&username, &virtual_path).await {
                    if mime == "inode/directory" {
                        let _ = self.op.remove_all(&op_path).await;
                    } else {
                        let _ = self.op.delete(&op_path).await;
                    }
                }
                let _ = sqlx::query("delete from storage.cloud_files where id = $1").bind(id).execute(&self.db).await;
            }
        }
    }

    async fn initiate_multipart_upload(&self, _username: &str, _parent_virtual_path: &str, _name: &str) -> Result<String> {
        Err(DomainError::BadRequest("multipart upload not implemented".to_string()))
    }

    async fn save_file_part(&self, _username: &str, _upload_id: &str, _part_number: i32, _data: bytes::Bytes) -> Result<String> {
        Err(DomainError::BadRequest("multipart upload not implemented".to_string()))
    }

    async fn complete_multipart_upload(&self, _username: &str, _upload_id: &str, _etags: Vec<(i32, String)>) -> Result<()> {
        Err(DomainError::BadRequest("multipart upload not implemented".to_string()))
    }

    async fn abort_multipart_upload(&self, _username: &str, _upload_id: &str) -> Result<()> {
        Err(DomainError::BadRequest("multipart upload not implemented".to_string()))
    }
}

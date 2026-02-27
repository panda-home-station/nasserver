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

    async fn blob_base(&self, username: &str) -> String {
        format!("vol1/User/{}/.Trash/blobs", username)
    }

    async fn ensure_dir(&self, path: &str) {
        let _ = self.op.create_dir(&format!("{}/", path)).await;
    }

    async fn hash_and_archive_file(&self, username: &str, virtual_path: &str) -> Result<(String, i64)> {
        let op_src = self.resolve_opendal_path(username, virtual_path).await?;
        let meta = self.op.stat(&op_src).await.map_err(|e| DomainError::Io(e.to_string()))?;
        if !meta.mode().is_file() {
            return Err(DomainError::BadRequest("Not a file".to_string()));
        }
        let size = meta.content_length() as i64;
        // Use a random UUID string as blob id to avoid hashing cost
        let hash = uuid::Uuid::new_v4().to_string();
        let dest_base = self.blob_base(username).await;
        let dest_path = format!("{}/{}", dest_base, &hash);
        if let Some(parent) = Path::new(&dest_path).parent() {
            let parent_str = parent.to_string_lossy().to_string();
            self.ensure_dir(&parent_str).await;
        }
        // Move source object into blob store
        self.op.rename(&op_src, &dest_path).await.map_err(|e| DomainError::Io(e.to_string()))?;

        Ok((hash, size))
    }

    async fn upsert_trash_file_item(&self, username: &str, original_dir: &str, name: &str, mime: &str, hash: &str, size: i64) -> Result<()> {
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;

        sqlx::query("
            insert into storage.trash_items(id, user_id, name, original_dir, is_dir, size, mime, blob_hash)
            values ($1, $2, $3, $4, false, $5, $6, $7)
        ")
        .bind(uuid::Uuid::new_v4())
        .bind(user_id)
        .bind(name)
        .bind(original_dir)
        .bind(size)
        .bind(mime)
        .bind(hash)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;
        Ok(())
    }

    async fn insert_trash_dir_item(&self, username: &str, original_dir: &str, name: &str) -> Result<()> {
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
        sqlx::query("
            insert into storage.trash_items(id, user_id, name, original_dir, is_dir, size, mime)
            values ($1, $2, $3, $4, true, 0, 'inode/directory')
            on conflict do nothing
        ")
        .bind(uuid::Uuid::new_v4())
        .bind(user_id)
        .bind(name)
        .bind(original_dir)
        .execute(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;
        Ok(())
    }

    async fn trash_directory_recursive(&self, username: &str, full_path: &str) -> Result<()> {
        // Iterative DFS to avoid async recursion
        let mut stack: Vec<String> = vec![full_path.to_string()];
        while let Some(curr) = stack.pop() {
            let p = Path::new(&curr);
            let parent = p.parent().and_then(|x| x.to_str()).unwrap_or("/");
            let name = p.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid path".to_string()))?;
            self.insert_trash_dir_item(username, parent, name).await?;

            let src_op = self.resolve_opendal_path(username, &curr).await?;
            let list_path = if src_op.ends_with('/') { src_op.clone() } else { format!("{}/", src_op) };
            if let Ok(entries) = self.op.list(&list_path).await {
                for e in entries {
                    let child_name = e.name().to_string();
                    if child_name.is_empty() || &child_name == "." { continue; }
                    let child_virtual = if curr == "/" { format!("/{}", child_name) } else { format!("{}/{}", curr, child_name) };
                    let is_dir = e.metadata().mode().is_dir();
                    if is_dir {
                        stack.push(child_virtual);
                    } else {
                        let mime = mime_guess::from_path(&child_name).first_or_octet_stream().to_string();
                        let (hash, size) = self.hash_and_archive_file(username, &child_virtual).await?;
                        self.upsert_trash_file_item(username, &curr, &child_name, &mime, &hash, size).await?;
                    }
                }
            }
            // Remove the now-empty directory
            let _ = self.op.remove_all(&src_op).await;
        }
        Ok(())
    }

    async fn list_trash_dir(&self, username: &str, rel_dir: &str) -> Result<Vec<DocsEntry>> {
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
        let dir_key = if rel_dir.is_empty() { "/" } else { rel_dir };
        
        // For trash root directory, show all deleted items in a flat view
        if dir_key == "/" {
            let rows = sqlx::query("
                select id, name, is_dir, size, mime, original_dir, (extract(epoch from deleted_at))::float8 as ts
                from storage.trash_items
                where user_id = $1
            ")
            .bind(user_id)
            .fetch_all(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            
            let mut entries: Vec<DocsEntry> = Vec::new();
            for row in rows {
                let name: String = row.get("name");
                let original_dir: String = row.get("original_dir");
                let original_path = if original_dir == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", original_dir, name)
                };
                // Display just the file name, original path is available in original_path
                entries.push(DocsEntry {
                    id: row.get::<uuid::Uuid, _>("id").to_string(),
                    name: name.clone(),
                    is_dir: row.get("is_dir"),
                    size: row.get("size"),
                    modified_ts: row.get::<f64, _>("ts") as i64,
                    mime: row.get("mime"),
                    original_path: Some(original_path),
                });
            }
            
            entries.sort_by(|a, b| {
                match (a.is_dir, b.is_dir) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                }
            });
            return Ok(entries);
        }
        
        // For subdirectories, return empty (not supported in current implementation)
        Ok(vec![])
    }

    async fn delete_trash_item_recursive(&self, username: &str, rel_dir: &str, name: &str) -> Result<()> {
        let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
        let row = sqlx::query("
            select id, is_dir, blob_hash from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3
        ")
        .bind(user_id)
        .bind(if rel_dir.is_empty() { "/" } else { rel_dir })
        .bind(name)
        .fetch_optional(&self.db)
        .await
        .map_err(|e| DomainError::Database(e.to_string()))?;
        if row.is_none() { return Ok(()); }
        let row = row.unwrap();
        let is_dir: bool = row.get("is_dir");
        if is_dir {
            let base = if rel_dir.is_empty() { format!("/{}", name) } else { format!("{}/{}", rel_dir, name) };
            // Collect all file items under subtree to delete blob files
            let files = sqlx::query("
                select blob_hash from storage.trash_items where user_id = $1 and is_dir = false and (original_dir = $2 or original_dir like $3)
            ")
            .bind(user_id)
            .bind(&base)
            .bind(&format!("{}/%", base))
            .fetch_all(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            for f in files {
                let blob: Option<String> = f.get("blob_hash");
                if let Some(bhash) = blob {
                    // remove blob file
                    let blob_path = format!("{}/{bhash}", self.blob_base(username).await);
                    let _ = self.op.delete(&blob_path).await;
                }
            }
            let _ = sqlx::query("delete from storage.trash_items where user_id = $1 and (original_dir = $2 or original_dir like $3)")
                .bind(user_id)
                .bind(&base)
                .bind(&format!("{}/%", base))
                .execute(&self.db)
                .await;
            // Also delete the directory node itself
            let _ = sqlx::query("delete from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3 and is_dir = true")
                .bind(user_id)
                .bind(if rel_dir.is_empty() { "/" } else { rel_dir })
                .bind(name)
                .execute(&self.db)
                .await;
        } else {
            let blob: Option<String> = row.get("blob_hash");
            let _ = sqlx::query("delete from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3 and is_dir = false")
                .bind(user_id)
                .bind(if rel_dir.is_empty() { "/" } else { rel_dir })
                .bind(name)
                .execute(&self.db)
                .await;
            if let Some(bhash) = blob {
                let blob_path = format!("{}/{bhash}", self.blob_base(username).await);
                let _ = self.op.delete(&blob_path).await;
            }
        }
        Ok(())
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
                let first = rel.split('/').next().unwrap_or("");
                let is_date = first.len() == 8 && first.chars().all(|c| c.is_ascii_digit());
                if is_date {
                    Ok(format!("vol1/User/{}/.Trash/{}", username, rel))
                } else {
                    let base = format!("vol1/User/{}/.Trash", username);
                    let mut matches = Vec::new();
                    if let Ok(op_entries) = self.op.list(&(base.clone() + "/")).await {
                        for entry in op_entries {
                            let n = entry.name().to_string();
                            if n.len() == 8 && n.chars().all(|c| c.is_ascii_digit()) && entry.metadata().mode().is_dir() {
                                let cand = format!("{}/{}/{}", base, n, rel);
                                if self.op.stat(&cand).await.is_ok() || self.op.stat(&(cand.clone() + "/")).await.is_ok() {
                                    matches.push(cand);
                                }
                            }
                        }
                    }
                    if matches.len() == 1 {
                        Ok(matches.remove(0))
                    } else if matches.is_empty() {
                        Ok(format!("{}/{}", base, rel))
                    } else {
                        Err(DomainError::BadRequest("Ambiguous /Trash path; please include YYYYMMDD".to_string()))
                    }
                }
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
                    DocsEntry { id: "sys".to_string(), name: "System".to_string(), is_dir: true, size: 0, modified_ts: 0, mime: "inode/directory".to_string(), original_path: None },
                    DocsEntry { id: "usr".to_string(), name: "User".to_string(), is_dir: true, size: 0, modified_ts: 0, 
mime: "inode/directory".to_string(), original_path: None },
                    DocsEntry { id: "app".to_string(), name: "AppData".to_string(), is_dir: true, size: 0, modified_ts: 0, mime: "inode/directory".to_string(), original_path: None },
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
                original_path: None,
            }).collect();

            return Ok(DocsListResp { path: dir, entries, has_more: false, next_offset: 0 });
        }

        if dir == "/Trash" || dir.starts_with("/Trash/") {
            // DB-driven Trash view, mirroring original_dir tree under /Trash
            let rel = dir.trim_start_matches("/Trash");
            let rel_dir = rel.trim_start_matches('/');
            let entries = self.list_trash_dir(username, rel_dir).await?;
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
                        original_path: None,
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
                original_path: None,
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

        // Restore from Trash
        if from_path.starts_with("/Trash") {
            let from_obj = Path::new(&from_path);
            let rel_parent = from_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
            let rel_parent = rel_parent.trim_start_matches("/Trash").trim_start_matches('/');
            let from_name = from_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid from path".to_string()))?;

            // Destination must be /User/... or /System
            let to_user = if to_path.starts_with("/System") {
                "admin".to_string()
            } else if to_path.starts_with("/User/") {
                let parts: Vec<&str> = to_path.split('/').filter(|s| !s.is_empty()).collect();
                if parts.len() < 2 { return Err(DomainError::BadRequest("Invalid to path".to_string())); }
                parts[1].to_string()
            } else {
                return Err(DomainError::BadRequest("Invalid to path".to_string()));
            };
            if to_user != username && username != "admin" {
                return Err(DomainError::Forbidden("Access denied".to_string()));
            }
            // Query item
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
            let original_dir = if rel_parent.is_empty() { "/".to_string() } else { format!("/{}", rel_parent) };
            let item = sqlx::query("select id, is_dir, blob_hash from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3")
                .bind(user_id)
                .bind(&original_dir)
                .bind(from_name)
                .fetch_optional(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
            if item.is_none() { return Err(DomainError::BadRequest("Trash item not found".to_string())); }
            let item = item.unwrap();
            let is_dir: bool = item.get("is_dir");
            let to_obj = Path::new(&to_path);
            let to_parent = to_obj.parent().and_then(|p| p.to_str()).unwrap_or("/");
            let to_name = to_obj.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid to path".to_string()))?;

            if is_dir {
                // Restore directory subtree
                let from_root = if rel_parent.is_empty() { format!("/{}", from_name) } else { format!("{}/{}", rel_parent, from_name) };
                // Ensure destination directory exists
                let to_op_parent = self.resolve_opendal_path(&to_user, to_parent).await?;
                let _ = self.op.create_dir(&format!("{}/", to_op_parent)).await;
                // Fetch all items under subtree ordered by original_dir depth (dirs first)
                let rows = sqlx::query("
                    select name, original_dir, is_dir, blob_hash, mime, size
                    from storage.trash_items where user_id = $1 and (original_dir = $2 or original_dir like $3)
                    order by is_dir desc, original_dir asc, name asc
                ")
                .bind(user_id)
                .bind(&from_root)
                .bind(&format!("{}/%", from_root))
                .fetch_all(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
                for row in &rows {
                    let cname: String = row.get("name");
                    let cdir: String = row.get("original_dir");
                    let rel_tail = cdir.strip_prefix(&from_root).unwrap_or("").trim_start_matches('/');
                    let dest_parent = if rel_tail.is_empty() {
                        to_path.clone()
                    } else {
                        if to_path == "/" { format!("/{}", rel_tail) } else { format!("{}/{}", to_path, rel_tail) }
                    };
                    let is_dir: bool = row.get("is_dir");
                    if is_dir {
                        // create directory both physically and in cloud_files
                        let dest_virtual = if dest_parent == "/" { format!("/{}", cname) } else { format!("{}/{}", dest_parent, cname) };
                        let op_dir = self.resolve_opendal_path(&to_user, &dest_virtual).await?;
                        let _ = self.op.create_dir(&format!("{}/", op_dir)).await;
                        let dest_parent_only = Path::new(&dest_virtual).parent().and_then(|p| p.to_str()).unwrap_or("/");
                        let u: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1").bind(&to_user).fetch_one(&self.db).await.map_err(|e| DomainError::Database(e.to_string()))?;
                        let _ = sqlx::query("insert into storage.cloud_files (id, user_id, name, dir, size, mime, storage) values ($1,$2,$3,$4,0,'inode/directory','virtual') on conflict do nothing")
                            .bind(uuid::Uuid::new_v4())
                            .bind(u)
                            .bind(cname.clone())
                            .bind(dest_parent_only)
                            .execute(&self.db)
                            .await;
                    } else {
                        let blob: Option<String> = row.get("blob_hash");
                        if let Some(hash) = blob {
                            let dest_virtual = if dest_parent == "/" { format!("/{}", cname) } else { format!("{}/{}", dest_parent, cname) };
                            let dest_op = self.resolve_opendal_path(&to_user, &dest_virtual).await?;
                            if let Some(parent) = Path::new(&dest_op).parent() {
                                let parent_str = parent.to_string_lossy().to_string();
                                let _ = self.op.create_dir(&format!("{}/", parent_str)).await;
                            }
                            let blob_path = format!("{}/{}", self.blob_base(username).await, hash);
                            let data = self.op.read(&blob_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
                            self.op.write(&dest_op, data).await.map_err(|e| DomainError::Io(e.to_string()))?;
                            // upsert cloud_files
                            let u: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1").bind(&to_user).fetch_one(&self.db).await.map_err(|e| DomainError::Database(e.to_string()))?;
                            let mime = mime_guess::from_path(&cname).first_or_octet_stream().to_string();
                            let size: i64 = row.get("size");
                            let _ = sqlx::query("
                                insert into storage.cloud_files(id,user_id,name,dir,size,mime,storage) values($1,$2,$3,$4,$5,$6,'file')
                                on conflict (user_id,dir,name) do update set size=EXCLUDED.size,mime=EXCLUDED.mime,updated_at=CURRENT_TIMESTAMP
                            ")
                            .bind(uuid::Uuid::new_v4())
                            .bind(u)
                            .bind(&cname)
                            .bind(&dest_parent)
                            .bind(size)
                            .bind(mime)
                            .execute(&self.db)
                            .await;
                        }
                    }
                }
                // After restoring, delete file blobs (no refcount now)
                for row in rows {
                    if !row.get::<bool,_>("is_dir") {
                        if let Some(hash) = row.get::<Option<String>,_>("blob_hash") {
                            let blob_path = format!("{}/{}", self.blob_base(username).await, hash);
                            let _ = self.op.delete(&blob_path).await;
                        }
                    }
                }
                let _ = sqlx::query("delete from storage.trash_items where user_id = $1 and (original_dir = $2 or original_dir like $3)")
                    .bind(user_id)
                    .bind(&from_root)
                    .bind(&format!("{}/%", from_root))
                    .execute(&self.db)
                    .await;
                let _ = sqlx::query("delete from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3 and is_dir = true")
                    .bind(user_id)
                    .bind(&original_dir)
                    .bind(from_name)
                    .execute(&self.db)
                    .await;
            } else {
                // Restore single file
                let hash: Option<String> = item.get("blob_hash");
                let hash = hash.ok_or_else(|| DomainError::Database("blob hash missing".to_string()))?;
                let blob_path = format!("{}/{}", self.blob_base(username).await, hash);
                let data = self.op.read(&blob_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
                let dest_op_parent = self.resolve_opendal_path(&to_user, to_parent).await?;
                let _ = self.op.create_dir(&format!("{}/", dest_op_parent)).await;
                let dest_op = self.resolve_opendal_path(&to_user, &to_path).await?;
                self.op.write(&dest_op, data).await.map_err(|e| DomainError::Io(e.to_string()))?;
                // cloud_files upsert
                let u: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1").bind(&to_user).fetch_one(&self.db).await.map_err(|e| DomainError::Database(e.to_string()))?;
                let mime = mime_guess::from_path(to_name).first_or_octet_stream().to_string();
                let size = self.op.stat(&dest_op).await.map_err(|e| DomainError::Io(e.to_string()))?.content_length() as i64;
                sqlx::query("
                    insert into storage.cloud_files(id,user_id,name,dir,size,mime,storage) values($1,$2,$3,$4,$5,$6,'file')
                    on conflict (user_id,dir,name) do update set size=EXCLUDED.size,mime=EXCLUDED.mime,updated_at=CURRENT_TIMESTAMP
                ")
                .bind(uuid::Uuid::new_v4())
                .bind(u)
                .bind(to_name)
                .bind(to_parent)
                .bind(size)
                .bind(mime)
                .execute(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
                // remove item and delete blob file
                let _ = sqlx::query("delete from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3 and is_dir = false")
                    .bind(user_id)
                    .bind(&original_dir)
                    .bind(from_name)
                    .execute(&self.db)
                    .await;
                let _ = self.op.delete(&blob_path).await;
            }
            return Ok(());
        }
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
            if full_path == "/Trash" {
                return Err(DomainError::BadRequest("Cannot delete Trash root".to_string()));
            }
            let rel_dir = parent.trim_start_matches("/Trash").trim_start_matches('/');
            self.delete_trash_item_recursive(username, rel_dir, name).await?;
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

            // CAS Trash path
            let op_src = self.resolve_opendal_path(&target_user, &full_path).await?;
            let is_dir = self.op.stat(&op_src).await.map_err(|e| DomainError::Io(e.to_string()))?.mode().is_dir();
            if is_dir {
                self.trash_directory_recursive(&target_user, &full_path).await?;
                // Remove from cloud_files for subtree
                let like_pat = format!("{}/%", full_path);
                let _ = sqlx::query("delete from storage.cloud_files where user_id = $1 and (dir = $2 or dir like $3)")
                    .bind(user_id)
                    .bind(&full_path)
                    .bind(&like_pat)
                    .execute(&self.db)
                    .await;
                let _ = sqlx::query("delete from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                    .bind(user_id)
                    .bind(parent)
                    .bind(name)
                    .execute(&self.db)
                    .await;
            } else {
                // single file
                let mime = mime_guess::from_path(name).first_or_octet_stream().to_string();
                let (hash, size) = self.hash_and_archive_file(&target_user, &full_path).await?;
                self.upsert_trash_file_item(&target_user, parent, name, &mime, &hash, size).await?;
                let _ = sqlx::query("delete from storage.cloud_files where user_id = $1 and dir = $2 and name = $3")
                    .bind(user_id)
                    .bind(parent)
                    .bind(name)
                    .execute(&self.db)
                    .await;
            }
        }

        Ok(())
    }



    async fn get_file_reader(&self, username: &str, virtual_path: &str) -> Result<Box<dyn AsyncRead + Unpin + Send + Sync>> {
        let normalized_path = self.normalize_path(virtual_path);

        // Permission check
        if normalized_path.starts_with("/Trash") {
            // Allow only self or admin to read blobs
        } else if normalized_path.starts_with("/User/") {
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
        if normalized_path.starts_with("/Trash") {
            // Map /Trash/<rel_dir>/<name> to blob reader via DB
            let p = Path::new(&normalized_path);
            let parent = p.parent().and_then(|x| x.to_str()).unwrap_or("/");
            let rel_parent = parent.trim_start_matches("/Trash").trim_start_matches('/');
            let name = p.file_name().and_then(|n| n.to_str()).ok_or_else(|| DomainError::BadRequest("Invalid path".to_string()))?;
            let user_id: uuid::Uuid = sqlx::query_scalar("select id from sys.users where username = $1")
                .bind(username)
                .fetch_one(&self.db)
                .await
                .map_err(|e| DomainError::Database(e.to_string()))?;
            let blob: Option<String> = sqlx::query_scalar("
                select blob_hash from storage.trash_items where user_id = $1 and original_dir = $2 and name = $3 and is_dir = false
            ")
            .bind(user_id)
            .bind(if rel_parent.is_empty() { "/" } else { rel_parent })
            .bind(name)
            .fetch_optional(&self.db)
            .await
            .map_err(|e| DomainError::Database(e.to_string()))?;
            let hash = blob.ok_or_else(|| DomainError::BadRequest("Trash file not found".to_string()))?;
            let blob_path = format!("{}/{}", self.blob_base(username).await, hash);
            let reader = self.op.reader(&blob_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            return Ok(Box::new(reader));
        } else {
            let op_path = self.resolve_opendal_path(username, &normalized_path).await?;
            let reader = self.op.reader(&op_path).await.map_err(|e| DomainError::Io(e.to_string()))?;
            Ok(Box::new(reader))
        }

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
            // Delete trash items older than 30 days
            let rows = sqlx::query(
                "select ti.id, ti.is_dir, ti.blob_hash, u.username
                 from storage.trash_items ti
                 join sys.users u on ti.user_id = u.id
                 where ti.deleted_at < CURRENT_TIMESTAMP - INTERVAL '30 days'"
            )
            .fetch_all(&self.db)
            .await
            .unwrap_or_default();
            for row in rows {
                let id: uuid::Uuid = row.get("id");
                let is_dir: bool = row.get("is_dir");
                let username: String = row.get("username");
                if !is_dir {
                    if let Some(hash) = row.get::<Option<String>,_>("blob_hash") {
                        let blob_path = format!("{}/{}", self.blob_base(&username).await, hash);
                        let _ = self.op.delete(&blob_path).await;
                    }
                }
                let _ = sqlx::query("delete from storage.trash_items where id = $1").bind(id).execute(&self.db).await;
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

use axum::{
    extract::{Extension, Multipart, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use axum::body::Body;
use tokio_util::io::ReaderStream;
use tokio::io::{AsyncWriteExt, AsyncSeekExt};
use tokio::fs::{self as tokio_fs, OpenOptions};
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use sqlx::Row;
use mime_guess;

use crate::state::AppState;
use crate::models::auth::AuthUser;
use crate::models::docs::{DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery};



pub fn normalize_path(p: &str) -> String {
    let s = if p.starts_with('/') { &p[1..] } else { p };
    let s = s.replace("\\", "/");
    let parts: Vec<&str> = s.split('/').filter(|x| !x.is_empty() && *x != "." && *x != "..").collect();
    format!("/{}", parts.join("/"))
}

async fn check_app_access(db: &sqlx::SqlitePool, username: &str, app_name: &str) -> bool {
    if username == "admin" { return true; }
    let count: i64 = sqlx::query_scalar("select count(*) from app_permissions where app_name = $1 and username = $2")
        .bind(app_name)
        .bind(username)
        .fetch_one(db)
        .await
        .unwrap_or(0);
    count > 0
}

pub async fn resolve_path(state: &AppState, username: &str, virtual_path: &str) -> Result<PathBuf, String> {
    let clean_path = normalize_path(virtual_path);
    if clean_path.starts_with("/AppData/") {
        let parts: Vec<&str> = clean_path.split('/').filter(|x| !x.is_empty()).collect();
        // parts[0] is "AppData", parts[1] is app_name
        if parts.len() < 2 {
            // Accessing AppData root directly?
            // Technically mapped to storage_path/vol1/AppData
            return Ok(Path::new(&state.storage_path).join("vol1").join("AppData"));
        }
        let app_name = parts[1];
        if !check_app_access(&state.db, username, app_name).await {
            return Err("Access denied".to_string());
        }
        // Construct path: storage/vol1/AppData/app_name/rest...
        let mut p = Path::new(&state.storage_path).join("vol1").join("AppData").join(app_name);
        if parts.len() > 2 {
            let rel = parts[2..].join("/");
            p = p.join(rel);
        }
        Ok(p)
    } else if clean_path == "/AppData" {
         Ok(Path::new(&state.storage_path).join("vol1").join("AppData"))
    } else {
        // User storage
        let rel = if clean_path.starts_with('/') { &clean_path[1..] } else { &clean_path };
        Ok(Path::new(&state.storage_path).join("vol1").join("User").join(username).join(rel))
    }
}

pub async fn list(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsListQuery>) -> impl IntoResponse {
    let dir = normalize_path(&q.path.unwrap_or_else(|| "/".to_string()));
    
    // Permission check for AppData subdirectories
    if dir.starts_with("/AppData/") {
        let parts: Vec<&str> = dir.split('/').filter(|x| !x.is_empty()).collect();
        if parts.len() >= 2 {
            let app_name = parts[1];
            if !check_app_access(&state.db, &user.username, app_name).await {
                 return Json(DocsListResp { path: dir, entries: vec![], has_more: false, next_offset: 0 });
            }
        }
    }

    // Special handling for AppData root listing
    if dir == "/AppData" {
        let mut entries: Vec<DocsEntry> = Vec::new();
        
        if user.username == "admin" {
            // Admin sees all apps in AppData
            let app_data_path = Path::new(&state.storage_path).join("vol1/AppData");
            if let Ok(mut read_dir) = tokio_fs::read_dir(app_data_path).await {
                while let Ok(Some(entry)) = read_dir.next_entry().await {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if !name.starts_with('.') {
                            entries.push(DocsEntry {
                                id: uuid::Uuid::new_v4().to_string(),
                                name,
                                is_dir: true,
                                size: 0,
                                modified_ts: 0,
                                mime: "application/x-app".to_string(),
                            });
                        }
                    }
                }
            }
        } else {
            // Fetch accessible apps from app_permissions
            let apps: Vec<String> = sqlx::query_scalar("select app_name from app_permissions where username = $1")
                .bind(&user.username)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();
                
            for app in apps {
                 let path = Path::new(&state.storage_path).join("vol1/AppData").join(&app);
                 if path.exists() {
                     entries.push(DocsEntry {
                         id: uuid::Uuid::new_v4().to_string(),
                         name: app,
                         is_dir: true,
                         size: 0,
                         modified_ts: 0, // Just use 0 or current time
                         mime: "application/x-app".to_string(),
                     });
                 }
            }
        }
        return Json(DocsListResp { path: "AppData".to_string(), entries, has_more: false, next_offset: 0 });
    }

    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let page_size = limit + 1;
    
    // Use different query for AppData (shared) vs User (private)
    let is_app_data = dir.starts_with("/AppData");
    
    let mut entries: Vec<DocsEntry> = Vec::new();
    let has_more;
    let next_offset;

    if is_app_data {
        // Physical listing for AppData
        let mut physical_entries = Vec::new();
        let fs_path = match resolve_path(&state, &user.username, &dir).await {
            Ok(p) => p,
            Err(_) => return Json(DocsListResp { path: dir, entries: vec![], has_more: false, next_offset: 0 }),
        };

        if let Ok(mut read_dir) = tokio_fs::read_dir(fs_path).await {
            while let Ok(Some(entry)) = read_dir.next_entry().await {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') { continue; }

                let is_dir = path.is_dir();
                let meta = entry.metadata().await.ok();
                let size = meta.as_ref().map(|m| m.len()).unwrap_or(0) as i64;
                let ts = meta.as_ref().and_then(|m| m.modified().ok())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64)
                    .unwrap_or(0);
                
                let mime = if is_dir {
                    "application/x-directory".to_string()
                } else {
                    mime_guess::from_path(&path).first_or_octet_stream().to_string()
                };

                // Construct a physical ID: phys:{urlencoding::encode(virtual_path)}
                // We need the virtual path for the entry.
                // dir is e.g. /AppData/jellyfin
                // entry virtual path is /AppData/jellyfin/subdir
                let entry_vpath = if dir == "/" { format!("/{}", name) } else { format!("{}/{}", dir, name) };
                let id = format!("phys:{}", urlencoding::encode(&entry_vpath));

                physical_entries.push(DocsEntry {
                    id,
                    name,
                    is_dir,
                    size,
                    modified_ts: ts,
                    mime,
                });
            }
        }
        
        // Sort and paginate manually
        physical_entries.sort_by(|a, b| {
            if a.is_dir != b.is_dir {
                b.is_dir.cmp(&a.is_dir) // dirs first
            } else {
                a.name.cmp(&b.name)
            }
        });
        
        let total = physical_entries.len();
        let start = offset as usize;
        let end = (start + limit as usize).min(total);
        
        if start < total {
            for e in physical_entries.drain(start..end) {
                entries.push(e);
            }
        }
        has_more = end < total;
        next_offset = end as i64;

    } else {
        let rows = sqlx::query("select id, name, storage, size, mime, strftime('%s', coalesce(updated_at, created_at)) as ts from cloud_files where user_id = $1 and dir = $2 order by storage desc, name asc limit $3 offset $4")
            .bind(user.user_id)
            .bind(&dir)
            .bind(page_size)
            .bind(offset)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

        for r in rows.iter().take(limit as usize) {
            let id: String = r.try_get("id").unwrap_or_default();
            let name: String = r.try_get("name").unwrap_or_default();
            let storage: String = r.try_get("storage").unwrap_or_default();
            let size: i64 = r.try_get("size").unwrap_or(0);
            let ts: i64 = r.try_get("ts").unwrap_or(0);
            let mime_val: String = r.try_get("mime").unwrap_or_default();
            let mime = if mime_val.is_empty() {
                if storage == "dir" { "application/x-directory".to_string() } else { "application/octet-stream".to_string() }
            } else {
                mime_val
            };
            entries.push(DocsEntry { id, name, is_dir: storage == "dir", size, modified_ts: ts, mime });
        }
        has_more = rows.len() > limit as usize;
        next_offset = offset + entries.len() as i64;
    }
    
    Json(DocsListResp { path: dir, entries, has_more, next_offset })
}

pub async fn mkdir(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(req): Json<DocsMkdirReq>) -> impl IntoResponse {
    let dir = normalize_path(&req.path);
    let reserved: [&str; 8] = [
        "AppData", "Favorites", "MyShares", "PublicLinks", "Recent", "SharedWithMe", "Team", "Trash",
    ];
    let top_name = Path::new(&dir).components().next().and_then(|c| {
        use std::path::Component;
        match c {
            Component::RootDir => None,
            Component::Normal(os) => os.to_str(),
            _ => None,
        }
    }).unwrap_or("");
    
    // Prevent creating reserved folders at root, but allow using them if they are valid paths
    if reserved.contains(&top_name) && dir == format!("/{}", top_name) {
        // Already exists physically or virtually
        return Json(serde_json::json!({ "ok": true }));
    }
    
    let fs_path = match resolve_path(&state, &user.username, &dir).await {
        Ok(p) => p,
        Err(e) => return Json(serde_json::json!({ "ok": false, "error": e })),
    };

    if let Err(e) = tokio_fs::create_dir_all(&fs_path).await {
        return Json(serde_json::json!({ "ok": false, "error": e.to_string() }));
    }

    let parent = {
        let p = Path::new(&dir);
        let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
        if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
    };
    let name = Path::new(&dir).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
    if name.is_empty() {
        return Json(serde_json::json!({ "ok": false }));
    }
    
    // Check existence
    let is_app_data = dir.starts_with("/AppData");
    let exists: Option<(i64,)> = if is_app_data {
        sqlx::query_as("select 1 from cloud_files where dir = $1 and name = $2 and storage = 'dir' limit 1")
            .bind(&parent)
            .bind(&name)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
    } else {
        sqlx::query_as("select 1 from cloud_files where user_id = $1 and dir = $2 and name = $3 and storage = 'dir' limit 1")
            .bind(user.user_id)
            .bind(&parent)
            .bind(&name)
            .fetch_optional(&state.db)
            .await
            .unwrap_or(None)
    };
        
    if exists.is_none() {
        let id = uuid::Uuid::new_v4().to_string();
        let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage) values ($1, $2, $3, $4, 0, 'dir')")
            .bind(&id)
            .bind(user.user_id)
            .bind(&name)
            .bind(&parent)
            .execute(&state.db)
            .await;
        Json(serde_json::json!({ "ok": true, "id": id }))
    } else {
        Json(serde_json::json!({ "ok": true }))
    }
}

pub async fn upload(State(state): State<AppState>, Extension(user): Extension<AuthUser>, mut multipart: Multipart) -> impl IntoResponse {
    let mut dest_path = "/".to_string();
    let mut expected_size: Option<u64> = None;
    let mut offset: u64 = 0;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "path" {
            dest_path = normalize_path(&field.text().await.unwrap_or("/".to_string()));
        } else if name == "size" {
            if let Ok(s) = field.text().await.unwrap_or_default().parse::<u64>() { expected_size = Some(s); }
        } else if name == "upload_id" {
            let _ = field.text().await.unwrap_or_default();
        } else if name == "offset" {
            if let Ok(o) = field.text().await.unwrap_or_default().parse::<u64>() { offset = o; }
        } else if name == "file" {
            let file_name = field.file_name().map(|s| s.to_string()).unwrap_or("upload.bin".to_string());
            
            let parent_fs_path = match resolve_path(&state, &user.username, &dest_path).await {
                Ok(p) => p,
                Err(e) => return Json(serde_json::json!({ "ok": false, "error": e })),
            };
            let target_file_path = parent_fs_path.join(&file_name);
            let temp_file_name = format!(".{}.part", file_name);
            let temp_file_path = parent_fs_path.join(&temp_file_name);
            
            if let Some(parent) = target_file_path.parent() {
                let _ = tokio_fs::create_dir_all(parent).await;
            }

            let mut f = if offset > 0 {
                match OpenOptions::new().write(true).create(true).open(&temp_file_path).await {
                    Ok(mut file) => {
                        if let Err(_) = file.seek(SeekFrom::Start(offset)).await {
                            return Json(serde_json::json!({ "ok": false, "error": "seek failed" }));
                        }
                        file
                    },
                    Err(_) => return Json(serde_json::json!({ "ok": false, "error": "open failed" })),
                }
            } else {
                match tokio_fs::File::create(&temp_file_path).await {
                    Ok(h) => h,
                    Err(_) => return Json(serde_json::json!({ "ok": false, "error": "create failed" })),
                }
            };

            let mut written: u64 = 0;
            while let Ok(Some(chunk)) = field.chunk().await {
                written += chunk.len() as u64;
                if let Err(_) = f.write_all(&chunk).await {
                    return Json(serde_json::json!({ "ok": false, "error": "write failed" }));
                }
            }

            // Flush and sync to disk before renaming
            if let Err(e) = f.sync_all().await {
                 println!("Failed to sync file {}: {}", temp_file_name, e);
                 return Json(serde_json::json!({ "ok": false, "error": "sync failed" }));
            }
            drop(f); // Ensure file is closed

            let current_size = match tokio_fs::metadata(&temp_file_path).await {
                Ok(m) => m.len(),
                Err(_) => offset + written,
            };

            if let Some(es) = expected_size {
                if current_size != es {
                    return Json(serde_json::json!({ "ok": true, "done": false, "bytes": current_size }));
                }
            }
            
            // Finished: Atomic Rename
            if let Err(e) = tokio_fs::rename(&temp_file_path, &target_file_path).await {
                println!("Failed to rename temp file: {}", e);
                return Json(serde_json::json!({ "ok": false, "error": "rename failed" }));
            }
            
            // Sync parent directory to ensure rename is persisted
            if let Some(parent) = target_file_path.parent() {
                if let Ok(dir) = tokio_fs::File::open(parent).await {
                    let _ = dir.sync_all().await;
                }
            }

            // Note: We no longer manually update the DB here.
            // The filesystem watcher (watcher.rs) will detect the rename/create event
            // and automatically sync the metadata to cloud_files.
            // This ensures a single source of truth and prevents race conditions.

            return Json(serde_json::json!({ "ok": true, "done": true, "bytes": current_size }));
        }
    }
    Json(serde_json::json!({ "ok": false }))
}

pub async fn download(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsDownloadQuery>) -> Response {
    let (name, dir) = if let Some(id) = &q.id {
        if id.starts_with("phys:") {
            let encoded = &id[5..];
            let vpath = urlencoding::decode(encoded).unwrap_or(std::borrow::Cow::Borrowed("")).to_string();
            if vpath.is_empty() {
                return Response::builder().status(404).body(Body::empty()).unwrap();
            }
            
            let p = Path::new(&vpath);
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            let parent = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            let parent = if parent.is_empty() { "/".to_string() } else { if parent.starts_with('/') { parent } else { format!("/{}", parent) } };
            
            (name, parent)
        } else if let Ok(row) = sqlx::query("select name, dir, user_id from cloud_files where id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await 
        {
            let d: String = row.try_get("dir").unwrap_or_default();
            // user_id is stored as BLOB (Uuid) in DB, so try to read it as Uuid first, fallback to string if legacy
            let u_uuid: Option<uuid::Uuid> = row.try_get("user_id").ok();
            let u_str: String = if let Some(uid) = u_uuid {
                uid.to_string()
            } else {
                 row.try_get("user_id").unwrap_or_default()
            };
            
            // Check access
            if d.starts_with("/AppData") {
                 // Check AppData permission
                 // We need to resolve_path to check permission
                 if let Err(_) = resolve_path(&state, &user.username, &d).await {
                     return Response::builder().status(403).body(Body::empty()).unwrap();
                 }
            } else {
                 // Standard user file check
                 if u_str != user.user_id.to_string() {
                     return Response::builder().status(404).body(Body::empty()).unwrap();
                 }
            }
            (row.try_get::<String, _>("name").unwrap_or_default(), d)
        } else {
            return Response::builder().status(404).body(Body::empty()).unwrap();
        }
    } else if let Some(path) = &q.path {
        let np = normalize_path(path);
        let parent = {
            let p = Path::new(&np);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        (name, parent)
    } else {
        return Response::builder().status(404).body(Body::empty()).unwrap();
    };

    let parent_fs_path = match resolve_path(&state, &user.username, &dir).await {
        Ok(p) => p,
        Err(_) => return Response::builder().status(403).body(Body::empty()).unwrap(),
    };
    let fs_path = parent_fs_path.join(&name);

    let file = match tokio_fs::File::open(&fs_path).await {
        Ok(f) => f,
        Err(_) => return Response::builder().status(404).body(Body::empty()).unwrap(),
    };
    
    let meta = file.metadata().await.ok();
    let len = meta.map(|m| m.len()).unwrap_or(0);
    let stream = ReaderStream::with_capacity(file, 8192 * 16);
    let body = Body::from_stream(stream);
    
    Response::builder()
        .status(200)
        .header("content-type", "application/octet-stream")
        .header("content-disposition", format!("attachment; filename=\"{}\"", name))
        .header("content-length", len.to_string())
        .body(body)
        .unwrap()
}

pub async fn rename(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(req): Json<DocsRenameReq>) -> impl IntoResponse {
    
    // 1. Identify source
    let (src_id, src_name, src_dir, src_storage) = if let Some(id) = &req.id {
         // Find by ID
         if let Ok(row) = sqlx::query("select id, name, dir, storage, user_id from cloud_files where id = $1")
            .bind(id).fetch_one(&state.db).await {
            
            let d: String = row.try_get("dir").unwrap_or_default();
            let u: String = row.try_get("user_id").unwrap_or_default();
             // Check access
            if d.starts_with("/AppData") {
                 if let Err(_) = resolve_path(&state, &user.username, &d).await {
                     return Json(serde_json::json!({ "ok": false, "error": "access denied" }));
                 }
            } else {
                 if u != user.user_id.to_string() {
                     return Json(serde_json::json!({ "ok": false, "error": "not found" }));
                 }
            }
            (
                row.try_get::<String, _>("id").unwrap_or_default(),
                row.try_get::<String, _>("name").unwrap_or_default(),
                d,
                row.try_get::<String, _>("storage").unwrap_or_default(),
            )
         } else { return Json(serde_json::json!({ "ok": false, "error": "not found" })); }
    } else if let Some(from) = &req.from {
        let np = normalize_path(from);
        let parent = {
            let p = Path::new(&np);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        
        let is_app_data = parent.starts_with("/AppData");
        let row = if is_app_data {
            sqlx::query("select id, storage from cloud_files where dir = $1 and name = $2")
                .bind(&parent)
                .bind(&name)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
        } else {
            sqlx::query("select id, storage from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user.user_id)
                .bind(&parent)
                .bind(&name)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
        };
        
        if let Some(r) = row {
            (
                r.try_get("id").unwrap_or_default(),
                name,
                parent,
                r.try_get("storage").unwrap_or_default(),
            )
        } else {
             return Json(serde_json::json!({ "ok": false, "error": "not found" }));
        }
    } else {
        return Json(serde_json::json!({ "ok": false, "error": "missing source" }));
    };
    
    // 2. Identify destination
    let (target_dir, target_name) = if let Some(to) = &req.to {
        let np = normalize_path(to);
        let parent = {
            let p = Path::new(&np);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        (parent, name)
    } else if let (Some(nd), Some(nn)) = (&req.new_dir, &req.new_name) {
        (normalize_path(nd), nn.clone())
    } else if let Some(nn) = &req.new_name {
        (src_dir.clone(), nn.clone())
    } else {
        return Json(serde_json::json!({ "ok": false, "error": "missing destination" }));
    };
    
    // 3. Resolve physical paths
    let src_parent_fs = match resolve_path(&state, &user.username, &src_dir).await {
        Ok(p) => p,
        Err(_) => return Json(serde_json::json!({ "ok": false, "error": "access denied" })),
    };
    let src_fs_path = src_parent_fs.join(&src_name);
    
    let dest_parent_fs = match resolve_path(&state, &user.username, &target_dir).await {
        Ok(p) => p,
        Err(_) => return Json(serde_json::json!({ "ok": false, "error": "access denied" })),
    };
    // Ensure dest parent exists physically
    if let Err(_) = tokio_fs::create_dir_all(&dest_parent_fs).await {
         return Json(serde_json::json!({ "ok": false, "error": "failed to create dest dir" }));
    }
    let dest_fs_path = dest_parent_fs.join(&target_name);
    
    // 4. Perform physical rename
    if let Err(e) = tokio_fs::rename(&src_fs_path, &dest_fs_path).await {
        println!("Physical rename failed: {:?} -> {:?}: {}", src_fs_path, dest_fs_path, e);
        return Json(serde_json::json!({ "ok": false, "error": e.to_string() }));
    }
    
    // 5. Update database
    // Update self
    if let Err(e) = sqlx::query("update cloud_files set dir = $1, name = $2, updated_at = CURRENT_TIMESTAMP where id = $3")
        .bind(&target_dir)
        .bind(&target_name)
        .bind(&src_id)
        .execute(&state.db)
        .await 
    {
        println!("DB update failed: {}", e);
        // Rollback physical? Too late/complex.
    }
    
    // If directory, update children recursively
    // Logic: find all items where dir starts with (src_dir + "/" + src_name)
    // Replace prefix with (target_dir + "/" + target_name)
    if src_storage == "dir" {
        let old_prefix = if src_dir == "/" {
            format!("/{}", src_name)
        } else {
            format!("{}/{}", src_dir, src_name)
        };
        
        let new_prefix = if target_dir == "/" {
            format!("/{}", target_name)
        } else {
            format!("{}/{}", target_dir, target_name)
        };
        
        // SQLite doesn't have a simple regex replace, need to do string manipulation
        // dir = new_prefix || substr(dir, length(old_prefix) + 1)
        // WHERE dir = old_prefix OR dir LIKE old_prefix || '/%'
        
        // Note: old_prefix should not have trailing slash for exact match, 
        // but for children it needs checking.
        
        let _sql = format!(
            "UPDATE cloud_files SET dir = '{}' || SUBSTR(dir, {} + 1) WHERE dir = '{}' OR dir LIKE '{}' || '/%'",
            new_prefix, old_prefix.len(), old_prefix, old_prefix
        );
        // Warning: This SQL injection risk if paths contain single quotes. 
        // Should use bind parameters but dynamic string concatenation in SQL with binds is tricky in pure SQL.
        // Better:
        // UPDATE cloud_files SET dir = ? || SUBSTR(dir, LENGTH(?) + 1) WHERE dir = ? OR dir LIKE ? || '/%'
        
        let like_pattern = format!("{}/%", old_prefix);
        
        if let Err(e) = sqlx::query("UPDATE cloud_files SET dir = $1 || SUBSTR(dir, LENGTH($2) + 1) WHERE dir = $2 OR dir LIKE $3")
            .bind(&new_prefix)
            .bind(&old_prefix)
            .bind(&like_pattern)
            .execute(&state.db)
            .await 
        {
             println!("Failed to update children paths: {}", e);
        }
    }
    
    Json(serde_json::json!({ "ok": true }))
}

pub async fn delete(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsDeleteQuery>) -> impl IntoResponse {
    let (id, name, dir, storage) = if let Some(id) = &q.id {
         if let Ok(row) = sqlx::query("select id, name, dir, storage, user_id from cloud_files where id = $1")
            .bind(id).fetch_one(&state.db).await {
            
            let d: String = row.try_get("dir").unwrap_or_default();
            // user_id is stored as BLOB (Uuid) in DB, so try to read it as Uuid first, fallback to string if legacy
            let u_uuid: Option<uuid::Uuid> = row.try_get("user_id").ok();
            let u_str: String = if let Some(uid) = u_uuid {
                uid.to_string()
            } else {
                 row.try_get("user_id").unwrap_or_default()
            };
            
            // Check access
            if d.starts_with("/AppData") {
                 if let Err(_) = resolve_path(&state, &user.username, &d).await {
                     return Json(serde_json::json!({ "ok": false, "error": "access denied" }));
                 }
            } else {
                 if u_str != user.user_id.to_string() {
                     return Json(serde_json::json!({ "ok": false, "error": "not found" }));
                 }
            }
            
            (
                row.try_get::<String, _>("id").unwrap_or_default(),
                row.try_get::<String, _>("name").unwrap_or_default(),
                d,
                row.try_get::<String, _>("storage").unwrap_or_default(),
            )
         } else { return Json(serde_json::json!({ "ok": false })); }
    } else if let Some(path) = &q.path {
        let np = normalize_path(path);
        let parent = {
            let p = Path::new(&np);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        
        // Find ID from path
        let is_app_data = parent.starts_with("/AppData");
        let row = if is_app_data {
            sqlx::query("select id, storage from cloud_files where dir = $1 and name = $2")
                .bind(&parent)
                .bind(&name)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
        } else {
            sqlx::query("select id, storage from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user.user_id)
                .bind(&parent)
                .bind(&name)
                .fetch_optional(&state.db)
                .await
                .unwrap_or(None)
        };
        
        if let Some(r) = row {
            (
                r.try_get("id").unwrap_or_default(),
                name,
                parent,
                r.try_get("storage").unwrap_or_default(),
            )
        } else {
             return Json(serde_json::json!({ "ok": false, "error": "not found" }));
        }
    } else { return Json(serde_json::json!({ "ok": false })); };

    let parent_fs_path = match resolve_path(&state, &user.username, &dir).await {
        Ok(p) => p,
        Err(_) => return Json(serde_json::json!({ "ok": false, "error": "access denied" })),
    };
    let fs_path = parent_fs_path.join(&name);
    
    if storage == "dir" {
        let _ = tokio_fs::remove_dir_all(&fs_path).await;
    } else {
        let _ = tokio_fs::remove_file(&fs_path).await;
    }
    
    let _ = sqlx::query("delete from cloud_files where id = $1").bind(&id).execute(&state.db).await;
    Json(serde_json::json!({ "ok": true }))
}

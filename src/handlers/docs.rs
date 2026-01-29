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
use serde::Deserialize;

use crate::state::AppState;
use crate::models::auth::AuthUser;
use crate::models::docs::{DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery};



fn normalize_path(p: &str) -> String {
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

async fn resolve_path(state: &AppState, username: &str, virtual_path: &str) -> Result<PathBuf, String> {
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

    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let page_size = limit + 1;
    
    // Use different query for AppData (shared) vs User (private)
    let is_app_data = dir.starts_with("/AppData");
    
    let rows = if is_app_data {
        sqlx::query("select id, name, storage, size, strftime('%s', coalesce(updated_at, created_at)) as ts from cloud_files where dir = $1 order by storage desc, name asc limit $2 offset $3")
            .bind(&dir)
            .bind(page_size)
            .bind(offset)
            .fetch_all(&state.db)
            .await
    } else {
        sqlx::query("select id, name, storage, size, strftime('%s', coalesce(updated_at, created_at)) as ts from cloud_files where user_id = $1 and dir = $2 order by storage desc, name asc limit $3 offset $4")
            .bind(user.user_id.to_string())
            .bind(&dir)
            .bind(page_size)
            .bind(offset)
            .fetch_all(&state.db)
            .await
    }.unwrap_or_default();

    let mut entries: Vec<DocsEntry> = Vec::new();
    for r in rows.iter().take(limit as usize) {
        let id: String = r.try_get("id").unwrap_or_default();
        let name: String = r.try_get("name").unwrap_or_default();
        let storage: String = r.try_get("storage").unwrap_or_default();
        let size: i64 = r.try_get("size").unwrap_or(0);
        let ts: i64 = r.try_get("ts").unwrap_or(0);
        entries.push(DocsEntry { id, name, is_dir: storage == "dir", size, modified_ts: ts });
    }
    let has_more = rows.len() > limit as usize;
    let next_offset = offset + entries.len() as i64;
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
            .bind(user.user_id.to_string())
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
            .bind(user.user_id.to_string())
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
            
            if let Some(parent) = target_file_path.parent() {
                let _ = tokio_fs::create_dir_all(parent).await;
            }

            let mut f = if offset > 0 {
                match OpenOptions::new().write(true).create(true).open(&target_file_path).await {
                    Ok(mut file) => {
                        if let Err(_) = file.seek(SeekFrom::Start(offset)).await {
                            return Json(serde_json::json!({ "ok": false, "error": "seek failed" }));
                        }
                        file
                    },
                    Err(_) => return Json(serde_json::json!({ "ok": false, "error": "open failed" })),
                }
            } else {
                match tokio_fs::File::create(&target_file_path).await {
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

            let current_size = match tokio_fs::metadata(&target_file_path).await {
                Ok(m) => m.len(),
                Err(_) => offset + written,
            };

            if let Some(es) = expected_size {
                if current_size != es {
                    return Json(serde_json::json!({ "ok": true, "done": false, "bytes": current_size }));
                }
            }
            
            // Finished
            let id = uuid::Uuid::new_v4().to_string();
            let parent = normalize_path(&dest_path);
            let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, storage) values ($1, $2, $3, $4, $5, '', 'file')")
                .bind(&id)
                .bind(user.user_id.to_string())
                .bind(&file_name)
                .bind(&parent)
                .bind(current_size as i64)
                .execute(&state.db)
                .await;

            return Json(serde_json::json!({ "ok": true, "done": true, "bytes": current_size }));
        }
    }
    Json(serde_json::json!({ "ok": false }))
}

pub async fn download(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsDownloadQuery>) -> Response {
    let (name, dir) = if let Some(id) = &q.id {
        // Need to find the file first to know its path
        // For AppData, we don't check user_id if it's in AppData dir? 
        // But we don't know the dir yet.
        // So we query by ID first.
        if let Ok(row) = sqlx::query("select name, dir, user_id from cloud_files where id = $1")
            .bind(id)
            .fetch_one(&state.db)
            .await 
        {
            let d: String = row.try_get("dir").unwrap_or_default();
            let u: String = row.try_get("user_id").unwrap_or_default();
            
            // Check access
            if d.starts_with("/AppData") {
                 // Check AppData permission
                 // We need to resolve_path to check permission
                 if let Err(_) = resolve_path(&state, &user.username, &d).await {
                     return Response::builder().status(403).body(Body::empty()).unwrap();
                 }
            } else {
                 // Standard user file check
                 if u != user.user_id.to_string() {
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

pub async fn rename(State(_state): State<AppState>, Extension(_user): Extension<AuthUser>, Json(_req): Json<DocsRenameReq>) -> impl IntoResponse {
    Json(serde_json::json!({ "ok": false, "error": "not_implemented_yet" }))
}

pub async fn delete(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsDeleteQuery>) -> impl IntoResponse {
    let (id, name, dir, storage) = if let Some(id) = &q.id {
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
         } else { return Json(serde_json::json!({ "ok": false })); }
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

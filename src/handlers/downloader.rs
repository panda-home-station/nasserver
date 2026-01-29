use axum::{
    extract::{Path, State, Extension},
    response::{IntoResponse, Json},
    http::StatusCode,
};
use crate::state::AppState;
use crate::models::downloader::{DownloadTask, CreateDownloadReq, ControlDownloadReq};
use crate::models::auth::AuthUser;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use std::time::Instant;
use crate::handlers::docs::{resolve_path, normalize_path};
use serde::Serialize;

#[derive(Serialize)]
struct DownloadTaskResp {
    #[serde(flatten)]
    task: DownloadTask,
    virtual_path: String,
}

fn get_virtual_path(path: &str, storage_path: &str) -> String {
    let rel = if path.starts_with(storage_path) {
        &path[storage_path.len()..]
    } else {
        return path.to_string(); // Should not happen
    };
    
    // rel is like /vol1/User/zac/Downloads/foo.zip or /vol1/AppData/app/Downloads/foo.zip
    // We want /Downloads/foo.zip or /Downloads
    // Actually we want the DIRECTORY.
    // Logic:
    // /vol1/User/<user>/... -> /...
    // /vol1/AppData/<app>/... -> /AppData/<app>/... -> Wait, docs::resolve_path logic:
    // /AppData/app/... maps to vol1/AppData/app/...
    // So vol1/AppData/app/... maps to /AppData/app/...
    
    let parts: Vec<&str> = rel.split('/').filter(|x| !x.is_empty()).collect();
    if parts.len() >= 3 && parts[0] == "vol1" && parts[1] == "User" {
        // parts[2] is username. parts[3..] is path
        if parts.len() > 3 {
             return format!("/{}", parts[3..].join("/"));
        }
        return "/".to_string();
    } else if parts.len() >= 3 && parts[0] == "vol1" && parts[1] == "AppData" {
        // parts[2] is app_name. parts[3..] is path
        // Virtual path starts with /AppData/app_name/...
        return format!("/AppData/{}", parts[2..].join("/"));
    }
    
    "/".to_string()
}

fn extract_info_from_path(path: &str, storage_path: &str) -> Option<(String, String)> {
    if !path.starts_with(storage_path) {
        return None;
    }
    let rel = &path[storage_path.len()..];
    let parts: Vec<&str> = rel.split('/').filter(|x| !x.is_empty()).collect();
    
    // Expect: vol1/User/<username>/...
    if parts.len() >= 3 && parts[0] == "vol1" && parts[1] == "User" {
        let username = parts[2].to_string();
        
        // Virtual dir is the directory containing the file
        // e.g. path = .../vol1/User/zac/Downloads/foo.zip
        // rel = /vol1/User/zac/Downloads/foo.zip
        // parts = ["vol1", "User", "zac", "Downloads", "foo.zip"]
        // we want "/Downloads"
        
        if parts.len() > 3 {
             let dir_parts = &parts[3..parts.len()-1];
             let virtual_dir = if dir_parts.is_empty() {
                 "/".to_string()
             } else {
                 format!("/{}", dir_parts.join("/"))
             };
             return Some((username, virtual_dir));
        } else {
             // File in root?
             return Some((username, "/".to_string()));
        }
    }
    None
 }
 
 async fn ensure_directory_exists(state: &AppState, user_id: &str, virtual_dir: &str) {
    if virtual_dir == "/" { return; }
    
    // Split path into components
    let parts: Vec<&str> = virtual_dir.split('/').filter(|x| !x.is_empty()).collect();
    let mut current_path = "/".to_string();
    
    for part in parts {
        let name = part;
        let parent_dir = current_path.clone();
        
        // Check if exists
        let exists: bool = sqlx::query_scalar("select count(*) > 0 from cloud_files where user_id = $1 and dir = $2 and name = $3 and storage = 'dir'")
            .bind(user_id)
            .bind(&parent_dir)
            .bind(name)
            .fetch_one(&state.db)
            .await
            .unwrap_or(false);
            
        if !exists {
            let id = uuid::Uuid::new_v4().to_string();
            let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage, created_at, updated_at) values ($1, $2, $3, $4, 0, 'dir', datetime('now'), datetime('now'))")
                .bind(id)
                .bind(user_id)
                .bind(name)
                .bind(&parent_dir)
                .execute(&state.db)
                .await;
             println!("[Downloader] Created missing directory: {}/{}", parent_dir, name);
        }
        
        if current_path == "/" {
            current_path = format!("/{}", name);
        } else {
            current_path = format!("{}/{}", current_path, name);
        }
    }
}

 pub async fn list_downloads(State(state): State<AppState>) -> impl IntoResponse {
    let tasks = sqlx::query_as::<_, DownloadTask>("select * from downloads order by created_at desc")
        .fetch_all(&state.db)
        .await;

    match tasks {
        Ok(t) => {
            let storage_path = state.storage_path.clone();
            let resps: Vec<DownloadTaskResp> = t.into_iter().map(|task| {
                let p = task.path.clone();
                let dir_path = std::path::Path::new(&p).parent().unwrap_or(std::path::Path::new("/")).to_str().unwrap_or("/");
                let virtual_path = get_virtual_path(dir_path, &storage_path);
                DownloadTaskResp {
                    task,
                    virtual_path
                }
            }).collect();
            Json(resps).into_response()
        },
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn create_download(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(payload): Json<CreateDownloadReq>) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    // Clean URL: remove leading/trailing whitespace and common punctuation from copy-pasting
    let url = payload.url.trim().trim_end_matches([',', ';', ' ']).to_string();
    
    // Magnet link handling
    if url.starts_with("magnet:?") {
        let filename = url.split('&')
            .find(|p| p.starts_with("dn="))
            .map(|p| urlencoding::decode(&p[3..]).unwrap_or_default().to_string())
            .unwrap_or_else(|| "magnet_download".to_string());
            
        let task = DownloadTask {
            id: id.clone(),
            url: url.clone(),
            path: "".to_string(),
            filename: filename.clone(),
            status: "error".to_string(),
            progress: 0.0,
            total_bytes: 0,
            downloaded_bytes: 0,
            speed: 0,
            created_at: chrono::Utc::now().naive_utc(),
            updated_at: chrono::Utc::now().naive_utc(),
            error_msg: Some("Magnet support requires 'librqbit' crate. Please add it to Cargo.toml.".to_string()),
        };

        let _ = sqlx::query(
            "insert into downloads (id, url, path, filename, status, progress, total_bytes, downloaded_bytes, speed, created_at, updated_at, error_msg) 
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"
        )
        .bind(&task.id)
        .bind(&task.url)
        .bind(&task.path)
        .bind(&task.filename)
        .bind(&task.status)
        .bind(&task.progress)
        .bind(&task.total_bytes)
        .bind(&task.downloaded_bytes)
        .bind(&task.speed)
        .bind(&task.created_at)
        .bind(&task.updated_at)
        .bind(&task.error_msg)
        .execute(&state.db)
        .await;

        return Json(task).into_response();
    }

    // Simple filename extraction
    let filename = url.split('/').last().unwrap_or("download").to_string();
    // Clean filename
    let filename = filename.split('?').next().unwrap_or(&filename).to_string();
    
    let storage_path = state.storage_path.clone();
    let save_dir = format!("{}/vol1/User/{}/下载", storage_path, user.username);
    let _ = tokio::fs::create_dir_all(&save_dir).await;
    
    // Proactively ensure the download directory exists in the database
    // This allows the user to see the folder immediately in the file manager
    ensure_directory_exists(&state, &user.user_id.to_string(), "/下载").await;

    let path = format!("{}/{}", save_dir, filename);

    let task = DownloadTask {
        id: id.clone(),
        url: url.clone(),
        path: path.clone(),
        filename: filename.clone(),
        status: "pending".to_string(),
        progress: 0.0,
        total_bytes: 0,
        downloaded_bytes: 0,
        speed: 0,
        created_at: chrono::Utc::now().naive_utc(),
        updated_at: chrono::Utc::now().naive_utc(),
        error_msg: None,
    };

    let res = sqlx::query(
        "insert into downloads (id, url, path, filename, status, progress, total_bytes, downloaded_bytes, speed, created_at, updated_at) 
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"
    )
    .bind(&task.id)
    .bind(&task.url)
    .bind(&task.path)
    .bind(&task.filename)
    .bind(&task.status)
    .bind(&task.progress)
    .bind(&task.total_bytes)
    .bind(&task.downloaded_bytes)
    .bind(&task.speed)
    .bind(&task.created_at)
    .bind(&task.updated_at)
    .execute(&state.db)
    .await;

    if let Err(e) = res {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // Spawn download task
    let state_clone = state.clone();
    let id_clone = id.clone();
    let url_clone = url.clone();
    let path_clone = path.clone();

    println!("[Downloader] Spawning task: id='{}', url='{}', path='{}'", id_clone, url_clone, path_clone);

    let handle = tokio::spawn(async move {
        download_process(state_clone, id_clone, url_clone, path_clone).await;
    });
    
    // Store abort handle
    if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.insert(id.clone(), handle.abort_handle());
    }

    Json(task).into_response()
}

async fn download_process(state: AppState, id: String, url: String, path: String) {
    println!("[Downloader] Process started for id={}", id);
    
    // Check for existing file for resume
    let mut downloaded_offset: u64 = 0;
    if let Ok(metadata) = tokio::fs::metadata(&path).await {
        downloaded_offset = metadata.len();
        println!("[Downloader] Found existing file, size: {}", downloaded_offset);
    }

    let client = reqwest::Client::builder()
        // Use Wget user agent since user confirmed wget works
        .user_agent("Wget/1.21.2") 
        .build()
        .unwrap_or_else(|e| {
            println!("[Downloader] Client build failed: {}", e);
            reqwest::Client::new()
        });
    
    // Update status to downloading
    let _ = sqlx::query("update downloads set status = 'downloading' where id = $1")
        .bind(&id)
        .execute(&state.db)
        .await;

    println!("[Downloader] Sending GET request to '{}', range={}-", url, downloaded_offset);
    
    let mut req = client.get(&url);
    if downloaded_offset > 0 {
        req = req.header("Range", format!("bytes={}-", downloaded_offset));
    }
    
    // Remove Referer header to mimic wget behavior
    let response = match req.send().await {
        Ok(r) => {
             println!("[Downloader] Response status: {}", r.status());
             println!("[Downloader] Response headers: {:?}", r.headers());
             if !r.status().is_success() {
                let msg = format!("HTTP error: {}", r.status());
                println!("[Downloader] Failed with status: {}", r.status());
                let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                    .bind(msg)
                    .bind(&id)
                    .execute(&state.db)
                    .await;
                // Remove task handle
                if let Ok(mut tasks) = state.download_tasks.lock() {
                    tasks.remove(&id);
                }
                return;
             }
             r
        },
        Err(e) => {
            println!("[Downloader] Request error: {}", e);
            let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                .bind(e.to_string())
                .bind(&id)
                .execute(&state.db)
                .await;
            // Remove task handle
            if let Ok(mut tasks) = state.download_tasks.lock() {
                tasks.remove(&id);
            }
            return;
        }
    };

    let total_size = response.content_length().unwrap_or(0) + downloaded_offset;
    let is_partial = response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    
    // If server returned 200 OK but we asked for range, it means it doesn't support range
    // So we should reset downloaded_offset to 0 and overwrite file
    let mut current_offset = if is_partial { downloaded_offset } else { 0 };
    
    println!("[Downloader] Total size: {}, Resuming: {}", total_size, is_partial);

    // Update total size
    let _ = sqlx::query("update downloads set total_bytes = $1 where id = $2")
        .bind(total_size as i64)
        .bind(&id)
        .execute(&state.db)
        .await;

    let file_result = if current_offset > 0 {
        tokio::fs::OpenOptions::new().create(true).append(true).open(&path).await
    } else {
        tokio::fs::File::create(&path).await
    };

    let mut file = match file_result {
        Ok(f) => f,
        Err(e) => {
             let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                .bind(e.to_string())
                .bind(&id)
                .execute(&state.db)
                .await;
            if let Ok(mut tasks) = state.download_tasks.lock() {
                tasks.remove(&id);
            }
            return;
        }
    };

    let mut last_update = Instant::now();
    let mut last_downloaded = current_offset;
    
    // response needs to be mutable for chunk()
    let mut response = response;

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = file.write_all(&chunk).await {
                     let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                        .bind(e.to_string())
                        .bind(&id)
                        .execute(&state.db)
                        .await;
                    if let Ok(mut tasks) = state.download_tasks.lock() {
                        tasks.remove(&id);
                    }
                    return;
                }

                current_offset += chunk.len() as u64;

                if last_update.elapsed().as_secs() >= 1 {
                    let speed = (current_offset - last_downloaded) as f64 / last_update.elapsed().as_secs_f64();
                    let progress = if total_size > 0 {
                        (current_offset as f64 / total_size as f64) * 100.0
                    } else {
                        0.0
                    };

                    let _ = sqlx::query("update downloads set downloaded_bytes = $1, speed = $2, progress = $3, updated_at = datetime('now') where id = $4")
                        .bind(current_offset as i64)
                        .bind(speed as i64)
                        .bind(progress)
                        .bind(&id)
                        .execute(&state.db)
                        .await;
                    
                    last_update = Instant::now();
                    last_downloaded = current_offset;
                }
            },
            Ok(None) => break,
            Err(e) => {
                let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                    .bind(e.to_string())
                    .bind(&id)
                    .execute(&state.db)
                    .await;
                if let Ok(mut tasks) = state.download_tasks.lock() {
                    tasks.remove(&id);
                }
                return;
            }
        }
    }

    // Done
    let _ = sqlx::query("update downloads set status = 'done', progress = 100.0, downloaded_bytes = $1, speed = 0, updated_at = datetime('now') where id = $2")
        .bind(current_offset as i64)
        .bind(&id)
        .execute(&state.db)
        .await;

    // Register to cloud_files
    let storage_path = state.storage_path.clone();
    if let Some((username, virtual_dir)) = extract_info_from_path(&path, &storage_path) {
        // Get user_id
        if let Ok(user_uuid) = sqlx::query_scalar::<_, uuid::Uuid>("select id from users where username = $1")
            .bind(&username)
            .fetch_one(&state.db)
            .await 
        {
             let user_id = user_uuid.to_string();
             
             // Ensure parent directories exist
             ensure_directory_exists(&state, &user_id, &virtual_dir).await;

             let file_id = uuid::Uuid::new_v4().to_string();
             let filename = std::path::Path::new(&path).file_name().unwrap_or_default().to_str().unwrap_or("unknown");
             
             // Check if file already exists in cloud_files to avoid duplicates (optional but good)
             // Actually cloud_files id is primary key, but we want to avoid multiple entries for same file
             // We can just insert, if we want to support duplicates (same name different id), or check.
             // Given the schema doesn't have unique constraint on (user_id, dir, name), we just insert.
             // But let's try to delete existing entry for same path if any, to keep it clean?
             // Or just insert. User might have deleted it from UI but file remains? No, UI deletes file.
             // Let's just insert.
             
             let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, 'file', datetime('now'), datetime('now'))")
                .bind(file_id)
                .bind(user_id)
                .bind(filename)
                .bind(virtual_dir)
                .bind(current_offset as i64)
                .execute(&state.db)
                .await;
             println!("[Downloader] Registered file '{}' to cloud_files", filename);
        }
    }
        
    if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.remove(&id);
    }
}

pub async fn control_download(State(state): State<AppState>, Path(id): Path<String>, Json(payload): Json<ControlDownloadReq>) -> impl IntoResponse {
    match payload.action.as_str() {
        "delete" => {
            // Get task info first to decide whether to delete file
            if let Ok(task) = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1").bind(&id).fetch_one(&state.db).await {
                // Abort if running
                if let Ok(mut tasks) = state.download_tasks.lock() {
                    if let Some(handle) = tasks.remove(&id) {
                        handle.abort();
                    }
                }
                
                // Delete file only if it is NOT done (clean up partial files)
                // If it is done, user wants to keep the file
                if task.status != "done" {
                    let _ = tokio::fs::remove_file(&task.path).await;
                }
            }
            
            // Delete from DB
            let _ = sqlx::query("delete from downloads where id = $1").bind(&id).execute(&state.db).await;
        },
        "pause" => {
            // Abort task
            if let Ok(mut tasks) = state.download_tasks.lock() {
                if let Some(handle) = tasks.remove(&id) {
                    handle.abort();
                    println!("[Downloader] Task {} paused (aborted)", id);
                }
            }
            // Update status
            let _ = sqlx::query("update downloads set status = 'paused', speed = 0 where id = $1")
                .bind(&id)
                .execute(&state.db)
                .await;
        },
        "resume" => {
            // Get task info
            if let Ok(task) = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1").bind(&id).fetch_one(&state.db).await {
                // Spawn new process
                let state_clone = state.clone();
                let id_clone = id.clone();
                let url_clone = task.url.clone();
                let path_clone = task.path.clone();

                let handle = tokio::spawn(async move {
                    download_process(state_clone, id_clone, url_clone, path_clone).await;
                });
                
                if let Ok(mut tasks) = state.download_tasks.lock() {
                    tasks.insert(id.clone(), handle.abort_handle());
                }
                println!("[Downloader] Task {} resumed", id);
            }
        },
        _ => {}
    }
    StatusCode::OK.into_response()
}

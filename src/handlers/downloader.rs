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

pub async fn list_downloads(State(state): State<AppState>) -> impl IntoResponse {
    let tasks = sqlx::query_as::<_, DownloadTask>("select * from downloads order by created_at desc")
        .fetch_all(&state.db)
        .await;

    match tasks {
        Ok(t) => Json(t).into_response(),
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
    let save_dir = format!("{}/vol1/User/{}/Downloads", storage_path, user.username);
    let _ = tokio::fs::create_dir_all(&save_dir).await;
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
        
    if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.remove(&id);
    }
}

pub async fn control_download(State(state): State<AppState>, Path(id): Path<String>, Json(payload): Json<ControlDownloadReq>) -> impl IntoResponse {
    match payload.action.as_str() {
        "delete" => {
            // Abort if running
            if let Ok(mut tasks) = state.download_tasks.lock() {
                if let Some(handle) = tasks.remove(&id) {
                    handle.abort();
                }
            }
            
            // Delete file
            if let Ok(task) = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1").bind(&id).fetch_one(&state.db).await {
                let _ = tokio::fs::remove_file(&task.path).await;
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

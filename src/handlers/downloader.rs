use axum::{
    extract::{Path, State, Extension},
    response::{IntoResponse, Json},
    http::StatusCode,
};
use crate::state::AppState;
use crate::models::downloader::{DownloadTask, CreateDownloadReq, ControlDownloadReq, ResolveMagnetReq, ResolveMagnetResp, StartMagnetDownloadReq, TorrentFileMetadata};
use crate::models::auth::AuthUser;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use std::time::Instant;
use crate::handlers::docs::{resolve_path, normalize_path};
use serde::Serialize;
use librqbit::{AddTorrent, AddTorrentOptions, AddTorrentResponse, ListOnlyResponse, ManagedTorrent, Api, ByteBufOwned};
use librqbit::dht::Id20;
use std::str::FromStr;

const DEFAULT_TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://9.rarbg.com:2810/announce",
    "udp://p4p.arenabg.com:1337/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://tracker.tiny-vps.com:6969/announce",
    "udp://tracker.moeking.me:6969/announce",
    "https://opentracker.i2p.rocks/announce",
];

fn enrich_magnet_link(url: &str) -> String {
    if !url.starts_with("magnet:?") {
        return url.to_string();
    }
    let mut new_url = url.to_string();
    for tracker in DEFAULT_TRACKERS {
        let encoded = urlencoding::encode(tracker);
        if !new_url.contains(&*encoded) {
            new_url.push_str(&format!("&tr={}", encoded));
        }
    }
    new_url
}

#[derive(Serialize)]
pub struct SubTaskResp {
    pub filename: String,
    pub progress: f64,
    pub total_bytes: u64,
    pub downloaded_bytes: u64,
    pub speed: u64,
    pub status: String,
}

#[derive(Serialize)]
struct DownloadTaskResp {
    #[serde(flatten)]
    task: DownloadTask,
    virtual_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sub_tasks: Option<Vec<SubTaskResp>>,
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
        .await
        .unwrap_or(vec![]);

    let mut resp_tasks = Vec::new();
    let storage_path = state.storage_path.clone();
    let api = Api::new(state.torrent_session.clone(), None);

    for mut task in tasks {
        let virtual_path = get_virtual_path(&task.path, &storage_path);
        let mut sub_tasks = None;

        if task.url.starts_with("magnet:?xt=urn:btih:") {
            if let Some(hash_str) = task.url.strip_prefix("magnet:?xt=urn:btih:") {
                let hash_clean = hash_str.split('&').next().unwrap_or(hash_str);
                
                if let Ok(id) = Id20::from_str(hash_clean) {
                    if let Ok(details) = api.api_torrent_details(librqbit::api::TorrentIdOrHash::Hash(id)) {
                         if let Some(stats) = &details.stats {
                             if stats.total_bytes > 0 {
                                 task.progress = (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0;
                             }
                             task.downloaded_bytes = stats.progress_bytes as i64;
                             task.total_bytes = stats.total_bytes as i64;
                             if let Some(live) = &stats.live {
                                 // mbps is MiB/s, convert to bytes/s
                                 task.speed = (live.download_speed.mbps * 1024.0 * 1024.0) as i64;
                             } else {
                                 task.speed = 0;
                             }
                             task.status = stats.state.to_string();
                         }

                         if let Some(files) = details.files {
                             let mut subs = Vec::new();
                             let file_progress = details.stats.as_ref().map(|s| &s.file_progress);
                             for (idx, file) in files.iter().enumerate() {
                                 let size = file.length;
                                 let downloaded = file_progress.and_then(|fp| fp.get(idx).copied()).unwrap_or(0);
                                 let speed = 0; // Per-file speed not available easily
                                 
                                 let progress = if size > 0 {
                                     (downloaded as f64 / size as f64) * 100.0
                                 } else {
                                     0.0
                                 };
                                 
                                 subs.push(SubTaskResp {
                                     filename: file.name.clone(),
                                     progress,
                                     total_bytes: size,
                                     downloaded_bytes: downloaded,
                                     speed,
                                     status: task.status.clone(),
                                 });
                             }
                             sub_tasks = Some(subs);
                         }
                    }
                }
            }
        }

        resp_tasks.push(DownloadTaskResp {
            task,
            virtual_path,
            sub_tasks
        });
    }

    Json(resp_tasks)
}

pub async fn create_download(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(payload): Json<CreateDownloadReq>) -> impl IntoResponse {
    let id = uuid::Uuid::new_v4().to_string();
    // Clean URL: remove leading/trailing whitespace and common punctuation from copy-pasting
    let mut url = payload.url.trim().trim_end_matches([',', ';', ' ']).to_string();
    
    // Enrich magnet link with trackers
    url = enrich_magnet_link(&url);
    
    // Magnet link handling
    if url.starts_with("magnet:?") {
        let filename = url.split('&')
            .find(|p| p.starts_with("dn="))
            .map(|p| urlencoding::decode(&p[3..]).unwrap_or_default().to_string())
            .unwrap_or_else(|| "magnet_download".to_string());
            
        let storage_path = state.storage_path.clone();
        let save_dir = format!("{}/vol1/User/{}/下载", storage_path, user.username);
        let _ = tokio::fs::create_dir_all(&save_dir).await;
        
        // Proactively ensure the download directory exists in the database
        ensure_directory_exists(&state, &user.user_id.to_string(), "/下载").await;

        let task = DownloadTask {
            id: id.clone(),
            url: url.clone(),
            path: save_dir.clone(),
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

        let state_clone = state.clone();
        let id_clone = id.clone();
        let url_clone = url.clone();
        let save_dir_clone = save_dir.clone();

        let handle = tokio::spawn(async move {
            download_magnet_process(state_clone, id_clone, url_clone, save_dir_clone).await;
        });

        if let Ok(mut tasks) = state.download_tasks.lock() {
            tasks.insert(id.clone(), handle.abort_handle());
        }

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

pub async fn resolve_magnet(State(state): State<AppState>, Json(payload): Json<ResolveMagnetReq>) -> impl IntoResponse {
    let magnet_url = enrich_magnet_link(&payload.magnet_url);
    let opts = AddTorrentOptions {
        list_only: true,
        ..Default::default()
    };
    
    let add_result = state.torrent_session.add_torrent(AddTorrent::from_url(magnet_url), Some(opts)).await;
    
    match add_result {
        Ok(AddTorrentResponse::ListOnly(resp)) => {
            let info_hash = resp.info_hash.as_string();
            let mut files = Vec::new();
            
            if let Ok(file_details) = resp.info.iter_file_details() {
                for (idx, fd) in file_details.enumerate() {
                    files.push(TorrentFileMetadata {
                        index: idx,
                        name: fd.filename.to_string().unwrap_or_default(),
                        size: fd.len,
                    });
                }
            }

            let name = resp.info.name.clone().map(|n| n.to_string());
            
            // Cache the torrent bytes
            if let Ok(mut cache) = state.magnet_cache.lock() {
                cache.insert(info_hash.clone(), resp.torrent_bytes);
            }
            
            Json(ResolveMagnetResp {
                token: info_hash,
                files,
                name,
            }).into_response()
        },
        Ok(_) => {
            (StatusCode::INTERNAL_SERVER_ERROR, "Unexpected response from torrent engine (not ListOnly)".to_string()).into_response()
        },
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to resolve magnet: {}", e)).into_response()
        }
    }
}

use librqbit_core::torrent_metainfo::torrent_from_bytes;

pub async fn start_magnet_download(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(payload): Json<StartMagnetDownloadReq>) -> impl IntoResponse {
    let torrent_bytes = {
        let cache = state.magnet_cache.lock().unwrap();
        match cache.get(&payload.token) {
            Some(b) => b.clone(),
            None => return (StatusCode::BAD_REQUEST, "Token expired or invalid".to_string()).into_response(),
        }
    };
    
    // Parse torrent metadata to determine name and structure
    let meta_result = torrent_from_bytes::<ByteBufOwned>(&torrent_bytes);
    let (torrent_name, is_single_file) = match meta_result {
        Ok(meta) => {
            // meta.info is TorrentMetaV1Info (or compatible)
            let name = meta.info.name.clone().map(|n| n.to_string()).unwrap_or_else(|| "magnet_download".to_string());
            let is_single = meta.info.length.is_some();
            (name, is_single)
        },
        Err(_) => ("magnet_download".to_string(), false)
    };

    let id = uuid::Uuid::new_v4().to_string();
    let storage_path = state.storage_path.clone();
    
    // Determine save directory
    let base_save_dir = if let Some(p) = payload.path {
        // Handle virtual path: remove leading slash if present
        let relative_path = p.trim_start_matches('/');
        // Construct full physical path
        format!("{}/vol1/User/{}/{}", storage_path, user.username, relative_path)
    } else {
        let default_dir = format!("{}/vol1/User/{}/下载", storage_path, user.username);
        ensure_directory_exists(&state, &user.user_id.to_string(), "/下载").await;
        default_dir
    };

    // If multi-file, append torrent name to base directory to create a dedicated folder
    // If single-file, save directly to base directory (filename will be appended by torrent engine)
    let save_dir = if !is_single_file {
        format!("{}/{}", base_save_dir, torrent_name)
    } else {
        base_save_dir
    };

    let _ = tokio::fs::create_dir_all(&save_dir).await;
    
    let opts = AddTorrentOptions {
        output_folder: Some(save_dir.clone()),
        only_files: Some(payload.files),
        overwrite: true,
        ..Default::default()
    };
    
    // Add torrent using URL to ensure trackers are used
    let magnet_url = enrich_magnet_link(&format!("magnet:?xt=urn:btih:{}", payload.token));
    println!("Adding torrent to path: {} with url: {}", save_dir, magnet_url);
    
    match state.torrent_session.add_torrent(AddTorrent::from_url(magnet_url.clone()), Some(opts)).await {
        Ok(resp) => {
            if let Some(handle) = resp.into_handle() {
                // Get torrent name if possible (using placeholder if API access fails)
                let torrent_name: String = handle.name().unwrap_or(torrent_name.clone());

                 let task = DownloadTask {
                    id: id.clone(),
                    url: magnet_url, 
                    path: save_dir.clone(),
                    filename: torrent_name, 
                    status: "downloading".to_string(),
                    progress: 0.0,
                    total_bytes: 0,
                    downloaded_bytes: 0,
                    speed: 0,
                    created_at: chrono::Utc::now().naive_utc(),
                    updated_at: chrono::Utc::now().naive_utc(),
                    error_msg: None,
                };
                
                // Insert into DB
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
                
                let state_clone = state.clone();
                let id_clone = id.clone();
                
                let abort_handle = tokio::spawn(async move {
                    monitor_torrent_process(state_clone, id_clone, handle).await;
                });
                
                if let Ok(mut tasks) = state.download_tasks.lock() {
                    tasks.insert(id.clone(), abort_handle.abort_handle());
                }
                
                return Json(task).into_response();
            } else {
                 println!("Failed to get torrent handle");
                 (StatusCode::INTERNAL_SERVER_ERROR, "Failed to get torrent handle".to_string()).into_response()
            }
        },
        Err(e) => {
            println!("Failed to start download: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to start download: {}", e)).into_response()
        }
    }
}

async fn monitor_torrent_process(state: AppState, id: String, handle: std::sync::Arc<ManagedTorrent>) {
    // Update status to downloading (already done in handler, but good to ensure)
    let _ = sqlx::query("update downloads set status = 'downloading' where id = $1")
        .bind(&id)
        .execute(&state.db)
        .await;

    loop {
        if handle.stats().finished {
            break;
        }
        
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        
        let stats = handle.stats();
        let total_bytes = stats.total_bytes;
        let downloaded_bytes = stats.progress_bytes; 
        let progress = if total_bytes > 0 { (downloaded_bytes as f64 / total_bytes as f64) * 100.0 } else { 0.0 };
        let speed = stats.live.as_ref().map(|l| (l.download_speed.mbps * 1024.0 * 1024.0) as u64).unwrap_or(0);
        
        let _ = sqlx::query("update downloads set downloaded_bytes = $1, speed = $2, progress = $3, total_bytes = $4, updated_at = datetime('now') where id = $5")
            .bind(downloaded_bytes as i64)
            .bind(speed as i64)
            .bind(progress)
            .bind(total_bytes as i64)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    
    // Done
    let _ = sqlx::query("update downloads set status = 'done', progress = 100.0, speed = 0, updated_at = datetime('now') where id = $1")
        .bind(&id)
        .execute(&state.db)
        .await;

    if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.remove(&id);
    }
}

async fn download_magnet_process(state: AppState, id: String, url: String, save_dir: String) {
    let url = enrich_magnet_link(&url);
    println!("[Downloader] Magnet process started for id={}", id);

    let opts = AddTorrentOptions {
        output_folder: Some(save_dir.clone()),
        overwrite: true,
        ..Default::default()
    };

    let handle = match state.torrent_session.add_torrent(AddTorrent::from_url(url.clone()), Some(opts)).await {
        Ok(h) => h.into_handle().unwrap(),
        Err(e) => {
            println!("[Downloader] Magnet add error: {}", e);
             let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                .bind(e.to_string())
                .bind(&id)
                .execute(&state.db)
                .await;
            return;
        }
    };
    
    monitor_torrent_process(state, id, handle).await;
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
    let mut response = match req.send().await {
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
            let msg = format!("Request failed: {}", e);
            println!("[Downloader] Request failed: {}", e);
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
    };

    let total_size = response.content_length().unwrap_or(0) + downloaded_offset;
    println!("[Downloader] Total size: {}", total_size);

    let mut file = if downloaded_offset > 0 {
        tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .unwrap()
    } else {
        tokio::fs::File::create(&path).await.unwrap()
    };

    let mut downloaded = downloaded_offset;
    let mut last_update = Instant::now();
    let mut last_downloaded = downloaded;

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if let Err(e) = file.write_all(&chunk).await {
                    let msg = format!("Write error: {}", e);
                    let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                        .bind(msg)
                        .bind(&id)
                        .execute(&state.db)
                        .await;
                    return;
                }
                downloaded += chunk.len() as u64;

                if last_update.elapsed().as_secs() >= 1 {
                    let speed = (downloaded - last_downloaded) as f64 / last_update.elapsed().as_secs_f64();
                    let progress = if total_size > 0 { (downloaded as f64 / total_size as f64) * 100.0 } else { 0.0 };
                    
                    let _ = sqlx::query("update downloads set downloaded_bytes = $1, speed = $2, progress = $3, total_bytes = $4, updated_at = datetime('now') where id = $5")
                        .bind(downloaded as i64)
                        .bind(speed as i64)
                        .bind(progress)
                        .bind(total_size as i64)
                        .bind(&id)
                        .execute(&state.db)
                        .await;

                    last_update = Instant::now();
                    last_downloaded = downloaded;
                }
            },
            Ok(None) => break, // End of stream
            Err(e) => {
                let msg = format!("Stream error: {}", e);
                let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                    .bind(msg)
                    .bind(&id)
                    .execute(&state.db)
                    .await;
                return;
            }
        }
    }

    // Done
    let _ = sqlx::query("update downloads set status = 'done', progress = 100.0, speed = 0, updated_at = datetime('now') where id = $1")
        .bind(&id)
        .execute(&state.db)
        .await;

    if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.remove(&id);
    }
}

pub async fn pause_download(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let handle_opt = if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.remove(&id)
    } else {
        None
    };

    if let Some(handle) = handle_opt {
        handle.abort();
        let _ = sqlx::query("update downloads set status = 'paused', speed = 0 where id = $1")
            .bind(&id)
            .execute(&state.db)
            .await;
        return (StatusCode::OK, "Paused").into_response();
    }
    (StatusCode::NOT_FOUND, "Task not found or already stopped").into_response()
}

pub async fn resume_download(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let task = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1")
        .bind(&id)
        .fetch_optional(&state.db)
        .await;

    if let Ok(Some(task)) = task {
        if task.status == "downloading" {
            return (StatusCode::OK, "Already downloading").into_response();
        }
        
        // Restart logic depending on type
        // For now, we only support resuming normal HTTP downloads via creating a new task logic but keeping ID?
        // Actually, resuming requires re-spawning the process.
        // Simplified: just update status and spawn.
        
        let state_clone = state.clone();
        let id_clone = id.clone();
        let url_clone = task.url.clone();
        let path_clone = task.path.clone(); // This is full file path for http, directory for magnet?
        
        let handle = if url_clone.starts_with("magnet:?") {
             // For magnet, path is save_dir
             tokio::spawn(async move {
                download_magnet_process(state_clone, id_clone, url_clone, path_clone).await;
            })
        } else {
             tokio::spawn(async move {
                download_process(state_clone, id_clone, url_clone, path_clone).await;
            })
        };

        if let Ok(mut tasks) = state.download_tasks.lock() {
            tasks.insert(id.clone(), handle.abort_handle());
        }

        return (StatusCode::OK, "Resumed").into_response();
    }
    
    (StatusCode::NOT_FOUND, "Task not found").into_response()
}

pub async fn delete_download(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    // Stop if running
    let handle_opt = if let Ok(mut tasks) = state.download_tasks.lock() {
        tasks.remove(&id)
    } else {
        None
    };

    if let Some(handle) = handle_opt {
        handle.abort();
    }
    
    // Delete from DB
    let _ = sqlx::query("delete from downloads where id = $1")
        .bind(&id)
        .execute(&state.db)
        .await;
        
    // Optional: Delete file? user might want to keep it. 
    // Usually "delete task" implies keeping file, "delete task and file" is another option.
    // Here we just delete task.
    
    (StatusCode::OK, "Deleted").into_response()
}

#[axum::debug_handler]
pub async fn control_download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<ControlDownloadReq>
) -> impl IntoResponse {
    match payload.action.as_str() {
        "pause" => pause_download(State(state), Path(id)).await.into_response(),
        "resume" => resume_download(State(state), Path(id)).await.into_response(),
        "delete" => delete_download(State(state), Path(id)).await.into_response(),
        _ => (StatusCode::BAD_REQUEST, "Invalid action").into_response(),
    }
}

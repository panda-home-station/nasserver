use async_trait::async_trait;
use sqlx::{Pool, Sqlite};
use std::sync::Arc;
use tokio::sync::Mutex;
use std::collections::HashMap;
use std::path::Path;
use librqbit::{Session, AddTorrent, AddTorrentOptions, AddTorrentResponse, ManagedTorrent};
use common::core::{Result, AppError};
use crate::{DownloaderService, DownloadTaskResp, SubTaskResp};
use models::downloader::{DownloadTask, CreateDownloadReq, ResolveMagnetReq, ResolveMagnetResp, StartMagnetDownloadReq, TorrentFileMetadata};
use chrono::Utc;
use uuid::Uuid;
use tokio::task::AbortHandle;
use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use std::time::Instant;
use std::str::FromStr;

pub struct DownloaderServiceImpl {
    db: Pool<Sqlite>,
    storage_path: String,
    torrent_session: Arc<Session>,
    magnet_cache: Arc<Mutex<HashMap<String, Bytes>>>,
    download_tasks: Arc<Mutex<HashMap<String, AbortHandle>>>,
}

impl DownloaderServiceImpl {
    pub fn new(
        db: Pool<Sqlite>,
        storage_path: String,
        torrent_session: Arc<Session>,
    ) -> Self {
        Self {
            db,
            storage_path,
            torrent_session,
            magnet_cache: Arc::new(Mutex::new(HashMap::new())),
            download_tasks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn get_virtual_path(&self, path: &str) -> String {
        let rel = if path.starts_with(&self.storage_path) {
            &path[self.storage_path.len()..]
        } else {
            return path.to_string();
        };
        
        let parts: Vec<&str> = rel.split('/').filter(|x| !x.is_empty()).collect();
        if parts.len() >= 3 && parts[0] == "vol1" && parts[1] == "User" {
            if parts.len() > 3 {
                 return format!("/{}", parts[3..].join("/"));
            }
            return "/".to_string();
        } else if parts.len() >= 3 && parts[0] == "vol1" && parts[1] == "AppData" {
            return format!("/AppData/{}", parts[2..].join("/"));
        }
        
        "/".to_string()
    }

    async fn ensure_directory_exists(&self, user_id: &str, virtual_dir: &str) {
        if virtual_dir == "/" { return; }
        
        let parts: Vec<&str> = virtual_dir.split('/').filter(|x| !x.is_empty()).collect();
        let mut current_path = "/".to_string();
        
        for part in parts {
            let name = part;
            let parent_dir = current_path.clone();
            
            let exists: bool = sqlx::query_scalar("select count(*) > 0 from cloud_files where user_id = $1 and dir = $2 and name = $3 and storage = 'dir'")
                .bind(user_id)
                .bind(&parent_dir)
                .bind(name)
                .fetch_one(&self.db)
                .await
                .unwrap_or(false);
                
            if !exists {
                let id = Uuid::new_v4().to_string();
                let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage, created_at, updated_at) values ($1, $2, $3, $4, 0, 'dir', datetime('now'), datetime('now'))")
                    .bind(id)
                    .bind(user_id)
                    .bind(name)
                    .bind(&parent_dir)
                    .execute(&self.db)
                    .await;
            }
            
            if current_path == "/" {
                current_path = format!("/{}", name);
            } else {
                current_path = format!("{}/{}", current_path, name);
            }
        }
    }

    async fn get_user_id_by_username(&self, username: &str) -> Option<String> {
        sqlx::query_scalar("select id from users where username = $1")
            .bind(username)
            .fetch_one(&self.db)
            .await
            .ok()
    }
}

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

#[async_trait]
impl DownloaderService for DownloaderServiceImpl {
    async fn list_downloads(&self) -> Result<Vec<DownloadTaskResp>> {
        let tasks = sqlx::query_as::<_, DownloadTask>("select * from downloads order by created_at desc")
            .fetch_all(&self.db)
            .await?;

        let mut resp_tasks = Vec::new();
        let api = librqbit::Api::new(self.torrent_session.clone(), None);

        for task in tasks {
            let virtual_path = self.get_virtual_path(&task.path);
            let mut sub_tasks = None;
            let mut current_task = task.clone();

            if task.url.starts_with("magnet:?xt=urn:btih:") {
                if let Some(hash_str) = task.url.strip_prefix("magnet:?xt=urn:btih:") {
                    let hash_clean = hash_str.split('&').next().unwrap_or(hash_str);
                    
                    if let Ok(id) = librqbit::dht::Id20::from_str(hash_clean) {
                        if let Ok(details) = api.api_torrent_details(librqbit::api::TorrentIdOrHash::Hash(id)) {
                             if let Some(stats) = &details.stats {
                                 if stats.total_bytes > 0 {
                                     current_task.progress = (stats.progress_bytes as f64 / stats.total_bytes as f64) * 100.0;
                                 }
                                 current_task.downloaded_bytes = stats.progress_bytes as i64;
                                 current_task.total_bytes = stats.total_bytes as i64;
                                 if let Some(live) = &stats.live {
                                     current_task.speed = (live.download_speed.mbps * 1024.0 * 1024.0) as i64;
                                 } else {
                                     current_task.speed = 0;
                                 }
                                 current_task.status = stats.state.to_string();
                             }

                             if let Some(files) = details.files {
                                 let mut subs = Vec::new();
                                 let file_progress = details.stats.as_ref().map(|s| &s.file_progress);
                                 for (idx, file) in files.iter().enumerate() {
                                     let size = file.length;
                                     let downloaded = file_progress.and_then(|fp| fp.get(idx).copied()).unwrap_or(0);
                                     
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
                                         speed: 0,
                                         status: current_task.status.clone(),
                                     });
                                 }
                                 sub_tasks = Some(subs);
                             }
                        }
                    }
                }
            }
            
            resp_tasks.push(DownloadTaskResp {
                task: current_task,
                virtual_path,
                sub_tasks,
            });
        }
        
        Ok(resp_tasks)
    }

    async fn create_download(&self, username: &str, req: CreateDownloadReq) -> Result<()> {
        let id = Uuid::new_v4().to_string();
        let mut url = req.url.trim().trim_end_matches([',', ';', ' ']).to_string();
        url = enrich_magnet_link(&url);
        
        let user_id = self.get_user_id_by_username(username).await
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;

        let save_dir = format!("{}/vol1/User/{}/下载", self.storage_path, username);
        let _ = tokio::fs::create_dir_all(&save_dir).await;
        self.ensure_directory_exists(&user_id, "/下载").await;

        if url.starts_with("magnet:?") {
            let filename = url.split('&')
                .find(|p| p.starts_with("dn="))
                .map(|p| urlencoding::decode(&p[3..]).unwrap_or_default().to_string())
                .unwrap_or_else(|| "magnet_download".to_string());
                
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
                created_at: Utc::now().naive_utc(),
                updated_at: Utc::now().naive_utc(),
                error_msg: None,
            };

            sqlx::query(
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
            .execute(&self.db)
            .await?;

            // Spawn download task (logic moved to helper)
            self.spawn_magnet_download(id, url, save_dir, user_id, username.to_string()).await;
            return Ok(());
        }

        let filename = url.split('/').last().unwrap_or("download").to_string();
        let filename = filename.split('?').next().unwrap_or(&filename).to_string();
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
            created_at: Utc::now().naive_utc(),
            updated_at: Utc::now().naive_utc(),
            error_msg: None,
        };

        sqlx::query(
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
        .execute(&self.db)
        .await?;

        self.spawn_http_download(id, url, path, user_id).await;
        Ok(())
    }

    async fn pause_download(&self, id: &str) -> Result<()> {
        let task = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1")
            .bind(id)
            .fetch_optional(&self.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

        if task.url.starts_with("magnet:?xt=urn:btih:") {
            if let Some(hash_str) = task.url.strip_prefix("magnet:?xt=urn:btih:") {
                let hash_clean = hash_str.split('&').next().unwrap_or(hash_str);
                if let Ok(info_hash) = librqbit::dht::Id20::from_str(hash_clean) {
                    let api = librqbit::Api::new(self.torrent_session.clone(), None);
                    let _ = api.api_torrent_action_pause(librqbit::api::TorrentIdOrHash::Hash(info_hash));
                }
            }
        }

        sqlx::query("update downloads set status = 'paused' where id = $1")
            .bind(id)
            .execute(&self.db)
            .await?;

        let mut tasks = self.download_tasks.lock().await;
        if let Some(handle) = tasks.remove(id) {
            handle.abort();
        }

        Ok(())
    }

    async fn resume_download(&self, id: &str) -> Result<()> {
        let task = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1")
            .bind(id)
            .fetch_optional(&self.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

        if task.url.starts_with("magnet:?xt=urn:btih:") {
            if let Some(hash_str) = task.url.strip_prefix("magnet:?xt=urn:btih:") {
                let hash_clean = hash_str.split('&').next().unwrap_or(hash_str);
                if let Ok(info_hash) = librqbit::dht::Id20::from_str(hash_clean) {
                    let api = librqbit::Api::new(self.torrent_session.clone(), None);
                    let _ = api.api_torrent_action_start(librqbit::api::TorrentIdOrHash::Hash(info_hash));
                    
                    // Re-spawn monitor if it's a torrent
                    let api = librqbit::Api::new(self.torrent_session.clone(), None);
                    let _ = api.api_torrent_details(librqbit::api::TorrentIdOrHash::Hash(info_hash));
                }
            }
        } else {
            // HTTP download resume
            self.spawn_http_download(task.id, task.url, task.path, "1".to_string()).await; // FIXME: user_id
        }

        sqlx::query("update downloads set status = 'downloading' where id = $1")
            .bind(id)
            .execute(&self.db)
            .await?;

        Ok(())
    }

    async fn delete_download(&self, id: &str) -> Result<()> {
        let task = sqlx::query_as::<_, DownloadTask>("select * from downloads where id = $1")
            .bind(id)
            .fetch_optional(&self.db)
            .await?
            .ok_or_else(|| AppError::NotFound("Task not found".to_string()))?;

        if task.url.starts_with("magnet:?xt=urn:btih:") {
            if let Some(hash_str) = task.url.strip_prefix("magnet:?xt=urn:btih:") {
                let hash_clean = hash_str.split('&').next().unwrap_or(hash_str);
                if let Ok(info_hash) = librqbit::dht::Id20::from_str(hash_clean) {
                    let api = librqbit::Api::new(self.torrent_session.clone(), None);
                    let _ = api.api_torrent_action_delete(librqbit::api::TorrentIdOrHash::Hash(info_hash)).await;
                }
            }
        }

        sqlx::query("delete from downloads where id = $1")
            .bind(id)
            .execute(&self.db)
            .await?;
        
        {
            let mut tasks = self.download_tasks.lock().await;
            if let Some(handle) = tasks.remove(id) {
                handle.abort();
            }
        }
        Ok(())
    }

    async fn resolve_magnet(&self, req: ResolveMagnetReq) -> Result<ResolveMagnetResp> {
        let magnet_url = enrich_magnet_link(&req.magnet_url);
        let opts = AddTorrentOptions {
            list_only: true,
            ..Default::default()
        };
        
        let add_result = self.torrent_session.add_torrent(AddTorrent::from_url(magnet_url), Some(opts)).await
            .map_err(|e| AppError::Internal(format!("Failed to resolve magnet: {}", e)))?;
        
        match add_result {
            AddTorrentResponse::ListOnly(resp) => {
                let info_hash = resp.info_hash.0.iter().map(|b| format!("{:02x}", b)).collect::<String>();
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
                
                {
                    let mut cache: tokio::sync::MutexGuard<HashMap<String, Bytes>> = self.magnet_cache.lock().await;
                    cache.insert(info_hash.clone(), resp.torrent_bytes.into());
                }
                
                Ok(ResolveMagnetResp {
                    token: info_hash,
                    files,
                    name,
                })
            },
            _ => Err(AppError::Internal("Unexpected response from torrent engine (not ListOnly)".to_string())),
        }
    }

    async fn start_magnet_download(&self, username: &str, req: StartMagnetDownloadReq) -> Result<()> {
        let torrent_bytes = {
            let cache: tokio::sync::MutexGuard<HashMap<String, Bytes>> = self.magnet_cache.lock().await;
            match cache.get(&req.token) {
                Some(b) => b.clone(),
                None => return Err(AppError::NotFound("Magnet info expired or not found".to_string())),
            }
        };
        
        let meta_result = librqbit_core::torrent_metainfo::torrent_from_bytes::<librqbit::ByteBufOwned>(&torrent_bytes);
        let (torrent_name, is_single_file): (String, bool) = match meta_result {
            Ok(meta) => {
                let name = meta.info.name.clone().map(|n| n.to_string()).unwrap_or_else(|| "magnet_download".to_string());
                let is_single = meta.info.length.is_some();
                (name, is_single)
            },
            Err(_) => ("magnet_download".to_string(), false)
        };

        let id = Uuid::new_v4().to_string();
        let user_id = self.get_user_id_by_username(username).await
            .ok_or_else(|| AppError::NotFound("User not found".to_string()))?;
        
        let base_save_dir = if let Some(p) = req.path {
            let relative_path = p.trim_start_matches('/');
            format!("{}/vol1/User/{}/{}", self.storage_path, username, relative_path)
        } else {
            let default_dir = format!("{}/vol1/User/{}/下载", self.storage_path, username);
            self.ensure_directory_exists(&user_id, "/下载").await;
            default_dir
        };

        let save_dir = if !is_single_file {
            format!("{}/{}", base_save_dir, torrent_name)
        } else {
            base_save_dir
        };

        let _ = tokio::fs::create_dir_all(&save_dir).await;
        
        let opts = AddTorrentOptions {
            output_folder: Some(save_dir.clone()),
            only_files: Some(req.files),
            overwrite: true,
            ..Default::default()
        };
        
        let magnet_url = enrich_magnet_link(&format!("magnet:?xt=urn:btih:{}", req.token));
        
        match self.torrent_session.add_torrent(AddTorrent::from_url(magnet_url.clone()), Some(opts)).await {
            Ok(resp) => {
                if let Some(handle) = resp.into_handle() {
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
                        created_at: Utc::now().naive_utc(),
                        updated_at: Utc::now().naive_utc(),
                        error_msg: None,
                    };
                    
                    sqlx::query(
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
                    .execute(&self.db)
                    .await?;
                    
                    self.spawn_monitor_torrent(id, handle, user_id, username.to_string(), save_dir).await;
                    Ok(())
                } else {
                    Err(AppError::Internal("Failed to get torrent handle".to_string()))
                }
            },
            Err(e) => Err(AppError::Internal(format!("Failed to start download: {}", e))),
        }
    }
}

impl DownloaderServiceImpl {
    async fn spawn_magnet_download(&self, id: String, url: String, save_dir: String, user_id: String, username: String) {
        let db = self.db.clone();
        let torrent_session = self.torrent_session.clone();
        let download_tasks = self.download_tasks.clone();
        let storage_path = self.storage_path.clone();

        let id_clone = id.clone();
        let handle = tokio::spawn(async move {
            let url = enrich_magnet_link(&url);
            let opts = AddTorrentOptions {
                output_folder: Some(save_dir.clone()),
                overwrite: true,
                ..Default::default()
            };

            let handle = match torrent_session.add_torrent(AddTorrent::from_url(url.clone()), Some(opts)).await {
                Ok(h) => h.into_handle().unwrap(),
                Err(e) => {
                    let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                        .bind(e.to_string())
                        .bind(&id_clone)
                        .execute(&db)
                        .await;
                    return;
                }
            };
            
            let prefix = format!("{}/vol1/User/{}", storage_path, username);
            let virtual_base_dir = if save_dir.starts_with(&prefix) {
                save_dir[prefix.len()..].to_string()
            } else {
                "/下载".to_string()
            };
            
            monitor_torrent_process_internal(db, torrent_session, download_tasks, id_clone, handle, user_id, virtual_base_dir, storage_path, username).await;
        });

        {
            let mut tasks = self.download_tasks.lock().await;
            tasks.insert(id, handle.abort_handle());
        }
    }

    async fn spawn_monitor_torrent(&self, id: String, handle: Arc<ManagedTorrent>, user_id: String, username: String, save_dir: String) {
        let db = self.db.clone();
        let torrent_session = self.torrent_session.clone();
        let download_tasks = self.download_tasks.clone();
        let storage_path = self.storage_path.clone();

        let id_clone = id.clone();
        let abort_handle = tokio::spawn(async move {
            let prefix = format!("{}/vol1/User/{}", storage_path, username);
            let virtual_base_dir = if save_dir.starts_with(&prefix) {
                save_dir[prefix.len()..].to_string()
            } else {
                "/下载".to_string()
            };
            monitor_torrent_process_internal(db, torrent_session, download_tasks, id_clone, handle, user_id, virtual_base_dir, storage_path, username).await;
        });
        
        {
            let mut tasks = self.download_tasks.lock().await;
            tasks.insert(id, abort_handle.abort_handle());
        }
    }

    async fn spawn_http_download(&self, id: String, url: String, path: String, user_id: String) {
        let db = self.db.clone();
        let download_tasks = self.download_tasks.clone();
        
        let id_clone = id.clone();
        let handle = tokio::spawn(async move {
            download_process_internal(db, download_tasks, id_clone, url, path, user_id).await;
        });

        {
            let mut tasks = self.download_tasks.lock().await;
            tasks.insert(id, handle.abort_handle());
        }
    }
}

async fn sync_file_to_db_internal(db: &Pool<Sqlite>, user_id: &str, physical_path: &Path, virtual_dir: &str) {
    if !physical_path.exists() { return; }
    
    let name = physical_path.file_name().unwrap_or_default().to_string_lossy().to_string();
    let size = tokio::fs::metadata(physical_path).await.map(|m| m.len()).unwrap_or(0);
    
    let exists: bool = sqlx::query_scalar("select count(*) > 0 from cloud_files where user_id = $1 and dir = $2 and name = $3")
        .bind(user_id)
        .bind(virtual_dir)
        .bind(&name)
        .fetch_one(db)
        .await
        .unwrap_or(false);
        
    if exists {
        let _ = sqlx::query("update cloud_files set size = $1, updated_at = datetime('now') where user_id = $2 and dir = $3 and name = $4")
            .bind(size as i64)
            .bind(user_id)
            .bind(virtual_dir)
            .bind(&name)
            .execute(db)
            .await;
    } else {
        let id = Uuid::new_v4().to_string();
        let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage, created_at, updated_at) values ($1, $2, $3, $4, $5, 'file', datetime('now'), datetime('now'))")
            .bind(id)
            .bind(user_id)
            .bind(&name)
            .bind(virtual_dir)
            .bind(size as i64)
            .execute(db)
            .await;
    }
}

async fn ensure_directory_exists_internal(db: &Pool<Sqlite>, user_id: &str, virtual_dir: &str) {
    if virtual_dir == "/" { return; }
    
    let parts: Vec<&str> = virtual_dir.split('/').filter(|x| !x.is_empty()).collect();
    let mut current_path = "/".to_string();
    
    for part in parts {
        let name = part;
        let parent_dir = current_path.clone();
        
        let exists: bool = sqlx::query_scalar("select count(*) > 0 from cloud_files where user_id = $1 and dir = $2 and name = $3 and storage = 'dir'")
            .bind(user_id)
            .bind(&parent_dir)
            .bind(name)
            .fetch_one(db)
            .await
            .unwrap_or(false);
            
        if !exists {
            let id = Uuid::new_v4().to_string();
            let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage, created_at, updated_at) values ($1, $2, $3, $4, 0, 'dir', datetime('now'), datetime('now'))")
                .bind(id)
                .bind(user_id)
                .bind(name)
                .bind(&parent_dir)
                .execute(db)
                .await;
        }
        
        if current_path == "/" {
            current_path = format!("/{}", name);
        } else {
            current_path = format!("{}/{}", current_path, name);
        }
    }
}

async fn download_process_internal(db: Pool<Sqlite>, download_tasks: Arc<Mutex<HashMap<String, AbortHandle>>>, id: String, url: String, path: String, user_id: String) {
    let mut downloaded_offset: u64 = 0;
    if let Ok(metadata) = tokio::fs::metadata(&path).await {
        downloaded_offset = metadata.len();
    }

    let client = reqwest::Client::builder()
        .user_agent("Wget/1.21.2") 
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    
    let _ = sqlx::query("update downloads set status = 'downloading' where id = $1")
        .bind(&id)
        .execute(&db)
        .await;

    let mut req = client.get(&url);
    if downloaded_offset > 0 {
        req = req.header("Range", format!("bytes={}-", downloaded_offset));
    }
    
    let mut response = match req.send().await {
        Ok(r) => {
             if !r.status().is_success() {
                let msg = format!("HTTP error: {}", r.status());
                let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                    .bind(msg)
                    .bind(&id)
                    .execute(&db)
                    .await;
                {
                    let mut tasks = download_tasks.lock().await;
                    tasks.remove(&id);
                }
                return;
             }
             r
        },
        Err(e) => {
            let msg = format!("Request failed: {}", e);
            let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                .bind(msg)
                .bind(&id)
                .execute(&db)
                .await;
            {
                let mut tasks = download_tasks.lock().await;
                tasks.remove(&id);
            }
            return;
        }
    };

    let total_size = response.content_length().unwrap_or(0) + downloaded_offset;

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
                        .execute(&db)
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
                        .execute(&db)
                        .await;

                    last_update = Instant::now();
                    last_downloaded = downloaded;
                }
            },
            Ok(None) => break,
            Err(e) => {
                let msg = format!("Stream error: {}", e);
                let _ = sqlx::query("update downloads set status = 'error', error_msg = $1 where id = $2")
                    .bind(msg)
                    .bind(&id)
                    .execute(&db)
                    .await;
                return;
            }
        }
    }

    let _ = sqlx::query("update downloads set status = 'done', progress = 100.0, speed = 0, updated_at = datetime('now') where id = $1")
        .bind(&id)
        .execute(&db)
        .await;

    let virtual_dir = "/下载";
    let physical_path = Path::new(&path);
    sync_file_to_db_internal(&db, &user_id, physical_path, virtual_dir).await;

    let mut tasks = download_tasks.lock().await;
    tasks.remove(&id);
}

async fn monitor_torrent_process_internal(
    db: Pool<Sqlite>,
    torrent_session: Arc<Session>,
    download_tasks: Arc<Mutex<HashMap<String, AbortHandle>>>,
    id: String,
    handle: Arc<ManagedTorrent>,
    user_id: String,
    virtual_base_dir: String,
    storage_path: String,
    username: String,
) {
    let _ = sqlx::query("update downloads set status = 'downloading' where id = $1")
        .bind(&id)
        .execute(&db)
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
            .execute(&db)
            .await;
    }
    
    let _ = sqlx::query("update downloads set status = 'done', progress = 100.0, speed = 0, updated_at = datetime('now') where id = $1")
        .bind(&id)
        .execute(&db)
        .await;

    let api = librqbit::Api::new(torrent_session.clone(), None);
    if let Ok(details) = api.api_torrent_details(librqbit::api::TorrentIdOrHash::Hash(handle.info_hash())) {
         if let Some(files) = details.files {
            if !username.is_empty() {
                let physical_base = format!("{}/vol1/User/{}{}", storage_path, username, virtual_base_dir);
                let base_path = Path::new(&physical_base);
                
                for file in files {
                     let relative_path_str = file.name;
                     let relative_path = Path::new(&relative_path_str);
                     let full_physical_path = base_path.join(relative_path);
                     
                     let full_virtual_path_buf = Path::new(&virtual_base_dir).join(relative_path);
                     let virtual_dir = full_virtual_path_buf.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
                     let virtual_dir = if virtual_dir.starts_with('/') { virtual_dir } else { format!("/{}", virtual_dir) };
                     
                     ensure_directory_exists_internal(&db, &user_id, &virtual_dir).await;
                     sync_file_to_db_internal(&db, &user_id, &full_physical_path, &virtual_dir).await;
                }
            }
         }
    }

    {
        let mut tasks = download_tasks.lock().await;
        tasks.remove(&id);
    }
}

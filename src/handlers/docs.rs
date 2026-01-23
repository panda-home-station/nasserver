use axum::{
    extract::{Extension, Multipart, Query, State},
    response::IntoResponse,
    Json,
};
use axum::body::Body;
use axum::response::Response;
use tokio_util::io::ReaderStream;
use tokio::io::{AsyncWriteExt, AsyncSeekExt};
use tokio::fs::{self as tokio_fs, OpenOptions};
use std::io::SeekFrom;
use std::path::Path;
use sqlx::Row;

use crate::state::AppState;
use crate::models::auth::AuthUser;
use crate::models::docs::{DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery};
use sha2::Digest;
use serde::Deserialize;

fn normalize_path(p: &str) -> String {
    let s = if p.starts_with('/') { &p[1..] } else { p };
    let s = s.replace("\\", "/");
    let parts: Vec<&str> = s.split('/').filter(|x| !x.is_empty() && *x != "." && *x != "..").collect();
    format!("/{}", parts.join("/"))
}

pub async fn list(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsListQuery>) -> impl IntoResponse {
    let dir = normalize_path(&q.path.unwrap_or_else(|| "/".to_string()));
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let page_size = limit + 1;
    let rows = sqlx::query("select id, name, storage, size, strftime('%s', coalesce(updated_at, created_at)) as ts from cloud_files where user_id = $1 and dir = $2 order by storage desc, name asc limit $3 offset $4")
        .bind(user.user_id.to_string())
        .bind(&dir)
        .bind(page_size)
        .bind(offset)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
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
        "AppData",
        "Favorites",
        "MyShares",
        "PublicLinks",
        "Recent",
        "SharedWithMe",
        "Team",
        "Trash",
    ];
    let top_name = Path::new(&dir).components().next().and_then(|c| {
        use std::path::Component;
        match c {
            Component::RootDir => None,
            Component::Normal(os) => os.to_str(),
            _ => None,
        }
    }).unwrap_or("");
    if reserved.contains(&top_name) && dir == format!("/{}", top_name) {
        return Json(serde_json::json!({ "ok": true }));
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
    let exists: Option<(i64,)> = sqlx::query_as("select 1 from cloud_files where user_id = $1 and dir = $2 and name = $3 and storage = 'dir' limit 1")
        .bind(user.user_id.to_string())
        .bind(&parent)
        .bind(&name)
        .fetch_optional(&state.db)
        .await
        .unwrap_or(None);
    if exists.is_some() {
        return Json(serde_json::json!({ "ok": true }));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, storage) values ($1, $2, $3, $4, 0, 'dir')")
        .bind(&id)
        .bind(user.user_id.to_string())
        .bind(&name)
        .bind(&parent)
        .execute(&state.db)
        .await;
    Json(serde_json::json!({ "ok": true, "id": id }))
}

pub async fn upload(State(state): State<AppState>, Extension(user): Extension<AuthUser>, mut multipart: Multipart) -> impl IntoResponse {
    println!("[upload] start upload request for user: {}", user.user_id);
    let mut dest_path = "/".to_string();
    let mut saved_name: Option<String> = None;
    let mut expected_size: Option<u64> = None;
    let mut upload_id = String::new();
    let mut offset: u64 = 0;
    let mut checksum_opt: Option<String> = None;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        println!("[upload] processing field: {}", name);
        if name == "path" {
            dest_path = normalize_path(&field.text().await.unwrap_or("/".to_string()));
            println!("[upload] dest_path: {}", dest_path);
        } else if name == "size" {
            if let Ok(s) = field.text().await.unwrap_or_default().parse::<u64>() {
                expected_size = Some(s);
                println!("[upload] expected_size: {}", s);
            }
        } else if name == "upload_id" {
            upload_id = field.text().await.unwrap_or_default();
            println!("[upload] upload_id: {}", upload_id);
        } else if name == "offset" {
            if let Ok(o) = field.text().await.unwrap_or_default().parse::<u64>() {
                offset = o;
                println!("[upload] offset: {}", offset);
            }
        } else if name == "checksum" {
            let cs = field.text().await.unwrap_or_default();
            if !cs.is_empty() {
                checksum_opt = Some(cs);
            }
            if let Some(csx) = checksum_opt.as_ref() {
                println!("[upload] received checksum: {}", csx);
            }
        } else if name == "file" {
            let file_name = field.file_name().map(|s| s.to_string()).unwrap_or("upload.bin".to_string());
            println!("[upload] start processing file: {}", file_name);
            let blobs_root = Path::new(&state.storage_path).join("vol1").join("blobs");
            let _ = std::fs::create_dir_all(&blobs_root);
            let pending_dir = blobs_root.join("pending");
            let _ = std::fs::create_dir_all(&pending_dir);
            
            if offset == 0 {
                if let Some(cs) = checksum_opt.as_ref() {
                    let final_path = blobs_root.join(cs);
                    let exists = tokio_fs::metadata(&final_path).await.ok().map(|m| m.is_file()).unwrap_or(false);
                    let pending = tokio_fs::metadata(pending_dir.join(cs)).await.ok().map(|m| m.is_file()).unwrap_or(false);
                    println!("[upload] pre-file short-circuit check: exists={} pending={} checksum={}", exists, pending, cs);
                    if exists || pending {
                        let id = uuid::Uuid::new_v4().to_string();
                        let parent = {
                            let p = Path::new(&dest_path);
                            let pp = p.to_string_lossy().to_string();
                            if pp.is_empty() { "/".to_string() } else { pp }
                        };
                        let sz = expected_size.map(|x| x as i64).unwrap_or(0);
                        let res = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, checksum, storage) values ($1, $2, $3, $4, $5, $6, $7, 'blob')")
                            .bind(&id)
                            .bind(user.user_id.to_string())
                            .bind(&file_name)
                            .bind(&parent)
                            .bind(sz)
                            .bind("")
                            .bind(cs)
                            .execute(&state.db)
                            .await;
                        if res.is_ok() {
                            println!("[upload] rapid by checksum: id={}, name={}, parent={} size={}", id, file_name, parent, sz);
                            return Json(serde_json::json!({ "ok": true, "done": true, "bytes": sz }));
                        } else {
                            println!("[upload] rapid by checksum insert failed, fallback to normal upload");
                        }
                    }
                }
            }
            
            let tmp_id = if !upload_id.is_empty() { upload_id.clone() } else { uuid::Uuid::new_v4().to_string() };
            let tmp_path = blobs_root.join(format!("{}.part", tmp_id));
            println!("[upload] tmp_path: {:?}", tmp_path);
            
            if offset == 0 {
                if let Some(cs) = checksum_opt.as_ref() {
                    let _ = tokio_fs::File::create(pending_dir.join(cs)).await;
                    println!("[upload] created pending mark for checksum {}", cs);
                }
            }
            
            let mut f = if offset > 0 {
                if let Ok(mut file) = OpenOptions::new().write(true).create(true).open(&tmp_path).await {
                    if let Err(e) = file.seek(SeekFrom::Start(offset)).await {
                        println!("[upload] seek failed: {}", e);
                        return Json(serde_json::json!({ "ok": false, "error": "seek failed" }));
                    }
                    file
                } else {
                    println!("[upload] open for append failed");
                    return Json(serde_json::json!({ "ok": false, "error": "open failed" }));
                }
            } else {
                match tokio_fs::File::create(&tmp_path).await {
                    Ok(h) => h,
                    Err(e) => {
                        println!("[upload] create failed: {}", e);
                        return Json(serde_json::json!({ "ok": false, "error": "create failed" }));
                    },
                }
            };

            let mut hasher_opt = if offset == 0 {
                Some(sha2::Sha256::new())
            } else {
                None
            };

            let mut written_this_chunk: u64 = 0;
            while let Ok(Some(chunk)) = field.chunk().await {
                written_this_chunk += chunk.len() as u64;
                if let Err(e) = f.write_all(&chunk).await {
                    println!("[upload] write failed: {}", e);
                    return Json(serde_json::json!({ "ok": false, "error": "write failed" }));
                }
                if let Some(h) = hasher_opt.as_mut() {
                    h.update(&chunk);
                }
            }
            println!("[upload] written_this_chunk: {}", written_this_chunk);
            
            // Check total size
            let current_size = match tokio_fs::metadata(&tmp_path).await {
                Ok(m) => m.len(),
                Err(_) => offset + written_this_chunk, // fallback
            };
            println!("[upload] current_size: {}", current_size);

            if let Some(es) = expected_size {
                if current_size != es {
                    // Not finished yet, or size mismatch
                    // Do NOT delete the file, so we can resume
                    // But return ok so client knows chunk is saved
                    println!("[upload] partial upload: current={} expected={}", current_size, es);
                    return Json(serde_json::json!({ "ok": true, "done": false, "bytes": current_size }));
                }
            }

            // Calculate hash of the FULL file
            // Note: This is expensive for large files to do at the end, but necessary if we didn't hash incrementally.
            // Since we supported resume, incremental hashing state is lost unless we save it. 
            // So we must re-read the file to hash it.
            let digest = if let Some(h) = hasher_opt {
                 println!("[upload] using incremental hash");
                 h.finalize()
            } else {
                println!("[upload] calculating hash (full re-read)...");
                let mut hasher = sha2::Sha256::new();
                if let Ok(mut file) = tokio_fs::File::open(&tmp_path).await {
                    // Increase buffer size to 1MB for faster reading
                    let mut buffer = vec![0u8; 1024 * 1024]; 
                    loop {
                        let n = match tokio::io::AsyncReadExt::read(&mut file, &mut buffer).await {
                            Ok(n) if n == 0 => break,
                            Ok(n) => n,
                            Err(_) => break,
                        };
                        hasher.update(&buffer[..n]);
                    }
                }
                hasher.finalize()
            };
            
            let mut hex = String::with_capacity(digest.len() * 2);
            for b in digest {
                hex.push_str(&format!("{:02x}", b));
            }
            println!("[upload] hash: {}", hex);

            let final_path = blobs_root.join(&hex);
            let existed_before = tokio_fs::metadata(&final_path).await.ok().map(|m| m.is_file()).unwrap_or(false);
            if let Err(e) = tokio_fs::rename(&tmp_path, &final_path).await {
                println!("[upload] rename failed: {}", e);
            } else {
                println!("[upload] finalize blob: path={:?} existed_before={}", final_path, existed_before);
            }
            if let Some(cs) = checksum_opt.as_ref() {
                let _ = tokio_fs::remove_file(pending_dir.join(cs)).await;
                println!("[upload] removed pending mark for checksum {}", cs);
            }
            
            let id = uuid::Uuid::new_v4().to_string();
            let parent = {
                let p = Path::new(&dest_path);
                let pp = p.to_string_lossy().to_string();
                if pp.is_empty() { "/".to_string() } else { pp }
            };
            
            println!("[upload] inserting db record: id={}, name={}, parent={}", id, file_name, parent);
            let res = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, checksum, storage) values ($1, $2, $3, $4, $5, $6, $7, 'blob')")
                .bind(&id)
                .bind(user.user_id.to_string())
                .bind(&file_name)
                .bind(&parent)
                .bind(current_size as i64)
                .bind("")
                .bind(&hex)
                .execute(&state.db)
                .await;
            
            if let Err(e) = res {
                println!("[upload] db insert failed: {}", e);
            } else {
                println!("[upload] db insert success");
            }

            saved_name = Some(file_name);
            
            // Return success with bytes
            return Json(serde_json::json!({ "ok": true, "done": true, "bytes": current_size }));
        }
    }
    if saved_name.is_none() {
        return Json(serde_json::json!({ "ok": false }));
    }
    Json(serde_json::json!({ "ok": true }))
}

#[derive(Deserialize)]
pub struct RapidUploadReq {
    pub path: String,
    pub name: String,
    pub size: i64,
    pub checksum: Option<String>,
}

pub async fn rapid_upload(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(req): Json<RapidUploadReq>) -> impl IntoResponse {
    let dir = normalize_path(&req.path);
    let blobs_root = Path::new(&state.storage_path).join("vol1").join("blobs");
    let pending_dir = blobs_root.join("pending");
    let mut checksum_use: Option<String> = None;
    if let Some(cs) = req.checksum.as_ref() {
        if !cs.is_empty() {
            let blob_path = blobs_root.join(cs);
            let exists = tokio_fs::metadata(&blob_path).await.ok().map(|m| m.is_file()).unwrap_or(false);
            let pending = tokio_fs::metadata(pending_dir.join(cs)).await.ok().map(|m| m.is_file()).unwrap_or(false);
            println!("[rapid-upload] user={} name={} dir={} size={} checksum={} exists={}", user.user_id, req.name, dir, req.size, cs, exists);
            if exists || pending {
                checksum_use = Some(cs.clone());
            }
        }
    }
    if checksum_use.is_none() {
        let found = sqlx::query("select checksum from cloud_files where user_id = $1 and name = $2 and size = $3 and storage = 'blob' order by updated_at desc limit 1")
            .bind(user.user_id.to_string())
            .bind(&req.name)
            .bind(req.size)
            .fetch_optional(&state.db)
            .await
            .ok()
            .and_then(|row_opt| row_opt.map(|row| row.try_get::<String, _>("checksum").unwrap_or_default()))
            .filter(|cs| !cs.is_empty());
        if let Some(cs) = found {
            checksum_use = Some(cs);
        } else {
            return Json(serde_json::json!({ "ok": true, "rapid": false }));
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    let res = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, checksum, storage) values ($1, $2, $3, $4, $5, $6, $7, 'blob')")
        .bind(&id)
        .bind(user.user_id.to_string())
        .bind(&req.name)
        .bind(&dir)
        .bind(req.size)
        .bind("")
        .bind(&checksum_use.unwrap())
        .execute(&state.db)
        .await;
    if res.is_ok() {
        println!("[rapid-upload] insert success: id={} name={} dir={}", id, req.name, dir);
    } else {
        println!("[rapid-upload] insert failed; fallback to normal upload");
    }
    Json(serde_json::json!({ "ok": res.is_ok(), "rapid": res.is_ok() }))
}

pub async fn download(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsDownloadQuery>) -> Response {
    let rec = if let Some(id) = &q.id {
        sqlx::query("select name, checksum from cloud_files where id = $1 and user_id = $2 and storage = 'blob'")
            .bind(id)
            .bind(user.user_id.to_string())
            .fetch_one(&state.db)
            .await
    } else if let Some(path) = &q.path {
        let np = normalize_path(path);
        let parent = {
            let p = Path::new(&np);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        sqlx::query("select name, checksum from cloud_files where user_id = $1 and dir = $2 and name = $3 and storage = 'blob'")
            .bind(user.user_id.to_string())
            .bind(&parent)
            .bind(&name)
            .fetch_one(&state.db)
            .await
    } else {
        Err(sqlx::Error::RowNotFound)
    };
    if let Ok(row) = rec {
        let name: String = row.try_get("name").unwrap_or("download.bin".to_string());
        let checksum: String = row.try_get("checksum").unwrap_or_default();
        let blobs_root = Path::new(&state.storage_path).join("vol1").join("blobs");
        let path = blobs_root.join(&checksum);
        let file = match tokio_fs::File::open(&path).await {
            Ok(f) => f,
            Err(_) => {
                return Response::builder().status(404).body(Body::empty()).unwrap();
            }
        };
        let meta = file.metadata().await.ok();
        let len = meta.map(|m| m.len()).unwrap_or(0);
        let stream = ReaderStream::with_capacity(file, 8192 * 16);
        let body = Body::from_stream(stream);
        return Response::builder()
            .status(200)
            .header("content-type", "application/octet-stream")
            .header("content-disposition", format!("attachment; filename=\"{}\"", name))
            .header("content-length", len.to_string())
            .body(body)
            .unwrap();
    }
    Response::builder().status(404).body(Body::empty()).unwrap()
}

pub async fn rename(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Json(req): Json<DocsRenameReq>) -> impl IntoResponse {
    if let (Some(from), Some(to)) = (&req.from, &req.to) {
        let nfrom = normalize_path(from);
        let nto = normalize_path(to);
        let from_parent = {
            let p = Path::new(&nfrom);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let from_name = Path::new(&nfrom).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        let to_parent = {
            let p = Path::new(&nto);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let to_name = Path::new(&nto).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        if from_name.is_empty() || to_name.is_empty() {
            return Json(serde_json::json!({ "ok": false }));
        }
        let res = sqlx::query("update cloud_files set dir = $1, name = $2 where user_id = $3 and dir = $4 and name = $5")
            .bind(&to_parent)
            .bind(&to_name)
            .bind(user.user_id.to_string())
            .bind(&from_parent)
            .bind(&from_name)
            .execute(&state.db)
            .await;
        return Json(serde_json::json!({ "ok": res.is_ok() }));
    }
    let mut ok = false;
    if let Some(id) = &req.id {
        if let Some(new_name) = &req.new_name {
            let res = sqlx::query("update cloud_files set name = $1 where id = $2 and user_id = $3")
                .bind(new_name)
                .bind(id)
                .bind(user.user_id.to_string())
                .execute(&state.db)
                .await;
            ok = ok || res.is_ok();
        }
        if let Some(new_dir) = &req.new_dir {
            let ndir = normalize_path(new_dir);
            let res = sqlx::query("update cloud_files set dir = $1 where id = $2 and user_id = $3")
                .bind(&ndir)
                .bind(id)
                .bind(user.user_id.to_string())
                .execute(&state.db)
                .await;
            ok = ok || res.is_ok();
        }
    }
    Json(serde_json::json!({ "ok": ok }))
}

pub async fn delete(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsDeleteQuery>) -> impl IntoResponse {
    let rec = if let Some(id) = &q.id {
        sqlx::query("select name, dir, storage, checksum from cloud_files where id = $1 and user_id = $2")
            .bind(id)
            .bind(user.user_id.to_string())
            .fetch_one(&state.db)
            .await
    } else if let Some(path) = &q.path {
        let np = normalize_path(path);
        let parent = {
            let p = Path::new(&np);
            let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
            if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
        };
        let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
        sqlx::query("select name, dir, storage, checksum from cloud_files where user_id = $1 and dir = $2 and name = $3")
            .bind(user.user_id.to_string())
            .bind(&parent)
            .bind(&name)
            .fetch_one(&state.db)
            .await
    } else {
        Err(sqlx::Error::RowNotFound)
    };
    if let Ok(row) = rec {
        let storage: String = row.try_get("storage").unwrap_or_default();
        let checksum: String = row.try_get("checksum").unwrap_or_default();
        let name: String = row.try_get("name").unwrap_or_default();
        let dir_cur: String = row.try_get("dir").unwrap_or("/".to_string());
        let dir_cur = if dir_cur.is_empty() { "/".to_string() } else { normalize_path(&dir_cur) };
        let full_path = normalize_path(&format!("{}/{}", dir_cur, name));
        if storage == "dir" {
            let from_prefix = full_path.clone();
            let to_prefix = normalize_path(&format!("/Trash{}", full_path));
            let _ = sqlx::query("update cloud_files set dir = replace(dir, $1, $2), updated_at = datetime('now') where user_id = $3 and dir like ($1 || '/%')")
                .bind(&from_prefix)
                .bind(&to_prefix)
                .bind(user.user_id.to_string())
                .execute(&state.db)
                .await;
            let trash_parent = normalize_path(&format!("/Trash{}", dir_cur));
            let _ = sqlx::query("update cloud_files set dir = $1, updated_at = datetime('now') where user_id = $2 and dir = $3 and name = $4")
                .bind(&trash_parent)
                .bind(user.user_id.to_string())
                .bind(&dir_cur)
                .bind(&name)
                .execute(&state.db)
                .await;
        } else {
            let trash_parent = normalize_path(&format!("/Trash{}", dir_cur));
            let _ = sqlx::query("update cloud_files set dir = $1, updated_at = datetime('now') where user_id = $2 and dir = $3 and name = $4")
                .bind(&trash_parent)
                .bind(user.user_id.to_string())
                .bind(&dir_cur)
                .bind(&name)
                .execute(&state.db)
                .await;
        }
        return Json(serde_json::json!({ "ok": true }));
    }
    Json(serde_json::json!({ "ok": false }))
}

use axum::{
    extract::{Extension, Multipart, Query},
    http::HeaderValue,
    http::HeaderMap,
    response::IntoResponse,
    Json,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use tokio::io::AsyncWriteExt;
use tokio::fs as tokio_fs;

use crate::models::auth::AuthUser;
use crate::models::fs::{FsListQuery, FsListResp, FsEntry, FsMkdirReq, FsDeleteQuery, FsRenameReq, FsDownloadQuery};

pub async fn fs_list(Extension(user): Extension<AuthUser>, Query(q): Query<FsListQuery>) -> impl IntoResponse {
    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
    let req_path = q.path.unwrap_or_else(|| "/".to_string());

    let norm = if req_path.starts_with('/') {
        &req_path[1..]
    } else {
        req_path.as_str()
    };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let target_abs = match fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => {
            return Json(FsListResp {
                base: base_abs.display().to_string(),
                path: req_path,
                entries: vec![],
            });
        }
    };
    if !target_abs.starts_with(&base_abs) {
        return Json(FsListResp {
            base: base_abs.display().to_string(),
            path: req_path,
            entries: vec![],
        });
    }

    let mut entries: Vec<FsEntry> = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&target_abs) {
        for ent in read_dir.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            let md = match ent.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let is_dir = md.is_dir();
            let size = if is_dir { 0 } else { md.len() };
            let modified_ts = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push(FsEntry {
                name,
                is_dir,
                size,
                modified_ts,
            });
        }
    }
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });
    Json(FsListResp {
        base: base_abs.display().to_string(),
        path: req_path,
        entries,
    })
}

pub async fn fs_mkdir(Extension(user): Extension<AuthUser>, Json(req): Json<FsMkdirReq>) -> impl IntoResponse {
    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
    let req_path = req.path;
    let norm = if req_path.starts_with('/') { &req_path[1..] } else { req_path.as_str() };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let parent = joined.parent().unwrap_or(Path::new(&base)).to_path_buf();
    let target_abs = match fs::canonicalize(&parent) {
        Ok(p) => p.join(joined.file_name().unwrap_or_default()),
        Err(_) => joined,
    };
    if !target_abs.starts_with(&base_abs) {
        return Json(serde_json::json!({ "ok": false }));
    }
    let _ = fs::create_dir_all(&target_abs);
    Json(serde_json::json!({ "ok": true }))
}

pub async fn fs_delete(Extension(user): Extension<AuthUser>, Query(q): Query<FsDeleteQuery>) -> impl IntoResponse {
    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
    let req_path = q.path;
    let norm = if req_path.starts_with('/') { &req_path[1..] } else { req_path.as_str() };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let target_abs = match fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => joined.clone(),
    };
    if !target_abs.starts_with(&base_abs) {
        return Json(serde_json::json!({ "ok": false }));
    }
    let md = fs::metadata(&target_abs);
    if let Ok(m) = md {
        if m.is_dir() {
            let _ = fs::remove_dir_all(&target_abs);
        } else {
            let _ = fs::remove_file(&target_abs);
        }
        return Json(serde_json::json!({ "ok": true }));
    }
    Json(serde_json::json!({ "ok": false }))
}

pub async fn fs_rename(Extension(user): Extension<AuthUser>, Json(req): Json<FsRenameReq>) -> impl IntoResponse {
    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);

    let norm_from = if req.from.starts_with('/') { &req.from[1..] } else { req.from.as_str() };
    let from_joined: PathBuf = Path::new(&base).join(norm_from);
    let norm_to = if req.to.starts_with('/') { &req.to[1..] } else { req.to.as_str() };
    let to_joined: PathBuf = Path::new(&base).join(norm_to);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let from_abs = match fs::canonicalize(&from_joined) {
        Ok(p) => p,
        Err(_) => from_joined.clone(),
    };
    let to_parent = to_joined.parent().unwrap_or(Path::new(&base)).to_path_buf();
    let to_abs = match fs::canonicalize(&to_parent) {
        Ok(p) => p.join(to_joined.file_name().unwrap_or_default()),
        Err(_) => to_joined.clone(),
    };
    if !from_abs.starts_with(&base_abs) || !to_abs.starts_with(&base_abs) {
        return Json(serde_json::json!({ "ok": false }));
    }
    let _ = fs::create_dir_all(to_parent);
    let ok = fs::rename(&from_abs, &to_abs).is_ok();
    Json(serde_json::json!({ "ok": ok }))
}

pub async fn fs_download(Extension(user): Extension<AuthUser>, Query(q): Query<FsDownloadQuery>) -> impl IntoResponse {
    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
    let req_path = q.path;
    let norm = if req_path.starts_with('/') { &req_path[1..] } else { req_path.as_str() };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let target_abs = match fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => joined.clone(),
    };
    if !target_abs.starts_with(&base_abs) {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/octet-stream"));
        return (headers, Vec::<u8>::new());
    }
    let data = fs::read(&target_abs).unwrap_or_default();
    let name = target_abs.file_name().and_then(|n| n.to_str()).unwrap_or("download.bin");
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/octet-stream"));
    let cd = format!("attachment; filename=\"{}\"", name);
    if let Ok(v) = HeaderValue::from_str(&cd) {
        headers.insert("content-disposition", v);
    }
    (headers, data)
}

pub async fn fs_upload(Extension(user): Extension<AuthUser>, mut multipart: Multipart) -> impl IntoResponse {
    let mut dest_path = "/".to_string();
    let mut wrote = false;
    let mut total_written: usize = 0;
    let mut saved_name: Option<String> = None;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "path" {
            dest_path = field.text().await.unwrap_or("/".to_string());
        } else if name == "file" {
            let file_name = field.file_name().map(|s| s.to_string()).unwrap_or("upload.bin".to_string());
            let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
            let base = format!("{}/users/{}", base_root, user.user_id);
            let norm = if dest_path.starts_with('/') { &dest_path[1..] } else { dest_path.as_str() };
            let dir_joined: PathBuf = Path::new(&base).join(norm);
            let base_abs = std::fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
            let dir_abs = match std::fs::canonicalize(&dir_joined) {
                Ok(p) => p,
                Err(_) => dir_joined.clone(),
            };
            if !dir_abs.starts_with(&base_abs) {
                return Json(serde_json::json!({ "ok": false }));
            }
            let _ = std::fs::create_dir_all(&dir_abs);
            let target_abs = dir_abs.join(&file_name);
            let mut f = match tokio_fs::File::create(&target_abs).await {
                Ok(h) => h,
                Err(e) => {
                    println!("fs_upload: create file failed: {}", e);
                    return Json(serde_json::json!({ "ok": false }));
                }
            };
            println!("fs_upload: user={} dest={} name={} starting", user.user_id, dest_path, file_name);
            while let Ok(Some(chunk)) = field.chunk().await {
                total_written += chunk.len();
                if let Err(e) = f.write_all(&chunk).await {
                    println!("fs_upload: write chunk failed: {}", e);
                    return Json(serde_json::json!({ "ok": false }));
                }
            }
            wrote = true;
            saved_name = Some(file_name);
            println!("fs_upload: finished bytes_len={}", total_written);
        }
    }

    if !wrote {
        println!("fs_upload: no file field received, dest={}", dest_path);
        return Json(serde_json::json!({ "ok": false }));
    }

    Json(serde_json::json!({ "ok": true, "name": saved_name.unwrap_or_default(), "bytes": total_written }))
}

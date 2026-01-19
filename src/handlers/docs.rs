use axum::{
    extract::{Extension, Multipart, Query, State},
    response::IntoResponse,
    Json,
};
use axum::body::Body;
use axum::response::Response;
use tokio_util::io::ReaderStream;
use tokio::io::AsyncWriteExt;
use tokio::fs as tokio_fs;
use std::path::Path;
use sqlx::Row;

use crate::state::AppState;
use crate::models::auth::AuthUser;
use crate::models::docs::{DocsListQuery, DocsListResp, DocsEntry, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery};
use sha2::Digest;

fn normalize_path(p: &str) -> String {
    let s = if p.starts_with('/') { &p[1..] } else { p };
    let s = s.replace("\\", "/");
    let parts: Vec<&str> = s.split('/').filter(|x| !x.is_empty() && *x != "." && *x != "..").collect();
    format!("/{}", parts.join("/"))
}

pub async fn list(State(state): State<AppState>, Extension(user): Extension<AuthUser>, Query(q): Query<DocsListQuery>) -> impl IntoResponse {
    let dir = normalize_path(&q.path.unwrap_or_else(|| "/".to_string()));
    let rows = sqlx::query("select id, name, storage, size, strftime('%s', coalesce(updated_at, created_at)) as ts from cloud_files where user_id = $1 and dir = $2 order by storage desc, name asc")
        .bind(user.user_id.to_string())
        .bind(&dir)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();
    let mut entries: Vec<DocsEntry> = Vec::new();
    for r in rows {
        let id: String = r.try_get("id").unwrap_or_default();
        let name: String = r.try_get("name").unwrap_or_default();
        let storage: String = r.try_get("storage").unwrap_or_default();
        let size: i64 = r.try_get("size").unwrap_or(0);
        let ts: i64 = r.try_get("ts").unwrap_or(0);
        entries.push(DocsEntry { id, name, is_dir: storage == "dir", size, modified_ts: ts });
    }
    Json(DocsListResp { path: dir, entries })
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
    let mut dest_path = "/".to_string();
    let mut saved_name: Option<String> = None;
    let mut total_written: usize = 0;
    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "path" {
            dest_path = normalize_path(&field.text().await.unwrap_or("/".to_string()));
        } else if name == "file" {
            let file_name = field.file_name().map(|s| s.to_string()).unwrap_or("upload.bin".to_string());
            let mut hasher = sha2::Sha256::new();
            let blobs_root = Path::new(&state.storage_path).join("vol1").join("blobs");
            let _ = std::fs::create_dir_all(&blobs_root);
            let tmp_id = uuid::Uuid::new_v4().to_string();
            let tmp_path = blobs_root.join(format!("{}.part", tmp_id));
            let mut f = match tokio_fs::File::create(&tmp_path).await {
                Ok(h) => h,
                Err(_) => return Json(serde_json::json!({ "ok": false })),
            };
            while let Ok(Some(chunk)) = field.chunk().await {
                total_written += chunk.len();
                hasher.update(&chunk);
                if let Err(_) = f.write_all(&chunk).await {
                    return Json(serde_json::json!({ "ok": false }));
                }
            }
            let digest = hasher.finalize();
            let mut hex = String::with_capacity(digest.len() * 2);
            for b in digest {
                hex.push_str(&format!("{:02x}", b));
            }
            let final_path = blobs_root.join(&hex);
            let _ = tokio_fs::rename(&tmp_path, &final_path).await;
            let id = uuid::Uuid::new_v4().to_string();
            let parent = {
                let p = Path::new(&dest_path);
                let pp = p.to_string_lossy().to_string();
                if pp.is_empty() { "/".to_string() } else { pp }
            };
            let _ = sqlx::query("insert into cloud_files (id, user_id, name, dir, size, mime, checksum, storage) values ($1, $2, $3, $4, $5, $6, $7, 'blob')")
                .bind(&id)
                .bind(user.user_id.to_string())
                .bind(&file_name)
                .bind(&parent)
                .bind(total_written as i64)
                .bind("")
                .bind(&hex)
                .execute(&state.db)
                .await;
            saved_name = Some(file_name);
        }
    }
    if saved_name.is_none() {
        return Json(serde_json::json!({ "ok": false }));
    }
    Json(serde_json::json!({ "ok": true, "bytes": total_written }))
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
        sqlx::query("select storage, checksum from cloud_files where id = $1 and user_id = $2")
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
        sqlx::query("select storage, checksum from cloud_files where user_id = $1 and dir = $2 and name = $3")
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
        if let Some(id) = &q.id {
            let _ = sqlx::query("delete from cloud_files where id = $1 and user_id = $2")
                .bind(id)
                .bind(user.user_id.to_string())
                .execute(&state.db)
                .await;
        } else if let Some(path) = &q.path {
            let np = normalize_path(path);
            let parent = {
                let p = Path::new(&np);
                let pp = p.parent().unwrap_or(Path::new("/")).to_string_lossy().to_string();
                if pp.is_empty() { "/".to_string() } else { if pp.starts_with('/') { pp } else { format!("/{}", pp) } }
            };
            let name = Path::new(&np).file_name().and_then(|x| x.to_str()).unwrap_or("").to_string();
            let _ = sqlx::query("delete from cloud_files where user_id = $1 and dir = $2 and name = $3")
                .bind(user.user_id.to_string())
                .bind(&parent)
                .bind(&name)
                .execute(&state.db)
                .await;
        }
        if storage == "blob" && !checksum.is_empty() {
            let cnt: i64 = sqlx::query_scalar("select count(*) from cloud_files where checksum = $1")
                .bind(&checksum)
                .fetch_one(&state.db)
                .await
                .unwrap_or(1);
            if cnt == 0 {
                let blobs_root = Path::new(&state.storage_path).join("vol1").join("blobs");
                let _ = std::fs::remove_file(blobs_root.join(&checksum));
            }
        }
        return Json(serde_json::json!({ "ok": true }));
    }
    Json(serde_json::json!({ "ok": false }))
}

use axum::{
    extract::{Multipart, Query, State, Extension},
    response::{IntoResponse, Response},
    Json,
};
use axum::body::Body;
use tokio_util::io::ReaderStream;
use tokio::fs::{self as tokio_fs};
use infra::AppState;
use domain::entities::auth::AuthUser;
use domain::storage::{DocsListResp, DocsListQuery, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery, InitiateMultipartReq, UploadPartQuery, CompleteMultipartReq};
use crate::error::ApiResult;
use tracing::error;

pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsListQuery>,
) -> ApiResult<Json<DocsListResp>> {
    let resp = state.storage_service.list(&user.username, q).await?;
    Ok(Json(resp))
}

pub async fn mkdir(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<DocsMkdirReq>,
) -> ApiResult<Json<serde_json::Value>> {
    state.storage_service.mkdir(&user.username, req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn initiate_multipart(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<InitiateMultipartReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let upload_id = state.storage_service.initiate_multipart_upload(&user.username, &req.path, &req.name).await?;
    Ok(Json(serde_json::json!({ "upload_id": upload_id })))
}

pub async fn upload_part(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<UploadPartQuery>,
    data: bytes::Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let etag = state.storage_service.save_file_part(&user.username, &q.upload_id, q.part_number, data).await?;
    Ok(Json(serde_json::json!({ "etag": etag })))
}

pub async fn complete_multipart(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CompleteMultipartReq>,
) -> ApiResult<Json<serde_json::Value>> {
    let parts = req.parts.into_iter().map(|p| (p.part_number, p.etag)).collect();
    state.storage_service.complete_multipart_upload(&user.username, &req.upload_id, parts).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn rename(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<DocsRenameReq>,
) -> ApiResult<Json<serde_json::Value>> {
    state.storage_service.rename(&user.username, req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsDeleteQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    state.storage_service.delete(&user.username, q).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsDownloadQuery>,
) -> ApiResult<impl IntoResponse> {
    let path = q.path.as_deref().unwrap_or("/");
    let physical_path = state.storage_service.get_file_path(&user.username, path).await?;
    
    let file = tokio_fs::File::open(&physical_path).await?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    let mime = mime_guess::from_path(&physical_path).first_or_octet_stream();
    let name = physical_path.file_name().unwrap_or_default().to_string_lossy();

    Ok(Response::builder()
        .header("Content-Type", mime.as_ref())
        .header("Content-Disposition", format!("attachment; filename=\"{}\"", name))
        .body(body)
        .unwrap())
}

pub async fn upload(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsListQuery>,
    mut multipart: Multipart,
) -> ApiResult<Json<serde_json::Value>> {
    let mut parent = q.path.clone().unwrap_or_else(|| "/".to_string());

    while let Some(field) = multipart.next_field().await? {
        let field_name = field.name().map(|n| n.to_string());
        let file_name = field.file_name().map(|n| n.to_string());

        match (field_name.as_deref(), file_name) {
            (Some("path"), None) => {
                let data = field.bytes().await?;
                if let Ok(p) = String::from_utf8(data.to_vec()) {
                    parent = p;
                }
            }
            (Some("size"), None) => {
                let _ = field.bytes().await?;
            }
            (_, Some(name)) => {
                let data = field.bytes().await?;
                match state.storage_service.save_file(&user.username, &parent, &name, data).await {
                    Ok(_) => {},
                    Err(e) => {
                        error!(user = %user.username, path = %parent, filename = %name, "Error processing uploaded file: {:?}", e);
                        return Err(e.into());
                    }
                }
            }
            _ => {
                let _ = field.bytes().await?;
            }
        }
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

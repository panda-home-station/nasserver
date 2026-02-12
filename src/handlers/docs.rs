use axum::{
    extract::{Multipart, Query, State, Extension},
    response::{IntoResponse, Response},
    Json,
};
use axum::body::Body;
use tokio_util::io::ReaderStream;
use tokio::fs::{self as tokio_fs};
use crate::state::AppState;
use crate::models::auth::AuthUser;
use crate::models::docs::{DocsListQuery, DocsListResp, DocsMkdirReq, DocsRenameReq, DocsDownloadQuery, DocsDeleteQuery};
use crate::core::Result;

pub async fn list(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsListQuery>,
) -> Result<Json<DocsListResp>> {
    let resp = state.storage_service.list(&user.username, q).await?;
    Ok(Json(resp))
}

pub async fn mkdir(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<DocsMkdirReq>,
) -> Result<Json<serde_json::Value>> {
    state.storage_service.mkdir(&user.username, req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn rename(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<DocsRenameReq>,
) -> Result<Json<serde_json::Value>> {
    state.storage_service.rename(&user.username, req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn delete(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsDeleteQuery>,
) -> Result<Json<serde_json::Value>> {
    state.storage_service.delete(&user.username, q).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DocsDownloadQuery>,
) -> Result<impl IntoResponse> {
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
) -> Result<Json<serde_json::Value>> {
    let parent = q.path.as_deref().unwrap_or("/");

    while let Some(field) = multipart.next_field().await? {
        let name = field.file_name().unwrap_or("unnamed").to_string();
        let data = field.bytes().await?;
        
        state.storage_service.save_file(&user.username, parent, &name, data).await?;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

use axum::{
    extract::{Path as AxumPath, State, Extension},
    response::{IntoResponse, Json},
    http::StatusCode,
};
use crate::state::AppState;
use models::downloader::{CreateDownloadReq, ControlDownloadReq, ResolveMagnetReq, StartMagnetDownloadReq, ResolveMagnetResp};
use models::auth::AuthUser;
use common::core::Result;

pub async fn list_downloads(State(state): State<AppState>) -> Result<Json<Vec<downloader::DownloadTaskResp>>> {
    let tasks = state.downloader_service.list_downloads().await?;
    Ok(Json(tasks))
}

pub async fn create_download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<CreateDownloadReq>
) -> Result<impl IntoResponse> {
    state.downloader_service.create_download(&user.username, payload).await?;
    Ok((StatusCode::CREATED, "Download started"))
}

pub async fn resolve_magnet(
    State(state): State<AppState>,
    Json(payload): Json<ResolveMagnetReq>
) -> Result<Json<ResolveMagnetResp>> {
    let resp = state.downloader_service.resolve_magnet(payload).await?;
    Ok(Json(resp))
}

pub async fn start_magnet_download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<StartMagnetDownloadReq>
) -> Result<impl IntoResponse> {
    state.downloader_service.start_magnet_download(&user.username, payload).await?;
    Ok((StatusCode::OK, "Download started"))
}

pub async fn control_download(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(payload): Json<ControlDownloadReq>
) -> Result<StatusCode> {
    match payload.action.as_str() {
        "pause" => state.downloader_service.pause_download(&id).await?,
        "resume" => state.downloader_service.resume_download(&id).await?,
        "delete" => state.downloader_service.delete_download(&id).await?,
        _ => return Err(common::core::AppError::BadRequest("Invalid action".to_string())),
    }
    Ok(StatusCode::OK)
}

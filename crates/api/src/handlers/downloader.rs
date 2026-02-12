use axum::{
    extract::{Path as AxumPath, State, Extension},
    response::{IntoResponse, Json},
    http::StatusCode,
};
use infra::AppState;
use domain::downloader::{
    CreateDownloadReq, ControlDownloadReq, ResolveMagnetReq, StartMagnetDownloadReq, 
    ResolveMagnetResp, DownloadTaskResp
};
use domain::entities::auth::AuthUser;
use crate::error::ApiResult;

pub async fn list_downloads(State(state): State<AppState>) -> ApiResult<Json<Vec<DownloadTaskResp>>> {
    let tasks = state.downloader_service.list_tasks().await?;
    Ok(Json(tasks))
}

pub async fn create_download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<CreateDownloadReq>
) -> ApiResult<impl IntoResponse> {
    state.downloader_service.create_task(&user.username, payload).await?;
    Ok((StatusCode::CREATED, "Download started"))
}

pub async fn resolve_magnet(
    State(state): State<AppState>,
    Json(payload): Json<ResolveMagnetReq>
) -> ApiResult<Json<ResolveMagnetResp>> {
    let resp = state.downloader_service.resolve_magnet(payload).await?;
    Ok(Json(resp))
}

pub async fn start_magnet_download(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<StartMagnetDownloadReq>
) -> ApiResult<impl IntoResponse> {
    state.downloader_service.start_magnet_download(&user.username, payload).await?;
    Ok((StatusCode::OK, "Download started"))
}

pub async fn control_download(
    State(state): State<AppState>,
    AxumPath(id): AxumPath<String>,
    Json(payload): Json<ControlDownloadReq>
) -> ApiResult<StatusCode> {
    state.downloader_service.control_task(&id, payload).await?;
    Ok(StatusCode::OK)
}

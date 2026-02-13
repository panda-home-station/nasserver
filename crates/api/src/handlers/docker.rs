use axum::{
    extract::{State, Extension},
    response::IntoResponse,
    Json,
};
use infra::AppState;
use domain::auth::AuthUser;
use domain::dtos::container::{IdReq, CreateContainerReq, PullImageReq};
use crate::error::ApiResult;

pub async fn list_gpus(State(st): State<AppState>) -> impl IntoResponse {
    let gpus = st.system_service.get_gpus().await;
    Json(gpus).into_response()
}

pub async fn list_containers(State(st): State<AppState>) -> ApiResult<impl IntoResponse> {
    let items = st.container_service.list_containers().await?;
    Ok(Json(items))
}

pub async fn list_images(State(st): State<AppState>) -> ApiResult<impl IntoResponse> {
    let items = st.container_service.list_images().await?;
    Ok(Json(items))
}

pub async fn list_volumes(State(st): State<AppState>) -> ApiResult<impl IntoResponse> {
    let items = st.container_service.list_volumes().await?;
    Ok(Json(items))
}

pub async fn list_networks(State(st): State<AppState>) -> ApiResult<impl IntoResponse> {
    let items = st.container_service.list_networks().await?;
    Ok(Json(items))
}

pub async fn start_container(State(st): State<AppState>, Json(req): Json<IdReq>) -> ApiResult<impl IntoResponse> {
    st.container_service.start_container(&req.id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn stop_container(State(st): State<AppState>, Json(req): Json<IdReq>) -> ApiResult<impl IntoResponse> {
    st.container_service.stop_container(&req.id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn restart_container(State(st): State<AppState>, Json(req): Json<IdReq>) -> ApiResult<impl IntoResponse> {
    st.container_service.restart_container(&req.id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn remove_container(State(st): State<AppState>, Json(req): Json<IdReq>) -> ApiResult<impl IntoResponse> {
    st.container_service.remove_container(&req.id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn remove_image(State(st): State<AppState>, Json(req): Json<IdReq>) -> ApiResult<impl IntoResponse> {
    st.container_service.remove_image(&req.id).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn create_container(State(st): State<AppState>, Extension(user): Extension<AuthUser>, Json(mut req): Json<CreateContainerReq>) -> ApiResult<impl IntoResponse> {
    req.username = Some(user.username);
    st.container_service.create_container(req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn pull_image(State(st): State<AppState>, Json(req): Json<PullImageReq>) -> ApiResult<impl IntoResponse> {
    st.container_service.pull_image(req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

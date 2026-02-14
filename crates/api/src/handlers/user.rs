use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use infra::AppState;
use domain::auth::{AuthUser, SecuritySettings};
use axum::Extension;
use crate::error::ApiResult;

#[derive(Deserialize)]
pub struct SetWallpaperReq {
    pub path: Option<String>,
}

pub async fn get_wallpaper(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> ApiResult<impl IntoResponse> {
    let wallpaper = state.auth_service.get_wallpaper(&user.user_id.to_string()).await?;
    Ok(Json(serde_json::json!({ "path": wallpaper.unwrap_or_default() })))
}

pub async fn set_wallpaper(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<SetWallpaperReq>,
) -> ApiResult<impl IntoResponse> {
    let path = payload.path.unwrap_or_default();
    state.auth_service.set_wallpaper(&user.user_id.to_string(), &path).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_security_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult<impl IntoResponse> {
    let settings = state.auth_service.get_security_settings(&user.user_id.to_string()).await?;
    Ok(Json(settings))
}

pub async fn set_security_settings(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<SecuritySettings>,
) -> ApiResult<impl IntoResponse> {
    state.auth_service.set_security_settings(&user.user_id.to_string(), payload).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

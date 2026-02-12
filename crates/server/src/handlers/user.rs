use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use serde::Deserialize;

use crate::state::AppState;
use models::auth::AuthUser;
use axum::Extension;
use common::core::Result;

#[derive(Deserialize)]
pub struct SetWallpaperReq {
    pub path: Option<String>,
}

pub async fn get_wallpaper(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> Result<impl IntoResponse> {
    let wallpaper = state.auth_service.get_wallpaper(&user.user_id.to_string()).await?;
    Ok(Json(serde_json::json!({ "path": wallpaper.unwrap_or_default() })))
}

pub async fn set_wallpaper(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<SetWallpaperReq>,
) -> Result<impl IntoResponse> {
    let path = payload.path.unwrap_or_default();
    state.auth_service.set_wallpaper(&user.user_id.to_string(), &path).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

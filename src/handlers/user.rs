use axum::{
    extract::State,
    response::{IntoResponse, Json},
    http::StatusCode,
};
use serde::Deserialize;
use sqlx::Row;

use crate::state::AppState;
use crate::models::auth::AuthUser;
use axum::Extension;

#[derive(Deserialize)]
pub struct SetWallpaperReq {
    pub path: Option<String>,
}

pub async fn get_wallpaper(State(state): State<AppState>, Extension(user): Extension<AuthUser>) -> impl IntoResponse {
    let rec = sqlx::query("select wallpaper from users where id = $1")
        .bind(user.user_id.to_string())
        .fetch_optional(&state.db)
        .await;
    match rec {
        Ok(Some(row)) => {
            let path: Option<String> = row.try_get("wallpaper").ok();
            Json(serde_json::json!({ "path": path.unwrap_or_default() })).into_response()
        }
        Ok(None) => Json(serde_json::json!({ "path": "" })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn set_wallpaper(
    State(state): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(payload): Json<SetWallpaperReq>,
) -> impl IntoResponse {
    let path = payload.path.unwrap_or_default();
    let res = sqlx::query("update users set wallpaper = $1 where id = $2")
        .bind(path)
        .bind(user.user_id.to_string())
        .execute(&state.db)
        .await;
    match res {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

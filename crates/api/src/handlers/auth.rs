use axum::{
    extract::{Extension, State},
    Json,
};
use infra::AppState;
use models::auth::{SignupReq, SignupResp, LoginReq, LoginResp, AuthUser};
use common::core::Result;

pub async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> Result<Json<SignupResp>> {
    let resp = st.auth_service.signup(req).await?;
    Ok(Json(resp))
}

pub async fn auth_signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> Result<Json<SignupResp>> {
    let resp = st.auth_service.signup(req).await?;
    Ok(Json(resp))
}

pub async fn auth_login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Result<Json<LoginResp>> {
    let resp = st.auth_service.login(req).await?;
    Ok(Json(resp))
}

pub async fn whoami(State(st): State<AppState>, Extension(user): Extension<AuthUser>) -> Result<Json<serde_json::Value>> {
    let user_info = st.auth_service.get_user_by_id(&user.user_id.to_string()).await?;
    Ok(Json(user_info))
}

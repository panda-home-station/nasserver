use axum::{
    extract::{Extension, State},
    Json,
};
use infra::AppState;
use domain::auth::{SignupReq, SignupResp, LoginReq, LoginResp, AuthUser};
use crate::error::ApiResult;

pub async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> ApiResult<Json<SignupResp>> {
    let resp = st.auth_service.signup(req).await?;
    Ok(Json(resp))
}

pub async fn auth_signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> ApiResult<Json<SignupResp>> {
    let resp = st.auth_service.signup(req).await?;
    Ok(Json(resp))
}

pub async fn auth_login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> ApiResult<Json<LoginResp>> {
    let resp = st.auth_service.login(req).await?;
    Ok(Json(resp))
}

pub async fn whoami(State(st): State<AppState>, Extension(user): Extension<AuthUser>) -> ApiResult<Json<serde_json::Value>> {
    let user_info = st.auth_service.get_user_by_id(&user.user_id.to_string()).await?;
    Ok(Json(user_info))
}

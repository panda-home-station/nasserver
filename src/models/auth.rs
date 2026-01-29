use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Deserialize)]
pub struct SignupReq {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct SignupResp {
    pub user_id: String,
}

#[derive(Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResp {
    pub user_id: String,
    pub token: String,
}

#[derive(Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub name: String,
    pub exp: usize,
}

#[derive(Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub username: String,
}

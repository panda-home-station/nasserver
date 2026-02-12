use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub name: String,
    pub exp: usize,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub username: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct SignupReq {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct SignupResp {
    pub user_id: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct LoginResp {
    pub user_id: String,
    pub token: String,
}

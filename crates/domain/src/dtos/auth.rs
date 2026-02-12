use serde::{Deserialize, Serialize};

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

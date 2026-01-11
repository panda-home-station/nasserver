use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct DeviceCodeResp {
    pub code: String,
    pub expire_ts: i64,
}

#[derive(Deserialize)]
pub struct DeviceAuthReq {
    pub code: String,
    pub _device_id: String,
}

#[derive(Serialize)]
pub struct DeviceAuthResp {
    pub status: String,
}

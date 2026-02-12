use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone, Debug)]
pub struct DeviceCodeResp {
    pub code: String,
    pub expire_ts: i64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DeviceAuthReq {
    pub code: String,
    pub _device_id: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DeviceAuthResp {
    pub status: String,
}

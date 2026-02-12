use axum::{
    extract::State,
    response::IntoResponse,
    Json,
};
use chrono::{Duration, Utc};

use infra::state::{AppState, DEVICE_CODES};
use domain::device::{DeviceCodeResp, DeviceAuthReq, DeviceAuthResp};

pub async fn device_code(State(_st): State<AppState>) -> impl IntoResponse {
    let mut codes = DEVICE_CODES.lock().unwrap();
    let code = format!("{:08}", fastrand::u32(0..100_000_000));
    let expire = Utc::now() + Duration::minutes(10);
    codes.insert(code.clone(), expire.timestamp());
    Json(DeviceCodeResp {
        code,
        expire_ts: expire.timestamp(),
    })
}

pub async fn device_authorize(State(_st): State<AppState>, Json(req): Json<DeviceAuthReq>) -> impl IntoResponse {
    let mut codes = DEVICE_CODES.lock().unwrap();
    match codes.get(&req.code) {
        Some(ts) if *ts > Utc::now().timestamp() => {
            codes.remove(&req.code);
            Json(DeviceAuthResp { status: "bound".to_string() })
        }
        _ => Json(DeviceAuthResp { status: "expired".to_string() }),
    }
}

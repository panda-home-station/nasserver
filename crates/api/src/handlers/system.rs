use axum::{
    extract::{State, Query},
    Json,
};
use infra::AppState;
use models::system::{InitReq, DeviceInfoResp, PortCheckReq, PortCheckResp, SystemStats, StatsHistoryQuery, InitStateResp};
use common::core::Result;

pub async fn get_current_stats(State(st): State<AppState>) -> Result<Json<SystemStats>> {
    let stats = st.system_service.get_current_stats().await?;
    Ok(Json(stats))
}

pub async fn get_stats_history(
    State(st): State<AppState>,
    Query(query): Query<StatsHistoryQuery>,
) -> Result<Json<Vec<SystemStats>>> {
    let stats = st.system_service.get_stats_history(query).await?;
    Ok(Json(stats))
}

pub async fn health(State(st): State<AppState>) -> Result<Json<serde_json::Value>> {
    let h = st.system_service.health().await?;
    Ok(Json(h))
}

pub async fn version() -> Json<models::system::VersionResp> {
    Json(models::system::VersionResp {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

pub async fn init_state(State(st): State<AppState>) -> Result<Json<InitStateResp>> {
    let initialized = st.system_service.is_initialized().await?;
    Ok(Json(InitStateResp { initialized }))
}

pub async fn init_system(State(st): State<AppState>, Json(req): Json<InitReq>) -> Result<Json<serde_json::Value>> {
    st.system_service.init_system(req).await?;
    Ok(Json(serde_json::json!({ "ok": true })))
}

pub async fn get_device_info(State(st): State<AppState>) -> Result<Json<DeviceInfoResp>> {
    let info = st.system_service.get_device_info().await?;
    Ok(Json(info))
}

pub async fn check_ports(State(st): State<AppState>, Json(req): Json<PortCheckReq>) -> Result<Json<PortCheckResp>> {
    let results = st.system_service.check_ports(req.ports).await?;
    Ok(Json(PortCheckResp { results }))
}

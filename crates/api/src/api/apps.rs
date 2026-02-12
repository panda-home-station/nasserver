use axum::{
    extract::{State, Path},
    Json,
};
use infra::AppState;
use common::core::Result;
use models::domain::app::App;

pub async fn list_apps(State(st): State<AppState>) -> Result<Json<Vec<App>>> {
    let apps = st.app_manager.list_apps().await?;
    Ok(Json(apps))
}

pub async fn install_app(
    State(st): State<AppState>,
    Json(app_config): Json<App>,
) -> Result<Json<()>> {
    st.app_manager.install_app(app_config).await?;
    Ok(Json(()))
}

pub async fn stop_app(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<()>> {
    st.app_manager.stop_app(&id).await?;
    Ok(Json(()))
}

pub async fn start_app(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<()>> {
    st.app_manager.start_app(&id).await?;
    Ok(Json(()))
}

pub async fn uninstall_app(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<()>> {
    st.app_manager.uninstall_app(&id).await?;
    Ok(Json(()))
}

use axum::{
    middleware,
    routing::{delete, get, post, get_service},
    Router,
    http::{Method, StatusCode},
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use axum::extract::DefaultBodyLimit;

use crate::state::AppState;
use crate::handlers::{auth, system, device, docker, docs};
use crate::middleware::require_auth;

pub fn api_app(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::PUT, Method::OPTIONS])
        .allow_origin(Any)
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            axum::http::header::ACCEPT,
            axum::http::header::ORIGIN,
        ]);

    let protected = Router::new()
        .route("/api/auth/whoami", get(auth::whoami))
        .route("/api/docs/list", get(docs::list))
        .route("/api/docs/mkdir", post(docs::mkdir))
        .route("/api/docs/delete", delete(docs::delete))
        .route("/api/docs/rename", post(docs::rename))
        .route("/api/docs/download", get(docs::download))
        .route("/api/docs/upload", post(docs::upload))
        // User preferences
        .route("/api/user/wallpaper", get(crate::handlers::user::get_wallpaper))
        .route("/api/user/wallpaper", post(crate::handlers::user::set_wallpaper))
        // Podman management (via Docker-compatible client; supports Podman socket)
        .route("/api/podman/containers", get(docker::list_containers))
        .route("/api/podman/images", get(docker::list_images))
        .route("/api/podman/volumes", get(docker::list_volumes))
        .route("/api/podman/networks", get(docker::list_networks))
        .route("/api/podman/gpus", get(docker::list_gpus))
        .route("/api/podman/container/start", post(docker::start_container))
        .route("/api/podman/container/stop", post(docker::stop_container))
        .route("/api/podman/container/restart", post(docker::restart_container))
        .route("/api/podman/container/remove", post(docker::remove_container))
        .route("/api/podman/container/create", post(docker::create_container))
        .route("/api/podman/image/pull", post(docker::pull_image))
        .route("/api/podman/image/remove", post(docker::remove_image))
        // Registry
        .route("/api/podman/registry/search", get(crate::handlers::docker_registry::search))
        .route("/api/podman/registry/hot", get(crate::handlers::docker_registry::hot))
        // Registry settings
        .route("/api/podman/mirrors", get(crate::handlers::docker_registry::get_mirrors))
        .route("/api/podman/mirrors", post(crate::handlers::docker_registry::set_mirrors))
        .route("/api/tasks", get(crate::handlers::task::list_tasks))
        .route("/api/tasks", post(crate::handlers::task::create_task))
        .route("/api/tasks/:id", post(crate::handlers::task::update_task))
        .route("/api/tasks/clear", post(crate::handlers::task::clear_completed_tasks))
        .route("/api/downloads", get(crate::handlers::downloader::list_downloads))
        .route("/api/downloads", post(crate::handlers::downloader::create_download))
        .route("/api/downloads/:id/control", post(crate::handlers::downloader::control_download))
        .route("/api/downloads/magnet/resolve", post(crate::handlers::downloader::resolve_magnet))
        .route("/api/downloads/magnet/start", post(crate::handlers::downloader::start_magnet_download))
        .route("/api/agent/chat", post(crate::handlers::agent::chat))
        .route("/api/agent/tasks", post(crate::handlers::agent::create_task))
        .route("/api/agent/tasks/:id", get(crate::handlers::agent::get_task))
        .route("/api/system/stats", get(system::get_current_stats))
        .route("/api/system/stats/history", get(system::get_stats_history))
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    Router::new()
        .route("/health", get(system::health))
        .route("/version", get(system::version))
        .route("/api/system/init/state", get(system::init_state))
        .route("/api/system/init", post(system::init_system))
        .route("/api/system/device", get(system::get_device_info))
        .route("/api/system/check_ports", post(system::check_ports))
        // Note: original code had /api/cloud/signup pointing to signup and /api/auth/signup pointing to auth_signup
        // but both called the same logic. Here we keep it consistent.
        // Assuming `signup` was for cloud registration and `auth_signup` for local user creation.
        // In the handler implementation, both do the same thing (create user).
        .route("/api/cloud/signup", post(auth::signup)) 
        .route("/api/auth/signup", post(auth::auth_signup))
        .route("/api/auth/login", post(auth::auth_login))
        .route("/api/cloud/device/code", post(device::device_code))
        .route("/api/cloud/device/authorize", post(device::device_authorize))
        .with_state(state)
        .merge(protected)
        .layer(cors)
        .layer({
            let max_mb = std::env::var("PNAS_MAX_UPLOAD_MB")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(10240);
            DefaultBodyLimit::max(max_mb * 1024 * 1024)
        })
}

pub fn static_app() -> Router {
    let static_dir = std::env::var("STATIC_DIR").unwrap_or_else(|_| "/srv/www".to_string());
    let static_service = get_service(
        ServeDir::new(&static_dir).fallback(ServeFile::new(format!("{}/index.html", static_dir))),
    )
    .handle_error(|err| async move {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("static serve error: {}", err),
        )
    });

    Router::new().fallback_service(static_service)
}

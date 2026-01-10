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
use crate::handlers::{auth, system, device, fs, docker};
use crate::middleware::require_auth;

pub fn app(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_origin(Any)
        .allow_headers(Any);

    let protected = Router::new()
        .route("/api/auth/whoami", get(auth::whoami))
        .route("/api/fs/list", get(fs::fs_list))
        .route("/api/fs/mkdir", post(fs::fs_mkdir))
        .route("/api/fs/delete", delete(fs::fs_delete))
        .route("/api/fs/rename", post(fs::fs_rename))
        .route("/api/fs/download", get(fs::fs_download))
        .route("/api/fs/upload", post(fs::fs_upload))
        // Docker management
        .route("/api/docker/containers", get(docker::list_containers))
        .route("/api/docker/images", get(docker::list_images))
        .route("/api/docker/container/start", post(docker::start_container))
        .route("/api/docker/container/stop", post(docker::stop_container))
        .route("/api/docker/container/restart", post(docker::restart_container))
        .route("/api/docker/container/remove", post(docker::remove_container))
        .route("/api/docker/image/pull", post(docker::pull_image))
        .with_state(state.clone())
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

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

    Router::new()
        .route("/health", get(system::health))
        .route("/version", get(system::version))
        .route("/api/system/init/state", get(system::init_state))
        .route("/api/system/init", post(system::init_system))
        .route("/api/system/device", get(system::get_device_info))
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
        .fallback_service(static_service)
        .layer(cors)
        .layer({
            let max_mb = std::env::var("PNAS_MAX_UPLOAD_MB")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(10240);
            DefaultBodyLimit::max(max_mb * 1024 * 1024)
        })
}

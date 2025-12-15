use axum::{
    extract::State,
    http::Method,
    response::IntoResponse,
    routing::{get, post, delete},
    Json, Router,
};
use chrono::{Duration, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Mutex};
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    device_codes: &'static Lazy<Mutex<HashMap<String, i64>>>,
    users: &'static Lazy<Mutex<HashMap<String, String>>>,
}

static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static USERS: Lazy<Mutex<HashMap<String, String>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Deserialize)]
struct SignupReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct SignupResp {
    user_id: String,
}

#[derive(Serialize)]
struct DeviceCodeResp {
    code: String,
    expire_ts: i64,
}

#[derive(Deserialize)]
struct DeviceAuthReq {
    code: String,
    device_id: String,
}

#[derive(Serialize)]
struct DeviceAuthResp {
    status: String,
}

#[derive(Serialize)]
struct HealthResp {
    status: String,
    ts: i64,
}

#[derive(Serialize)]
struct VersionResp {
    version: String,
}

#[derive(Deserialize)]
struct FsListQuery {
    path: Option<String>,
}

#[derive(Deserialize)]
struct FsDeleteQuery {
    path: String,
}

#[derive(Deserialize)]
struct FsMkdirReq {
    path: String,
}

#[derive(Serialize)]
struct FsEntry {
    name: String,
    is_dir: bool,
    size: u64,
    modified_ts: i64,
}

#[derive(Serialize)]
struct FsListResp {
    base: String,
    path: String,
    entries: Vec<FsEntry>,
}

#[tokio::main]
async fn main() {
    let state = AppState {
        device_codes: &DEVICE_CODES,
        users: &USERS,
    };
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_origin(Any)
        .allow_headers(Any);
    let app = Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/api/cloud/signup", post(signup))
        .route("/api/cloud/device/code", post(device_code))
        .route("/api/cloud/device/authorize", post(device_authorize))
        .route("/api/fs/list", get(fs_list))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/fs/delete", delete(fs_delete))
        .with_state(state)
        .layer(cors);
    let addr: SocketAddr = ([0, 0, 0, 0], 8000).into();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
    let mut users = st.users.lock().unwrap();
    let id = Uuid::new_v4().to_string();
    users.insert(id.clone(), req.email);
    Json(SignupResp { user_id: id })
}

async fn device_code(State(st): State<AppState>) -> impl IntoResponse {
    let mut codes = st.device_codes.lock().unwrap();
    let code = format!("{:08}", fastrand::u32(0..100_000_000));
    let expire = Utc::now() + Duration::minutes(10);
    codes.insert(code.clone(), expire.timestamp());
    Json(DeviceCodeResp {
        code,
        expire_ts: expire.timestamp(),
    })
}

async fn device_authorize(State(st): State<AppState>, Json(req): Json<DeviceAuthReq>) -> impl IntoResponse {
    let mut codes = st.device_codes.lock().unwrap();
    match codes.get(&req.code) {
        Some(ts) if *ts > Utc::now().timestamp() => {
            codes.remove(&req.code);
            Json(DeviceAuthResp { status: "bound".to_string() })
        }
        _ => Json(DeviceAuthResp { status: "expired".to_string() }),
    }
}

async fn health() -> impl IntoResponse {
    Json(HealthResp {
        status: "ok".to_string(),
        ts: Utc::now().timestamp(),
    })
}

async fn version() -> impl IntoResponse {
    Json(VersionResp {
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn fs_list(axum::extract::Query(q): axum::extract::Query<FsListQuery>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    let base = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let req_path = q.path.unwrap_or_else(|| "/".to_string());

    let norm = if req_path.starts_with('/') {
        &req_path[1..]
    } else {
        req_path.as_str()
    };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let target_abs = match fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => {
            return Json(FsListResp {
                base: base_abs.display().to_string(),
                path: req_path,
                entries: vec![],
            });
        }
    };
    if !target_abs.starts_with(&base_abs) {
        return Json(FsListResp {
            base: base_abs.display().to_string(),
            path: req_path,
            entries: vec![],
        });
    }

    let mut entries: Vec<FsEntry> = Vec::new();
    if let Ok(read_dir) = fs::read_dir(&target_abs) {
        for ent in read_dir.flatten() {
            let name = ent.file_name().to_string_lossy().to_string();
            let md = match ent.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let is_dir = md.is_dir();
            let size = if is_dir { 0 } else { md.len() };
            let modified_ts = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            entries.push(FsEntry {
                name,
                is_dir,
                size,
                modified_ts,
            });
        }
    }
    entries.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return b.is_dir.cmp(&a.is_dir);
        }
        a.name.to_lowercase().cmp(&b.name.to_lowercase())
    });
    Json(FsListResp {
        base: base_abs.display().to_string(),
        path: req_path,
        entries,
    })
}

async fn fs_mkdir(Json(req): Json<FsMkdirReq>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};

    let base = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let req_path = req.path;
    let norm = if req_path.starts_with('/') { &req_path[1..] } else { req_path.as_str() };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let parent = joined.parent().unwrap_or(Path::new(&base)).to_path_buf();
    let target_abs = match fs::canonicalize(&parent) {
        Ok(p) => p.join(joined.file_name().unwrap_or_default()),
        Err(_) => joined,
    };
    if !target_abs.starts_with(&base_abs) {
        return Json(serde_json::json!({ "ok": false }));
    }
    let _ = fs::create_dir_all(&target_abs);
    Json(serde_json::json!({ "ok": true }))
}

async fn fs_delete(axum::extract::Query(q): axum::extract::Query<FsDeleteQuery>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};

    let base = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let req_path = q.path;
    let norm = if req_path.starts_with('/') { &req_path[1..] } else { req_path.as_str() };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let target_abs = match fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => joined.clone(),
    };
    if !target_abs.starts_with(&base_abs) {
        return Json(serde_json::json!({ "ok": false }));
    }
    let md = fs::metadata(&target_abs);
    if let Ok(m) = md {
        if m.is_dir() {
            let _ = fs::remove_dir_all(&target_abs);
        } else {
            let _ = fs::remove_file(&target_abs);
        }
        return Json(serde_json::json!({ "ok": true }));
    }
    Json(serde_json::json!({ "ok": false }))
}

use axum::{
    extract::{Extension, Request, State, Multipart},
    http::{Method, StatusCode, HeaderMap, HeaderValue},
    middleware::{self, Next},
    response::IntoResponse,
    routing::{delete, get, post, get_service},
    Json, Router,
};
use chrono::{Duration, Utc};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Mutex};
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use uuid::Uuid;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use sqlx::{postgres::PgPoolOptions, Pool, Postgres};
use dotenvy::dotenv;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use password_hash::SaltString;
use axum::extract::DefaultBodyLimit;

#[derive(Clone)]
struct AppState {
    device_codes: &'static Lazy<Mutex<HashMap<String, i64>>>,
    db: Pool<Postgres>,
    jwt_secret: String,
}

static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static START_TIME: Lazy<chrono::DateTime<Utc>> = Lazy::new(|| Utc::now());

#[derive(Deserialize)]
struct SignupReq {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SignupResp {
    user_id: String,
}

#[derive(Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct LoginResp {
    user_id: String,
    token: String,
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

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
}

#[derive(Clone)]
struct AuthUser {
    user_id: Uuid,
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
struct FsRenameReq {
    from: String,
    to: String,
}

#[derive(Deserialize)]
struct FsDownloadQuery {
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
    dotenv().ok();
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://postgres:postgres@postgres:5432/pnas".to_string());
    // Use lazy connection to avoid crashing when DB is unavailable in dev/test
    let db = PgPoolOptions::new().max_connections(5).connect_lazy(&db_url).unwrap();
    let _ = sqlx::query(
        "create table if not exists users (
            id uuid primary key,
            username text unique,
            email text,
            password_hash text not null,
            created_at timestamptz default now()
        )",
    )
    .execute(&db)
    .await;
    let _ = sqlx::query("alter table users add column if not exists role text not null default 'user'")
        .execute(&db)
        .await;
    let _ = sqlx::query("alter table users add column if not exists username text unique")
        .execute(&db)
        .await;
    let _ = sqlx::query(
        "create table if not exists system_config (
            key text primary key,
            value text
        )",
    )
    .execute(&db)
    .await;
    let jwt_secret = std::env::var("JWT_SECRET").unwrap_or_else(|_| "dev-secret".to_string());
    let state = AppState {
        device_codes: &DEVICE_CODES,
        db,
        jwt_secret,
    };
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_origin(Any)
        .allow_headers(Any);
    let protected = Router::new()
        .route("/api/auth/whoami", get(whoami))
        .route("/api/fs/list", get(fs_list))
        .route("/api/fs/mkdir", post(fs_mkdir))
        .route("/api/fs/delete", delete(fs_delete))
        .route("/api/fs/rename", post(fs_rename))
        .route("/api/fs/download", get(fs_download))
        .route("/api/fs/upload", post(fs_upload))
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
    let app = Router::new()
        .route("/health", get(health))
        .route("/version", get(version))
        .route("/api/system/init/state", get(init_state))
        .route("/api/system/init", post(init_system))
        .route("/api/system/device", get(get_device_info))
        .route("/api/cloud/signup", post(signup))
        .route("/api/auth/signup", post(auth_signup))
        .route("/api/auth/login", post(auth_login))
        .route("/api/cloud/device/code", post(device_code))
        .route("/api/cloud/device/authorize", post(device_authorize))
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
        });
    let port: u16 = std::env::var("PNAS_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(8000);
    println!("Backend listening on port {}", port);
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
    let existing = sqlx::query_scalar::<_, Uuid>("select id from users where username = $1")
        .bind(&req.username)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    let id = if let Some(uid) = existing {
        uid
    } else {
        let salt = SaltString::generate(&mut rand_core::OsRng);
        let argon2 = Argon2::default();
        let hash = argon2.hash_password(req.password.as_bytes(), &salt).unwrap().to_string();
        let uid = Uuid::new_v4();
        let _ = sqlx::query("insert into users (id, username, password_hash) values ($1, $2, $3)")
            .bind(uid)
            .bind(&req.username)
            .bind(&hash)
            .execute(&st.db)
            .await;
        // create per-user storage root
        let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| "/srv/nas".to_string());
        let user_root = std::path::Path::new(&base_root).join("users").join(uid.to_string());
        let _ = std::fs::create_dir_all(&user_root);
        uid
    };
    Json(SignupResp { user_id: id.to_string() })
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

#[derive(Serialize)]
struct InitStateResp {
    initialized: bool,
}

#[derive(Deserialize)]
struct InitReq {
    username: String,
    password: String,
    device_name: String,
}

async fn init_state(State(st): State<AppState>) -> impl IntoResponse {
    let cnt: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);
    Json(InitStateResp { initialized: cnt > 0 })
}

async fn init_system(State(st): State<AppState>, Json(req): Json<InitReq>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let cnt: i64 = sqlx::query_scalar("select count(*) from users")
        .fetch_one(&st.db)
        .await
        .unwrap_or(0);
    if cnt > 0 {
        return Err((StatusCode::CONFLICT, Json(serde_json::json!({ "error": "already_initialized" }))));
    }
    let salt = SaltString::generate(&mut rand_core::OsRng);
    let argon2 = Argon2::default();
    let hash = argon2.hash_password(req.password.as_bytes(), &salt).unwrap().to_string();
    let uid = Uuid::new_v4();
    let _ = sqlx::query("insert into users (id, username, password_hash, role) values ($1, $2, $3, 'admin')")
        .bind(uid)
        .bind(&req.username)
        .bind(&hash)
        .execute(&st.db)
        .await;

    // Generate device ID (40 chars hex)
    let mut bytes = [0u8; 20];
    for i in 0..20 {
        bytes[i] = fastrand::u8(..);
    }
    let device_id = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();

    let _ = sqlx::query("insert into system_config (key, value) values ('device_name', $1) on conflict (key) do update set value = $1")
        .bind(&req.device_name)
        .execute(&st.db)
        .await;
    let _ = sqlx::query("insert into system_config (key, value) values ('device_id', $1) on conflict (key) do update set value = $1")
        .bind(&device_id)
        .execute(&st.db)
        .await;

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| "/srv/nas".to_string());
    let user_root = std::path::Path::new(&base_root).join("users").join(uid.to_string());
    let _ = std::fs::create_dir_all(&user_root);
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[derive(Serialize)]
struct DeviceInfoResp {
    device_name: String,
    device_id: String,
    system_version: String,
    system_time: String,
    uptime: String,
}

async fn get_device_info(State(st): State<AppState>) -> impl IntoResponse {
    let name: String = sqlx::query_scalar("select value from system_config where key = 'device_name'")
        .fetch_optional(&st.db)
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "PNAS-Server".to_string());
    let id: String = sqlx::query_scalar("select value from system_config where key = 'device_id'")
        .fetch_optional(&st.db)
        .await
        .unwrap_or(None)
        .unwrap_or_else(|| "Unknown".to_string());
    
    let now = Utc::now();
    let uptime_duration = now.signed_duration_since(*START_TIME);
    let days = uptime_duration.num_days();
    let hours = uptime_duration.num_hours() % 24;
    let minutes = uptime_duration.num_minutes() % 60;
    
    let uptime_str = format!("{}天 {}小时 {}分", days, hours, minutes);
    let time_str = now.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string();

    Json(DeviceInfoResp {
        device_name: name,
        device_id: id,
        system_version: format!("PNAS Lite {}", env!("CARGO_PKG_VERSION")),
        system_time: time_str,
        uptime: uptime_str,
    })
}

async fn auth_signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
    let existing = sqlx::query_scalar::<_, Uuid>("select id from users where username = $1")
        .bind(&req.username)
        .fetch_optional(&st.db)
        .await
        .ok()
        .flatten();
    let id = if let Some(uid) = existing {
        uid
    } else {
        let salt = SaltString::generate(&mut rand_core::OsRng);
        let argon2 = Argon2::default();
        let hash = argon2.hash_password(req.password.as_bytes(), &salt).unwrap().to_string();
        let uid = Uuid::new_v4();
        let _ = sqlx::query("insert into users (id, username, password_hash) values ($1, $2, $3)")
            .bind(uid)
            .bind(&req.username)
            .bind(&hash)
            .execute(&st.db)
            .await;
        // create per-user storage root
        let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| "/srv/nas".to_string());
        let user_root = std::path::Path::new(&base_root).join("users").join(uid.to_string());
        let _ = std::fs::create_dir_all(&user_root);
        uid
    };
    Json(SignupResp { user_id: id.to_string() })
}

async fn auth_login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Result<Json<LoginResp>, (StatusCode, Json<serde_json::Value>)> {
    let row = sqlx::query_as::<_, (Uuid, String, String)>("select id, password_hash, username from users where username = $1")
        .bind(&req.username)
        .fetch_optional(&st.db)
        .await
        .unwrap();
    if let Some((uid, pwd_hash, _username)) = row {
        let parsed = PasswordHash::new(&pwd_hash).unwrap();
        let ok = Argon2::default()
            .verify_password(req.password.as_bytes(), &parsed)
            .is_ok();
        if ok {
            let exp = (Utc::now() + Duration::days(7)).timestamp() as usize;
            let claims = Claims {
                sub: uid.to_string(),
                exp,
            };
            let token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(st.jwt_secret.as_bytes()),
            )
            .unwrap();
            return Ok(Json(LoginResp {
                user_id: uid.to_string(),
                token,
            }));
        }
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({ "error": "invalid_credentials" })),
    ))
}

async fn require_auth(State(st): State<AppState>, mut req: Request, next: Next) -> Result<axum::response::Response, StatusCode> {
    let hdr = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let bearer = hdr.strip_prefix("Bearer ").unwrap_or("");
    let mut token = bearer.to_string();
    if token.is_empty() {
        if let Some(q) = req.uri().query() {
            for pair in q.split('&') {
                let mut kv = pair.splitn(2, '=');
                if kv.next() == Some("token") {
                    token = kv.next().unwrap_or("").to_string();
                    break;
                }
            }
        }
    }
    if token.is_empty() {
        println!("Auth failed: missing token for {}", req.uri().path());
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Dev-only bypass: allow raw UUID tokens when ALLOW_INSECURE_AUTH=1
    if std::env::var("ALLOW_INSECURE_AUTH").ok().as_deref() == Some("1") {
        if let Ok(uid) = Uuid::parse_str(token.as_str()) {
            req.extensions_mut().insert(AuthUser { user_id: uid });
            return Ok(next.run(req).await);
        }
    }
    let data = decode::<Claims>(
        token.as_str(),
        &DecodingKey::from_secret(st.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let uid = Uuid::parse_str(&data.claims.sub).map_err(|_| StatusCode::UNAUTHORIZED)?;
    req.extensions_mut().insert(AuthUser { user_id: uid });
    Ok(next.run(req).await)
}

async fn whoami(State(st): State<AppState>, Extension(user): Extension<AuthUser>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let rec = sqlx::query_scalar::<_, String>("select username from users where id = $1")
        .bind(user.user_id)
        .fetch_optional(&st.db)
        .await
        .unwrap();
    if let Some(username) = rec {
        Ok(Json(serde_json::json!({ "user_id": user.user_id.to_string(), "username": username })))
    } else {
        Err((StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not_found" }))))
    }
}

async fn fs_list(Extension(user): Extension<AuthUser>, axum::extract::Query(q): axum::extract::Query<FsListQuery>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
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

async fn fs_mkdir(Extension(user): Extension<AuthUser>, Json(req): Json<FsMkdirReq>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
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

async fn fs_delete(Extension(user): Extension<AuthUser>, axum::extract::Query(q): axum::extract::Query<FsDeleteQuery>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
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

async fn fs_rename(Extension(user): Extension<AuthUser>, Json(req): Json<FsRenameReq>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);

    let norm_from = if req.from.starts_with('/') { &req.from[1..] } else { req.from.as_str() };
    let from_joined: PathBuf = Path::new(&base).join(norm_from);
    let norm_to = if req.to.starts_with('/') { &req.to[1..] } else { req.to.as_str() };
    let to_joined: PathBuf = Path::new(&base).join(norm_to);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let from_abs = match fs::canonicalize(&from_joined) {
        Ok(p) => p,
        Err(_) => from_joined.clone(),
    };
    let to_parent = to_joined.parent().unwrap_or(Path::new(&base)).to_path_buf();
    let to_abs = match fs::canonicalize(&to_parent) {
        Ok(p) => p.join(to_joined.file_name().unwrap_or_default()),
        Err(_) => to_joined.clone(),
    };
    if !from_abs.starts_with(&base_abs) || !to_abs.starts_with(&base_abs) {
        return Json(serde_json::json!({ "ok": false }));
    }
    let _ = fs::create_dir_all(to_parent);
    let ok = fs::rename(&from_abs, &to_abs).is_ok();
    Json(serde_json::json!({ "ok": ok }))
}

async fn fs_download(Extension(user): Extension<AuthUser>, axum::extract::Query(q): axum::extract::Query<FsDownloadQuery>) -> impl IntoResponse {
    use std::fs;
    use std::path::{Path, PathBuf};

    let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
    let base = format!("{}/users/{}", base_root, user.user_id);
    let req_path = q.path;
    let norm = if req_path.starts_with('/') { &req_path[1..] } else { req_path.as_str() };
    let joined: PathBuf = Path::new(&base).join(norm);

    let base_abs = fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
    let target_abs = match fs::canonicalize(&joined) {
        Ok(p) => p,
        Err(_) => joined.clone(),
    };
    if !target_abs.starts_with(&base_abs) {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/octet-stream"));
        return (headers, Vec::<u8>::new());
    }
    let data = fs::read(&target_abs).unwrap_or_default();
    let name = target_abs.file_name().and_then(|n| n.to_str()).unwrap_or("download.bin");
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/octet-stream"));
    let cd = format!("attachment; filename=\"{}\"", name);
    if let Ok(v) = HeaderValue::from_str(&cd) {
        headers.insert("content-disposition", v);
    }
    (headers, data)
}

async fn fs_upload(Extension(user): Extension<AuthUser>, mut multipart: Multipart) -> impl IntoResponse {
    use std::path::{Path, PathBuf};
    use tokio::fs;
    use tokio::io::AsyncWriteExt;

    let mut dest_path = "/".to_string();
    let mut wrote = false;
    let mut total_written: usize = 0;
    let mut saved_name: Option<String> = None;

    while let Ok(Some(mut field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "path" {
            dest_path = field.text().await.unwrap_or("/".to_string());
        } else if name == "file" {
            let file_name = field.file_name().map(|s| s.to_string()).unwrap_or("upload.bin".to_string());
            let base_root = std::env::var("FS_BASE_DIR").unwrap_or_else(|_| ".".to_string());
            let base = format!("{}/users/{}", base_root, user.user_id);
            let norm = if dest_path.starts_with('/') { &dest_path[1..] } else { dest_path.as_str() };
            let dir_joined: PathBuf = Path::new(&base).join(norm);
            let base_abs = std::fs::canonicalize(&base).unwrap_or_else(|_| PathBuf::from(&base));
            let dir_abs = match std::fs::canonicalize(&dir_joined) {
                Ok(p) => p,
                Err(_) => dir_joined.clone(),
            };
            if !dir_abs.starts_with(&base_abs) {
                return Json(serde_json::json!({ "ok": false }));
            }
            let _ = std::fs::create_dir_all(&dir_abs);
            let target_abs = dir_abs.join(&file_name);
            let mut f = match fs::File::create(&target_abs).await {
                Ok(h) => h,
                Err(e) => {
                    println!("fs_upload: create file failed: {}", e);
                    return Json(serde_json::json!({ "ok": false }));
                }
            };
            println!("fs_upload: user={} dest={} name={} starting", user.user_id, dest_path, file_name);
            while let Ok(Some(chunk)) = field.chunk().await {
                total_written += chunk.len();
                if let Err(e) = f.write_all(&chunk).await {
                    println!("fs_upload: write chunk failed: {}", e);
                    return Json(serde_json::json!({ "ok": false }));
                }
            }
            wrote = true;
            saved_name = Some(file_name);
            println!("fs_upload: finished bytes_len={}", total_written);
        }
    }

    if !wrote {
        println!("fs_upload: no file field received, dest={}", dest_path);
        return Json(serde_json::json!({ "ok": false }));
    }

    Json(serde_json::json!({ "ok": true, "name": saved_name.unwrap_or_default(), "bytes": total_written }))
}

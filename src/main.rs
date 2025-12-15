use axum::{
    extract::{Extension, Request, State},
    http::{Method, StatusCode},
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

#[derive(Clone)]
struct AppState {
    device_codes: &'static Lazy<Mutex<HashMap<String, i64>>>,
    db: Pool<Postgres>,
    jwt_secret: String,
}

static DEVICE_CODES: Lazy<Mutex<HashMap<String, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Deserialize)]
struct SignupReq {
    email: String,
    password: String,
}

#[derive(Serialize)]
struct SignupResp {
    user_id: String,
}

#[derive(Deserialize)]
struct LoginReq {
    email: String,
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
    let db = PgPoolOptions::new().max_connections(5).connect(&db_url).await.unwrap();
    sqlx::query(
        "create table if not exists users (
            id uuid primary key,
            email text unique not null,
            password_hash text not null,
            created_at timestamptz default now()
        )",
    )
    .execute(&db)
    .await
    .unwrap();
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
        .route("/api/cloud/signup", post(signup))
        .route("/api/auth/signup", post(auth_signup))
        .route("/api/auth/login", post(auth_login))
        .route("/api/cloud/device/code", post(device_code))
        .route("/api/cloud/device/authorize", post(device_authorize))
        .with_state(state)
        .merge(protected)
        .fallback_service(static_service)
        .layer(cors);
    let addr: SocketAddr = ([0, 0, 0, 0], 8000).into();
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
    let existing = sqlx::query_scalar::<_, Uuid>("select id from users where email = $1")
        .bind(&req.email)
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
        let _ = sqlx::query("insert into users (id, email, password_hash) values ($1, $2, $3)")
            .bind(uid)
            .bind(&req.email)
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

async fn auth_signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
    let existing = sqlx::query_scalar::<_, Uuid>("select id from users where email = $1")
        .bind(&req.email)
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
        let _ = sqlx::query("insert into users (id, email, password_hash) values ($1, $2, $3)")
            .bind(uid)
            .bind(&req.email)
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
    let row = sqlx::query_as::<_, (Uuid, String, String)>("select id, password_hash, email from users where email = $1")
        .bind(&req.email)
        .fetch_optional(&st.db)
        .await
        .unwrap();
    if let Some((uid, pwd_hash, _email)) = row {
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
    let token = hdr.strip_prefix("Bearer ").unwrap_or("");
    if token.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(st.jwt_secret.as_bytes()),
        &Validation::default(),
    )
    .map_err(|_| StatusCode::UNAUTHORIZED)?;
    let uid = Uuid::parse_str(&data.claims.sub).map_err(|_| StatusCode::UNAUTHORIZED)?;
    req.extensions_mut().insert(AuthUser { user_id: uid });
    Ok(next.run(req).await)
}

async fn whoami(State(st): State<AppState>, Extension(user): Extension<AuthUser>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let rec = sqlx::query_scalar::<_, String>("select email from users where id = $1")
        .bind(user.user_id)
        .fetch_optional(&st.db)
        .await
        .unwrap();
    if let Some(email) = rec {
        Ok(Json(serde_json::json!({ "user_id": user.user_id.to_string(), "email": email })))
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

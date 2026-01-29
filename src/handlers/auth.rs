use axum::{
    extract::{Extension, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use password_hash::SaltString;
use uuid::Uuid;
use chrono::{Duration, Utc};
use jsonwebtoken::{encode, EncodingKey, Header};
use std::path::Path;

use crate::state::AppState;
use crate::models::auth::{SignupReq, SignupResp, LoginReq, LoginResp, Claims, AuthUser};

pub async fn signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
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
        let user_root = Path::new(&st.storage_path).join("vol1").join("User").join(&req.username);
        let _ = std::fs::create_dir_all(&user_root);
        uid
    };
    Json(SignupResp { user_id: id.to_string() })
}

pub async fn auth_signup(State(st): State<AppState>, Json(req): Json<SignupReq>) -> impl IntoResponse {
    signup(State(st), Json(req)).await
}

pub async fn auth_login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Result<Json<LoginResp>, (StatusCode, Json<serde_json::Value>)> {
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
                name: _username.clone(),
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

pub async fn whoami(State(st): State<AppState>, Extension(user): Extension<AuthUser>) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
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

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use uuid::Uuid;

use infra::AppState;
use domain::auth::{Claims, AuthUser};

pub async fn check_setup(State(st): State<AppState>, req: Request, next: Next) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    
    // 允许访问初始化相关的接口
    if path == "/api/system/init/state" || path == "/api/system/init" || path == "/health" || path == "/version" {
        return Ok(next.run(req).await);
    }

    // 检查是否已初始化
    let initialized = st.system_service.is_initialized().await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    
    if !initialized {
        // 如果未初始化，拦截所有其他请求，提示需要初始化
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }

    Ok(next.run(req).await)
}

pub async fn require_auth(State(st): State<AppState>, mut req: Request, next: Next) -> Result<Response, StatusCode> {
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
        if std::env::var("ALLOW_INSECURE_AUTH").ok().as_deref() == Some("1") {
            req.extensions_mut().insert(AuthUser { user_id: Uuid::nil(), username: "dev".to_string() });
            return Ok(next.run(req).await);
        }
        println!("Auth failed: missing token for {}", req.uri().path());
        return Err(StatusCode::UNAUTHORIZED);
    }
    // Dev-only bypass: allow raw UUID tokens when ALLOW_INSECURE_AUTH=1
    if std::env::var("ALLOW_INSECURE_AUTH").ok().as_deref() == Some("1") {
        if let Ok(uid) = Uuid::parse_str(token.as_str()) {
            req.extensions_mut().insert(AuthUser { user_id: uid, username: "dev".to_string() });
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
    req.extensions_mut().insert(AuthUser { user_id: uid, username: data.claims.name });
    Ok(next.run(req).await)
}

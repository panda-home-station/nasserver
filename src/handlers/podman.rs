use axum::{
    body::Body,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use hyper::{body::to_bytes, Body as HyperBody, Client, Method, Request};
use hyperlocal::{UnixClientExt, Uri};
use serde::{Deserialize, Serialize};
use crate::state::AppState;

#[derive(Serialize)]
pub struct ContainerInfo {
    id: String,
    names: Vec<String>,
    image: String,
    state: String,
    status: Option<String>,
    created: i64,
    ports: Vec<(u16, Option<u16>, Option<String>)>,
}

#[derive(Serialize)]
pub struct ImageInfo {
    id: String,
    repo_tags: Vec<String>,
    size: i64,
    created: i64,
}

#[derive(Deserialize)]
struct PodmanPort {
    #[serde(rename = "hostPort")]
    host_port: Option<u16>,
    #[serde(rename = "containerPort")]
    container_port: Option<u16>,
    #[serde(rename = "protocol")]
    protocol: Option<String>,
}

#[derive(Deserialize)]
struct PodmanContainer {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "Names")]
    names: Option<Vec<String>>,
    #[serde(rename = "Image")]
    image: String,
    #[serde(rename = "State")]
    state: String,
    #[serde(rename = "Status")]
    status: Option<String>,
    #[serde(rename = "Created")]
    created: i64,
    #[serde(rename = "Ports")]
    ports: Option<Vec<PodmanPort>>,
}

#[derive(Deserialize)]
struct PodmanImage {
    #[serde(rename = "Id")]
    id: String,
    #[serde(rename = "RepoTags")]
    repo_tags: Option<Vec<String>>,
    #[serde(rename = "Size")]
    size: i64,
    #[serde(rename = "Created")]
    created: i64,
}

fn podman_socket() -> Option<String> {
    if let Ok(p) = std::env::var("PODMAN_SOCKET") {
        if !p.trim().is_empty() && std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    // Try XDG_RUNTIME_DIR (rootless)
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let p = format!("{}/podman/podman.sock", xdg);
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    // Try explicit user runtime (rootless fallback)
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        let p = format!("/run/user/{}/podman/podman.sock", uid);
        if std::path::Path::new(&p).exists() {
            return Some(p);
        }
    }
    // Try system (rootful)
    let p = "/run/podman/podman.sock";
    if std::path::Path::new(p).exists() {
        return Some(p.to_string());
    }
    None
}

async fn call_podman(method: Method, subpath: &str, query: Option<&str>) -> Result<(StatusCode, bytes::Bytes), String> {
    let sock = podman_socket().ok_or_else(|| "podman socket not found".to_string())?;
    let client: Client<_, HyperBody> = Client::unix();

    let bases = ["/v5.0.0/libpod", "/v4.0.0/libpod", "/v3.0.0/libpod", "/v2.0.0/libpod", "/v1.0.0/libpod"];
    
    let mut last: Option<(StatusCode, bytes::Bytes)> = None;
    for base in bases {
        let mut path = format!("{}/{}", base, subpath.trim_start_matches('/'));
        if let Some(q) = query {
            path.push('?');
            path.push_str(q);
        }
        let uri = Uri::new(&sock, &path);

        let req = Request::builder()
            .method(method.clone())
            .uri(uri)
            .body(HyperBody::empty())
            .map_err(|e: hyper::http::Error| e.to_string())?;

        let resp = client.request(req).await.map_err(|e: hyper::Error| e.to_string())?;
        let status_v02 = resp.status();
        let status = StatusCode::from_u16(status_v02.as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(resp.into_body()).await.map_err(|e: hyper::Error| e.to_string())?;
        if status.is_success() {
            return Ok((status, body));
        }
        last = Some((status, body));
    }
    
    if let Some((status, body)) = last {
        return Ok((status, body));
    }
    Err("podman API not found".to_string())
}

pub async fn list_containers(State(_st): State<AppState>) -> Response {
    match call_podman(Method::GET, "/containers/json", Some("all=true")).await {
        Ok((status, body)) => {
            if status.is_success() {
                let items: Vec<PodmanContainer> = serde_json::from_slice(&body).unwrap_or_default();
                let mapped: Vec<ContainerInfo> = items
                    .into_iter()
                    .map(|c| ContainerInfo {
                        id: c.id,
                        names: c.names.unwrap_or_default(),
                        image: c.image,
                        state: c.state,
                        status: c.status,
                        created: c.created,
                        ports: c
                            .ports
                            .unwrap_or_default()
                            .into_iter()
                            .map(|p| (p.host_port.unwrap_or(0), p.container_port, p.protocol))
                            .collect(),
                    })
                    .collect();
                Json(mapped).into_response()
            } else {
                (status, String::from_utf8_lossy(body.as_ref()).to_string()).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

pub async fn list_images(State(_st): State<AppState>) -> Response {
    match call_podman(Method::GET, "/images/json", Some("all=true")).await {
        Ok((status, body)) => {
            if status.is_success() {
                let items: Vec<PodmanImage> = serde_json::from_slice(&body).unwrap_or_default();
                let mapped: Vec<ImageInfo> = items
                    .into_iter()
                    .map(|c| ImageInfo {
                        id: c.id,
                        repo_tags: c.repo_tags.unwrap_or_default(),
                        size: c.size,
                        created: c.created,
                    })
                    .collect();
                Json(mapped).into_response()
            } else {
                (status, String::from_utf8_lossy(body.as_ref()).to_string()).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

#[derive(Deserialize)]
pub struct IdPayload {
    id: String,
}

pub async fn start_container(State(_st): State<AppState>, Json(payload): Json<IdPayload>) -> Response {
    let path = format!("/containers/{}/start", payload.id);
    match call_podman(Method::POST, &path, None).await {
        Ok((status, body)) => {
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                Json(serde_json::json!({ "ok": true })).into_response()
            } else {
                let msg = String::from_utf8_lossy(&body);
                (status, Json(serde_json::json!({ "message": msg }))).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e }))).into_response(),
    }
}

pub async fn stop_container(State(_st): State<AppState>, Json(payload): Json<IdPayload>) -> Response {
    let path = format!("/containers/{}/stop", payload.id);
    match call_podman(Method::POST, &path, Some("timeout=10")).await {
        Ok((status, body)) => {
            if status.is_success() || status == StatusCode::NOT_MODIFIED {
                Json(serde_json::json!({ "ok": true })).into_response()
            } else {
                let msg = String::from_utf8_lossy(&body);
                (status, Json(serde_json::json!({ "message": msg }))).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e }))).into_response(),
    }
}

pub async fn restart_container(State(_st): State<AppState>, Json(payload): Json<IdPayload>) -> Response {
    let path = format!("/containers/{}/restart", payload.id);
    match call_podman(Method::POST, &path, Some("timeout=5")).await {
        Ok((status, body)) => {
            if status.is_success() {
                Json(serde_json::json!({ "ok": true })).into_response()
            } else {
                let msg = String::from_utf8_lossy(&body);
                (status, Json(serde_json::json!({ "message": msg }))).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e }))).into_response(),
    }
}

pub async fn remove_container(State(_st): State<AppState>, Json(payload): Json<IdPayload>) -> Response {
    let path = format!("/containers/{}", payload.id);
    match call_podman(Method::DELETE, &path, Some("force=true")).await {
        Ok((status, body)) => {
            if status.is_success() {
                Json(serde_json::json!({ "ok": true })).into_response()
            } else {
                let msg = String::from_utf8_lossy(&body);
                (status, Json(serde_json::json!({ "message": msg }))).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct PullPayload {
    image: String,
    tag: Option<String>,
}

pub async fn pull_image(State(st): State<AppState>, Json(payload): Json<PullPayload>) -> Response {
    let full_image = if let Some(tag) = payload.tag {
        format!("{}:{}", payload.image, tag)
    } else {
        payload.image.clone()
    };
    
    // Use registry settings if needed (omitted here for brevity, standard podman pull handles config)
    // But podman pull api needs 'reference'
    
    let q = format!("reference={}&policy=always", urlencoding::encode(&full_image));
    match call_podman(Method::POST, "/images/pull", Some(&q)).await {
        Ok((status, _)) => {
            if status.is_success() {
                Json(serde_json::json!({ "ok": true })).into_response()
            } else {
                (status, Json(serde_json::json!({ "ok": false }))).into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

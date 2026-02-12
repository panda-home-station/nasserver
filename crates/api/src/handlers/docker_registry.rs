use axum::{extract::Query, extract::State, response::IntoResponse, Json};
use serde::Deserialize;
use axum::http::StatusCode;
use infra::AppState;

#[derive(Deserialize)]
pub struct RegistrySearchQuery {
    pub q: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub async fn search(State(st): State<AppState>, Query(params): Query<RegistrySearchQuery>) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Json(serde_json::json!({ "results": [], "next": false, "prev": false })).into_response();
    }
    let page = params.page.unwrap_or(1).max(1);
    let mut ps = params.page_size.unwrap_or(24);
    if ps > 100 { ps = 100; }
    if ps < 1 { ps = 1; }
    let url = format!(
        "https://hub.docker.com/v2/search/repositories/?query={}&page_size={}&page={}",
        urlencoding::encode(q.trim()),
        ps,
        page
    );
    let client = reqwest::Client::new();
    let resp: reqwest::Result<reqwest::Response> = client.get(url).send().await;

    // Helper: Check mirrors for tags if hub search fails
    async fn check_mirrors(q: &str, st: &AppState) -> Vec<serde_json::Value> {
        let mirrors = st.system_service.get_docker_mirrors().await.unwrap_or_default();
        let enabled: Vec<String> = mirrors.into_iter().filter_map(|m| {
            if m.get("enabled").and_then(|e| e.as_bool()).unwrap_or(false) {
                m.get("host").and_then(|h| h.as_str()).map(|s| s.to_string())
            } else {
                None
            }
        }).collect();
        if enabled.is_empty() { return vec![]; }

        let client = reqwest::Client::new();
        let mut results = Vec::new();
        let (ns, name) = if q.contains('/') {
            let parts: Vec<&str> = q.splitn(2, '/').collect();
            (parts[0], parts[1])
        } else {
            ("library", q)
        };

        for host in enabled {
            let url = format!("https://{}/v2/{}/{}/tags/list", host, ns, name);
            if let Ok(resp) = client.get(&url).timeout(std::time::Duration::from_secs(2)).send().await {
                if resp.status().is_success() {
                     results.push(serde_json::json!({
                        "name": if ns == "library" { name.to_string() } else { format!("{}/{}", ns, name) },
                        "namespace": ns,
                        "description": format!("Found in mirror: {}", host),
                        "star_count": 0,
                        "pull_count": 0,
                        "is_official": ns == "library"
                     }));
                     break;
                }
            }
        }
        results
    }

    match resp {
        Ok(r) => {
            if !r.status().is_success() {
                let fallback = check_mirrors(&q, &st).await;
                return Json(serde_json::json!({ "results": fallback, "next": false, "prev": false })).into_response();
            }
            let v = r.json::<serde_json::Value>().await.unwrap_or_else(|_| serde_json::json!({}));
            let results = v.get("results").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            let mapped: Vec<serde_json::Value> = results
                .into_iter()
                .map(|it: serde_json::Value| {
                    let name = it.get("name").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let namespace = it.get("namespace").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let description = it.get("description").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let star_count = it.get("star_count").cloned().unwrap_or(serde_json::Value::Number(0.into()));
                    let pull_count = it.get("pull_count").cloned().unwrap_or(serde_json::Value::Number(0.into()));
                    serde_json::json!({
                        "name": name,
                        "namespace": namespace,
                        "description": description,
                        "star_count": star_count,
                        "pull_count": pull_count,
                        "is_official": namespace == "library"
                    })
                })
                .collect();
            let next = v.get("next").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            let prev = v.get("previous").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            Json(serde_json::json!({ "results": mapped, "next": next, "prev": prev })).into_response()
        }
        Err(_) => {
            let fallback = check_mirrors(&q, &st).await;
            Json(serde_json::json!({ "results": fallback, "next": false, "prev": false })).into_response()
        },
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct DockerSettings {
    pub mode: String,
    pub host: Option<String>,
}

pub async fn get_settings(State(st): State<AppState>) -> impl IntoResponse {
    let settings = st.system_service.get_docker_settings().await.unwrap_or_else(|_| serde_json::json!({ "mode": "none", "host": null }));
    Json(settings).into_response()
}

pub async fn set_settings(State(st): State<AppState>, Json(payload): Json<DockerSettings>) -> impl IntoResponse {
    let mode = payload.mode.trim().to_lowercase();
    let allowed = ["none", "daocloud", "netease", "tencent", "aliyun", "custom"];
    if !allowed.contains(&mode.as_str()) {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "invalid_mode" }))).into_response();
    }
    if mode == "custom" {
        let host = payload.host.as_deref().unwrap_or("").trim();
        if host.is_empty() {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "host_required" }))).into_response();
        }
    }
    let to_save = serde_json::to_value(payload).unwrap_or_default();
    let _ = st.system_service.set_docker_settings(to_save).await;
    StatusCode::OK.into_response()
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct MirrorEntry {
    pub id: String,
    pub name: String,
    pub host: String,
    pub enabled: bool,
}

pub async fn get_mirrors(State(st): State<AppState>) -> impl IntoResponse {
    let mirrors = st.system_service.get_docker_mirrors().await.unwrap_or_default();
    Json(mirrors).into_response()
}

pub async fn set_mirrors(State(st): State<AppState>, Json(payload): Json<Vec<MirrorEntry>>) -> impl IntoResponse {
    let mut sanitized: Vec<MirrorEntry> = Vec::new();
    for it in payload.into_iter() {
        let id = if it.id.trim().is_empty() { uuid::Uuid::new_v4().to_string() } else { it.id.trim().to_string() };
        let name = it.name.trim().to_string();
        let host = it.host.trim().to_string();
        if host.is_empty() {
            continue;
        }
        sanitized.push(MirrorEntry { id, name, host, enabled: it.enabled });
    }
    let to_save = serde_json::to_value(sanitized).unwrap_or_default();
    let _ = st.system_service.set_docker_mirrors(to_save.as_array().cloned().unwrap_or_default()).await;
    StatusCode::OK.into_response()
}

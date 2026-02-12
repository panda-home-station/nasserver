use axum::{extract::Query, extract::State, response::IntoResponse, Json};
use serde::Deserialize;
use axum::http::StatusCode;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct RegistrySearchQuery {
    pub q: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub async fn search(State(st): State<AppState>, Query(params): Query<RegistrySearchQuery>) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    println!("Registry Search: q='{}' page={:?} size={:?}", q, params.page, params.page_size);
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
    println!("Registry Search URL: {}", url);
    let client = reqwest::Client::new();
    let resp: reqwest::Result<reqwest::Response> = client.get(url).send().await;

    // Helper: Check mirrors for tags if hub search fails
    async fn check_mirrors(q: &str, st: &AppState) -> Vec<serde_json::Value> {
        // Load mirrors
        let list_json: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirrors'")
            .fetch_optional(&st.db)
            .await
            .unwrap_or(None);
        #[derive(serde::Deserialize)] struct MirrorEntry { host: String, enabled: bool }
        let mirrors: Vec<MirrorEntry> = list_json
            .and_then(|s| serde_json::from_str::<Vec<MirrorEntry>>(&s).ok())
            .unwrap_or_default();
        let enabled: Vec<String> = mirrors.into_iter().filter(|m| m.enabled).map(|m| m.host).collect();
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
            // Try OCI tags list endpoint
            // Some registries need token auth, but many public mirrors allow anonymous read
            let url = format!("https://{}/v2/{}/{}/tags/list", host, ns, name);
            println!("Mirror Search Probe: {}", url);
            // Short timeout for probe
            if let Ok(resp) = client.get(&url).timeout(std::time::Duration::from_secs(2)).send().await {
                if resp.status().is_success() {
                     // Found!
                     results.push(serde_json::json!({
                        "name": if ns == "library" { name.to_string() } else { format!("{}/{}", ns, name) },
                        "namespace": ns,
                        "description": format!("Found in mirror: {}", host),
                        "star_count": 0,
                        "pull_count": 0,
                        "is_official": ns == "library"
                     }));
                     // We found at least one valid mirror for this image, that's enough for a search result
                     break;
                }
            }
        }
        results
    }

    match resp {
        Ok(r) => {
            let status = r.status();
            println!("Registry Search Status: {}", status);
            if !status.is_success() {
                // Fallback to mirrors
                let fallback = check_mirrors(&q, &st).await;
                return Json(serde_json::json!({ "results": fallback, "next": false, "prev": false })).into_response();
            }
            let v = r.json::<serde_json::Value>().await.unwrap_or_else(|_| serde_json::json!({}));
            let results = v.get("results").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            // Pass-through minimal fields to frontend
            let mut mapped: Vec<serde_json::Value> = results
                .into_iter()
                .map(|it: serde_json::Value| {
                    let name = it.get("name").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let namespace = it.get("namespace").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let description = it.get("description").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let star_count = it.get("star_count").cloned().unwrap_or(serde_json::Value::Number(0.into()));
                    let pull_count = it.get("pull_count").cloned().unwrap_or(serde_json::Value::Number(0.into()));
                    let is_official = it.get("is_official").cloned().unwrap_or(serde_json::Value::Bool(false));
                    serde_json::json!({
                        "name": name,
                        "namespace": namespace,
                        "description": description,
                        "star_count": star_count,
                        "pull_count": pull_count,
                        "is_official": is_official
                    })
                })
                .collect();
            mapped.sort_by(|a: &serde_json::Value, b: &serde_json::Value| {
                let as_ = a.get("star_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                let bs_ = b.get("star_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                let ap = a.get("pull_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                let bp = b.get("pull_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                bs_.cmp(&as_).then(bp.cmp(&ap))
            });
            let next = v.get("next").and_then(|x: &serde_json::Value| x.as_str()).map(|s: &str| !s.is_empty()).unwrap_or(false);
            let prev = v.get("previous").and_then(|x: &serde_json::Value| x.as_str()).map(|s: &str| !s.is_empty()).unwrap_or(false);
            Json(serde_json::json!({ "results": mapped, "next": next, "prev": prev })).into_response()
        }
        Err(e) => {
            println!("Registry Search Error: {}", e);
            // Fallback to mirrors
            let fallback = check_mirrors(&q, &st).await;
            Json(serde_json::json!({ "results": fallback, "next": false, "prev": false })).into_response()
        },
    }
}

#[derive(Deserialize)]
pub struct HotQuery {
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub async fn hot(Query(params): Query<HotQuery>) -> impl IntoResponse {
    let client = reqwest::Client::new();
    let page = params.page.unwrap_or(1).max(1);
    let mut ps = params.page_size.unwrap_or(24);
    if ps > 100 { ps = 100; }
    if ps < 1 { ps = 1; }
    println!("Registry Hot: page={} size={}", page, ps);
    let url = format!("https://hub.docker.com/v2/repositories/library/?page_size={}&page={}", ps, page);
    println!("Registry Hot URL: {}", url);
    let resp: reqwest::Result<reqwest::Response> = client.get(url).send().await;
    match resp {
        Ok(r) => {
            let status = r.status();
            println!("Registry Hot Status: {}", status);
            if !status.is_success() {
                return Json(serde_json::json!({ "results": [], "next": false, "prev": false }));
            }
            let v = r.json::<serde_json::Value>().await.unwrap_or_else(|_| serde_json::json!({}));
            let mut results = v
                .get("results")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            results.sort_by(|a: &serde_json::Value, b: &serde_json::Value| {
                let as_ = a.get("star_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                let bs_ = b.get("star_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                let ap = a.get("pull_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                let bp = b.get("pull_count").and_then(|x: &serde_json::Value| x.as_i64()).unwrap_or(0);
                bs_.cmp(&as_).then(bp.cmp(&ap))
            });
            let mapped: Vec<serde_json::Value> = results
                .into_iter()
                .map(|it: serde_json::Value| {
                    let name = it.get("name").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let description = it.get("description").cloned().unwrap_or(serde_json::Value::String(String::new()));
                    let star_count = it.get("star_count").cloned().unwrap_or(serde_json::Value::Number(0.into()));
                    let pull_count = it.get("pull_count").cloned().unwrap_or(serde_json::Value::Number(0.into()));
                    serde_json::json!({
                        "name": name,
                        "namespace": "library",
                        "description": description,
                        "star_count": star_count,
                        "pull_count": pull_count,
                        "is_official": true
                    })
                })
                .collect();
            let next = v.get("next").and_then(|x: &serde_json::Value| x.as_str()).map(|s: &str| !s.is_empty()).unwrap_or(false);
            let prev = v.get("previous").and_then(|x: &serde_json::Value| x.as_str()).map(|s: &str| !s.is_empty()).unwrap_or(false);
            Json(serde_json::json!({ "results": mapped, "next": next, "prev": prev }))
        }
        Err(e) => {
            println!("Registry Hot Error: {}", e);
            Json(serde_json::json!({ "results": [], "next": false, "prev": false }))
        },
    }
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[allow(dead_code)]
pub struct DockerSettings {
    pub mode: String,
    pub host: Option<String>,
}

#[allow(dead_code)]
fn default_settings() -> DockerSettings {
    DockerSettings { mode: "none".to_string(), host: None }
}

#[allow(dead_code)]
pub async fn get_settings(State(st): State<AppState>) -> impl IntoResponse {
    let v: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirror'")
        .fetch_optional(&st.db)
        .await
        .unwrap_or(None);
    let mut settings = default_settings();
    if let Some(s) = v {
        if let Ok(cfg) = serde_json::from_str::<DockerSettings>(&s) {
            settings = cfg;
        }
    }
    Json(settings).into_response()
}

#[allow(dead_code)]
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
    let to_save = DockerSettings { mode, host: payload.host.clone().map(|s| s.trim().to_string()) };
    let json = serde_json::to_string(&to_save).unwrap_or_else(|_| "{\"mode\":\"none\"}".to_string());
    let _ = sqlx::query("insert or replace into system_config (key, value) values ('docker_mirror', $1)")
        .bind(json)
        .execute(&st.db)
        .await;
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
    let v: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirrors'")
        .fetch_optional(&st.db)
        .await
        .unwrap_or(None);
    let items: Vec<MirrorEntry> = v
        .and_then(|s| serde_json::from_str::<Vec<MirrorEntry>>(&s).ok())
        .unwrap_or_default();
    Json(items).into_response()
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
    let json = serde_json::to_string(&sanitized).unwrap_or_else(|_| "[]".to_string());
    let _ = sqlx::query("insert or replace into system_config (key, value) values ('docker_mirrors', $1)")
        .bind(json)
        .execute(&st.db)
        .await;
    StatusCode::OK.into_response()
}

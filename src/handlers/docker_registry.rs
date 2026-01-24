use axum::{extract::Query, response::IntoResponse, Json};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct RegistrySearchQuery {
    pub q: Option<String>,
    pub page: Option<usize>,
    pub page_size: Option<usize>,
}

pub async fn search(Query(params): Query<RegistrySearchQuery>) -> impl IntoResponse {
    let q = params.q.unwrap_or_default();
    if q.trim().is_empty() {
        return Json(serde_json::json!({ "results": [], "next": false, "prev": false }));
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
    let resp = client.get(url).send().await;
    match resp {
        Ok(r) => {
            if !r.status().is_success() {
                return Json(serde_json::json!({ "results": [], "next": false, "prev": false }));
            }
            let v = r.json::<serde_json::Value>().await.unwrap_or_else(|_| serde_json::json!({}));
            let results = v.get("results").and_then(|x| x.as_array()).cloned().unwrap_or_default();
            // Pass-through minimal fields to frontend
            let mut mapped: Vec<serde_json::Value> = results
                .into_iter()
                .map(|it| {
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
            mapped.sort_by(|a, b| {
                let as_ = a.get("star_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let bs_ = b.get("star_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let ap = a.get("pull_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let bp = b.get("pull_count").and_then(|x| x.as_i64()).unwrap_or(0);
                bs_.cmp(&as_).then(bp.cmp(&ap))
            });
            let next = v.get("next").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            let prev = v.get("previous").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            Json(serde_json::json!({ "results": mapped, "next": next, "prev": prev }))
        }
        Err(_) => Json(serde_json::json!({ "results": [], "next": false, "prev": false })),
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
    let url = format!("https://hub.docker.com/v2/repositories/library/?page_size={}&page={}", ps, page);
    let resp = client.get(url).send().await;
    match resp {
        Ok(r) => {
            if !r.status().is_success() {
                return Json(serde_json::json!({ "results": [], "next": false, "prev": false }));
            }
            let v = r.json::<serde_json::Value>().await.unwrap_or_else(|_| serde_json::json!({}));
            let mut results = v
                .get("results")
                .and_then(|x| x.as_array())
                .cloned()
                .unwrap_or_default();
            results.sort_by(|a, b| {
                let as_ = a.get("star_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let bs_ = b.get("star_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let ap = a.get("pull_count").and_then(|x| x.as_i64()).unwrap_or(0);
                let bp = b.get("pull_count").and_then(|x| x.as_i64()).unwrap_or(0);
                bs_.cmp(&as_).then(bp.cmp(&ap))
            });
            let mapped: Vec<serde_json::Value> = results
                .into_iter()
                .map(|it| {
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
            let next = v.get("next").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            let prev = v.get("previous").and_then(|x| x.as_str()).map(|s| !s.is_empty()).unwrap_or(false);
            Json(serde_json::json!({ "results": mapped, "next": next, "prev": prev }))
        }
        Err(_) => Json(serde_json::json!({ "results": [], "next": false, "prev": false })),
    }
}

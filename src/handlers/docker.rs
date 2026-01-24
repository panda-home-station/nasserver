use axum::{
    extract::State,
    response::IntoResponse,
    Json,
    http::StatusCode,
};
use serde::Serialize;
use bollard::Docker;
use bollard::container::{
    ListContainersOptions, StartContainerOptions, StopContainerOptions, RestartContainerOptions,
    RemoveContainerOptions,
};
use bollard::image::{ListImagesOptions, CreateImageOptions};
use futures_util::stream::StreamExt;
use sqlx::Row;

use crate::state::AppState;

fn docker_client() -> Docker {
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if host.starts_with("unix://") {
            let p = host.trim_start_matches("unix://");
            return Docker::connect_with_unix(p, 120, &bollard::API_DEFAULT_VERSION).unwrap();
        } else {
            // tcp or http(s)
            return Docker::connect_with_local_defaults().unwrap();
        }
    }
    Docker::connect_with_unix("/var/run/docker.sock", 120, &bollard::API_DEFAULT_VERSION).unwrap()
}

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

pub async fn list_containers(State(_st): State<AppState>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .list_containers(Some(ListContainersOptions::<String> {
            all: true,
            ..Default::default()
        }))
        .await;
    match res {
        Ok(list) => {
            let items: Vec<ContainerInfo> = list
                .into_iter()
                .map(|c| ContainerInfo {
                    id: c.id.unwrap_or_default(),
                    names: c.names.unwrap_or_default(),
                    image: c.image.unwrap_or_default(),
                    state: c.state.unwrap_or_default(),
                    status: c.status,
                    created: c.created.unwrap_or_default() as i64,
                    ports: c
                        .ports
                        .unwrap_or_default()
                        .into_iter()
                        .map(|p| (p.private_port, p.public_port, p.typ.map(|t| format!("{:?}", t))))
                        .collect(),
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_images(State(_st): State<AppState>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .list_images(Some(ListImagesOptions::<String> {
            all: true,
            ..Default::default()
        }))
        .await;
    match res {
        Ok(list) => {
            let items: Vec<ImageInfo> = list
                .into_iter()
                .map(|img| ImageInfo {
                    id: img.id,
                    repo_tags: img.repo_tags,
                    size: img.size,
                    created: img.created,
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct IdReq {
    id: String,
}

#[derive(serde::Deserialize)]
pub struct PullReq {
    image: String,
    tag: Option<String>,
}

pub async fn start_container(State(_st): State<AppState>, Json(req): Json<IdReq>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .start_container(&req.id, None::<StartContainerOptions<String>>)
        .await;
    match res {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn stop_container(State(_st): State<AppState>, Json(req): Json<IdReq>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .stop_container(&req.id, Some(StopContainerOptions { t: 10 }))
        .await;
    match res {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn restart_container(State(_st): State<AppState>, Json(req): Json<IdReq>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .restart_container(&req.id, Some(RestartContainerOptions { t: 5 }))
        .await;
    match res {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn remove_container(State(_st): State<AppState>, Json(req): Json<IdReq>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .remove_container(
            &req.id,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await;
    match res {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn pull_image(State(_st): State<AppState>, Json(req): Json<PullReq>) -> impl IntoResponse {
    let docker = docker_client();
    let original = req.image;
    let tag = req.tag.unwrap_or_else(|| "latest".to_string());
    eprintln!("docker pull start image={} tag={}", original, tag);
    // Load mirrors list or fallback to legacy single setting
    #[derive(serde::Deserialize)]
    struct MirrorEntry { host: String, enabled: bool }
    #[derive(serde::Deserialize)]
    struct DockerSettings { mode: String, host: Option<String> }
    let list_json: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirrors'")
        .fetch_optional(&_st.db)
        .await
        .unwrap_or(None);
    let mirrors: Vec<MirrorEntry> = list_json
        .and_then(|s| serde_json::from_str::<Vec<MirrorEntry>>(&s).ok())
        .unwrap_or_default();
    let enabled_hosts_preview: Vec<String> = mirrors.iter().filter(|m| m.enabled).map(|m| m.host.clone()).collect();
    eprintln!("docker pull enabled mirrors={:?}", enabled_hosts_preview);
    let legacy_json: Option<String> = sqlx::query_scalar("select value from system_config where key = 'docker_mirror'")
        .fetch_optional(&_st.db)
        .await
        .unwrap_or(None);
    let legacy: DockerSettings = legacy_json
        .and_then(|s| serde_json::from_str::<DockerSettings>(&s).ok())
        .unwrap_or(DockerSettings { mode: "none".to_string(), host: None });
    let has_host = original.contains('.') && original.contains('/');
    let mut candidates: Vec<(String, Option<String>)> = Vec::new();
    if has_host {
        candidates.push((original.clone(), None));
    } else {
        let enabled_hosts: Vec<String> = mirrors
            .into_iter()
            .filter(|m| m.enabled)
            .map(|m| m.host.trim().to_string())
            .collect();
        if enabled_hosts.is_empty() {
            let h = match legacy.mode.as_str() {
                "daocloud" => Some("docker.m.daocloud.io".to_string()),
                "netease" => Some("hub-mirror.c.163.com".to_string()),
                "tencent" => Some("mirror.ccs.tencentyun.com".to_string()),
                "aliyun" => Some("registry.aliyuncs.com".to_string()),
                "custom" => legacy.host.clone(),
                _ => None,
            };
            if let Some(h) = h {
                let ref_ = if original.contains('/') {
                    format!("{}/{}", h, original)
                } else {
                    format!("{}/library/{}", h, original)
                };
                candidates.push((ref_, Some(h)));
            }
        } else {
            for h in enabled_hosts {
                let ref_ = if original.contains('/') {
                    format!("{}/{}", h, original)
                } else {
                    format!("{}/library/{}", h, original)
                };
                candidates.push((ref_, Some(h)));
            }
        }
        candidates.push((original.clone(), None));
    }
    let mut used: Option<String> = None;
    let mut last_err: Option<String> = None;
    for (from_image, source) in candidates {
        let src_name = source.clone().unwrap_or_else(|| "docker.io".to_string());
        eprintln!("docker pull try source={} ref={}", src_name, from_image);
        let mut stream = docker.create_image(
            Some(CreateImageOptions {
                from_image,
                tag: tag.clone(),
                ..Default::default()
            }),
            None,
            None,
        );
        let mut ok = true;
        while let Some(update) = stream.next().await {
            match update {
                Ok(_) => {}
                Err(e) => {
                    ok = false;
                    last_err = Some(e.to_string());
                    eprintln!("docker pull error source={} err={}", src_name, last_err.as_deref().unwrap_or(""));
                    break;
                }
            }
        }
        if ok {
            used = source;
            if let Some(ref src) = used {
                eprintln!("docker pull success source={}", src);
            } else {
                eprintln!("docker pull success source=docker.io");
            }
            break;
        }
    }
    match used {
        Some(src) => Json(serde_json::json!({ "ok": true, "source": src })).into_response(),
        None => {
            if has_host {
                eprintln!("docker pull success source={}", original);
                Json(serde_json::json!({ "ok": true, "source": original })).into_response()
            } else {
                eprintln!("docker pull failed err={}", last_err.as_deref().unwrap_or(""));
                (StatusCode::INTERNAL_SERVER_ERROR, last_err.unwrap_or_else(|| "pull failed".to_string())).into_response()
            }
        }
    }
}

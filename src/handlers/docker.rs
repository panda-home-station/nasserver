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
    let from_image = req.image;
    let tag = req.tag.unwrap_or_else(|| "latest".to_string());
    let mut stream = docker.create_image(
        Some(CreateImageOptions {
            from_image,
            tag,
            ..Default::default()
        }),
        None,
        None,
    );
    while let Some(_update) = stream.next().await {
        // ignore progress details; we could stream to client later
    }
    Json(serde_json::json!({ "ok": true })).into_response()
}

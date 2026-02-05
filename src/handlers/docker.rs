use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions, RestartContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::models::{HostConfig, PortBinding};
use bollard::image::{CreateImageOptions, ListImagesOptions, RemoveImageOptions};
use bollard::volume::ListVolumesOptions;
use bollard::network::ListNetworksOptions;
use bollard::Docker;
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use sqlx::Row;

use crate::state::AppState;

fn docker_client() -> Docker {
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if host.starts_with("unix://") {
            let p = host.trim_start_matches("unix://");
            if let Ok(cli) = Docker::connect_with_unix(p, 120, &bollard::API_DEFAULT_VERSION) {
                return cli;
            }
        } else if let Ok(cli) = Docker::connect_with_local_defaults() {
            return cli;
        }
    }
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let podman_sock = format!("{}/podman/podman.sock", xdg);
        if std::path::Path::new(&podman_sock).exists() {
            if let Ok(cli) = Docker::connect_with_unix(&podman_sock, 120, &bollard::API_DEFAULT_VERSION) {
                return cli;
            }
        }
    }
    if let Ok(uid) = nix_current_uid() {
        let p = format!("/run/user/{}/podman/podman.sock", uid);
        if std::path::Path::new(&p).exists() {
            if let Ok(cli) = Docker::connect_with_unix(&p, 120, &bollard::API_DEFAULT_VERSION) {
                return cli;
            }
        }
    }
    let system_podman = "/run/podman/podman.sock";
    if std::path::Path::new(system_podman).exists() {
        if let Ok(cli) = Docker::connect_with_unix(system_podman, 120, &bollard::API_DEFAULT_VERSION) {
            return cli;
        }
    }
    let docker_sock = "/var/run/docker.sock";
    if std::path::Path::new(docker_sock).exists() {
        if let Ok(cli) = Docker::connect_with_unix(docker_sock, 120, &bollard::API_DEFAULT_VERSION) {
            return cli;
        }
    }
    Docker::connect_with_local_defaults().unwrap()
}

fn nix_current_uid() -> Result<u32, ()> {
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        Ok(uid)
    }
    #[cfg(not(unix))]
    {
        Err(())
    }
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
    exposed_ports: Vec<u16>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VolumeInfo {
    name: String,
    driver: String,
    mountpoint: String,
    created_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkInfo {
    id: String,
    name: String,
    driver: String,
    scope: String,
    internal: bool,
    attachable: bool,
    ingress: bool,
    #[serde(rename = "IPAM")]
    ipam: NetworkIpam,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkIpam {
    driver: String,
    config: Vec<NetworkIpamConfig>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkIpamConfig {
    subnet: Option<String>,
    gateway: Option<String>,
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
            let mut items: Vec<ImageInfo> = Vec::new();
            for img in list {
                let exposed_ports = if let Ok(inspect) = docker.inspect_image(&img.id).await {
                    inspect.config
                        .and_then(|config| config.exposed_ports)
                        .map(|ports| {
                            ports.keys()
                                .filter_map(|k| k.split('/').next()?.parse::<u16>().ok())
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                items.push(ImageInfo {
                    id: img.id,
                    repo_tags: img.repo_tags,
                    size: img.size,
                    created: img.created,
                    exposed_ports,
                });
            }
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_volumes(State(_st): State<AppState>) -> impl IntoResponse {
    let docker = docker_client();
    match docker.list_volumes(None::<ListVolumesOptions<String>>).await {
        Ok(list) => {
            let items: Vec<VolumeInfo> = list.volumes.unwrap_or_default()
                .into_iter()
                .map(|v| VolumeInfo {
                    name: v.name,
                    driver: v.driver,
                    mountpoint: v.mountpoint,
                    created_at: v.created_at,
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn list_networks(State(_st): State<AppState>) -> impl IntoResponse {
    let docker = docker_client();
    match docker.list_networks(None::<ListNetworksOptions<String>>).await {
        Ok(list) => {
            let items: Vec<NetworkInfo> = list
                .into_iter()
                .map(|n| {
                    let config = if let Some(ipam) = &n.ipam {
                        if let Some(config) = &ipam.config {
                            config.iter().map(|c| NetworkIpamConfig {
                                subnet: c.subnet.clone(),
                                gateway: c.gateway.clone(),
                            }).collect()
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    };

                    let ipam = NetworkIpam {
                        driver: n.ipam.as_ref().and_then(|i| i.driver.clone()).unwrap_or_default(),
                        config,
                    };

                    NetworkInfo {
                        id: n.id.unwrap_or_default(),
                        name: n.name.unwrap_or_default(),
                        driver: n.driver.unwrap_or_default(),
                        scope: n.scope.unwrap_or_default(),
                        internal: n.internal.unwrap_or_default(),
                        attachable: n.attachable.unwrap_or_default(),
                        ingress: n.ingress.unwrap_or_default(),
                        ipam,
                    }
                })
                .collect();
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct IdReq {
    id: String,
}

#[derive(Deserialize)]
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

pub async fn pull_image(State(st): State<AppState>, Json(req): Json<PullReq>) -> impl IntoResponse {
    let docker = docker_client();
    let original = req.image;
    let tag = req.tag.unwrap_or_else(|| "latest".to_string());
    #[derive(Deserialize)]
    struct MirrorEntry {
        host: String,
        enabled: bool,
    }
    #[derive(Deserialize)]
    struct DockerSettings {
        mode: String,
        host: Option<String>,
    }
    let list_json: Option<String> =
        sqlx::query_scalar("select value from system_config where key = 'docker_mirrors'")
            .fetch_optional(&st.db)
            .await
            .unwrap_or(None);
    let mirrors: Vec<MirrorEntry> = list_json
        .and_then(|s| serde_json::from_str::<Vec<MirrorEntry>>(&s).ok())
        .unwrap_or_default();
    let legacy_json: Option<String> =
        sqlx::query_scalar("select value from system_config where key = 'docker_mirror'")
            .fetch_optional(&st.db)
            .await
            .unwrap_or(None);
    let legacy: DockerSettings = legacy_json
        .and_then(|s| serde_json::from_str::<DockerSettings>(&s).ok())
        .unwrap_or(DockerSettings {
            mode: "none".to_string(),
            host: None,
        });
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
                    break;
                }
            }
        }
        if ok {
            used = source;
            break;
        }
    }
    match used {
        Some(src) => Json(serde_json::json!({ "ok": true, "source": src })).into_response(),
        None => {
            if has_host {
                Json(serde_json::json!({ "ok": true, "source": original })).into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    last_err.unwrap_or_else(|| "pull failed".to_string()),
                )
                    .into_response()
            }
        }
    }
}

pub async fn remove_image(State(_st): State<AppState>, Json(req): Json<IdReq>) -> impl IntoResponse {
    let docker = docker_client();
    let res = docker
        .remove_image(
            &req.id,
            Some(RemoveImageOptions {
                force: true,
                ..Default::default()
            }),
            None,
        )
        .await;
    match res {
        Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateContainerReq {
    image_id: String,
    name: Option<String>,
    cpu_limit: Option<f64>,
    memory_limit: Option<i64>,
    auto_start: Option<bool>,
    ports: Option<Vec<PortMapping>>,
    volumes: Option<Vec<String>>,
    env: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct PortMapping {
    host: String,
    container: String,
}

pub async fn create_container(State(_st): State<AppState>, Json(req): Json<CreateContainerReq>) -> impl IntoResponse {
    let docker = docker_client();
    
    let mut host_config = HostConfig {
        ..Default::default()
    };
    
    if let Some(cpu) = req.cpu_limit {
        host_config.nano_cpus = Some((cpu * 1_000_000_000.0) as i64);
    }
    
    if let Some(mem) = req.memory_limit {
        host_config.memory = Some(mem * 1024 * 1024 * 1024);
    }
    
    let mut exposed_ports = std::collections::HashMap::new();
    if let Some(ports) = req.ports {
        let mut port_bindings = std::collections::HashMap::new();
        for p in ports {
            let key = format!("{}/tcp", p.container);
            let bindings = vec![bollard::models::PortBinding {
                host_ip: None,
                host_port: Some(p.host),
            }];
            port_bindings.insert(key.clone(), Some(bindings));
            exposed_ports.insert(key, std::collections::HashMap::new());
        }
        host_config.port_bindings = Some(port_bindings);
    }
    
    if let Some(volumes) = req.volumes {
        host_config.binds = Some(volumes);
    }
    
    let mut config = Config {
        image: Some(req.image_id),
        host_config: Some(host_config),
        ..Default::default()
    };
    
    if !exposed_ports.is_empty() {
        config.exposed_ports = Some(exposed_ports);
    }
    
    if let Some(env) = req.env {
        config.env = Some(env);
    }
    
    let options: Option<CreateContainerOptions<String>> = if let Some(name) = req.name {
        Some(CreateContainerOptions {
            name,
            ..Default::default()
        })
    } else {
        None
    };
    
    let res = docker.create_container(options, config).await;
    
    match res {
        Ok(info) => {
            let id = info.id;
            let _ = docker.start_container(&id, None::<StartContainerOptions<String>>).await;
            Json(serde_json::json!({ "ok": true, "id": id })).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

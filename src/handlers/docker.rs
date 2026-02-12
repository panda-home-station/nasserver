use axum::{
    extract::{State, Extension},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use bollard::container::{
    Config, CreateContainerOptions, ListContainersOptions, RemoveContainerOptions, RestartContainerOptions, StartContainerOptions,
    StopContainerOptions,
};
use bollard::models::{HostConfig, RestartPolicy, RestartPolicyNameEnum};
use bollard::image::{CreateImageOptions, ListImagesOptions, RemoveImageOptions};
use bollard::volume::ListVolumesOptions;
use bollard::network::ListNetworksOptions;
use bollard::Docker;
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};

use crate::state::AppState;
use crate::models::auth::AuthUser;

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

fn nix_current_uid() -> Result<u32, String> {
    #[cfg(unix)]
    {
        let uid = unsafe { libc::getuid() };
        Ok(uid)
    }
    #[cfg(not(unix))]
    {
        Err("Unsupported platform".to_string())
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
    env: Vec<String>,
    volumes: Vec<String>,
}

#[derive(Serialize)]
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

pub async fn list_gpus(State(_st): State<AppState>) -> impl IntoResponse {
    let gpus = crate::handlers::gpu::get_system_gpus();
    Json(gpus).into_response()
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
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e.to_string() }))).into_response(),
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
                let (exposed_ports, env, volumes) = if let Ok(inspect) = docker.inspect_image(&img.id).await {
                    let config = inspect.config.unwrap_or_default();
                    
                    let ports = config.exposed_ports
                        .map(|ports| {
                            ports.keys()
                                .filter_map(|k| k.split('/').next()?.parse::<u16>().ok())
                                .collect()
                        })
                        .unwrap_or_default();

                    let env = config.env.unwrap_or_default();
                    
                    let vols = config.volumes
                        .map(|v| v.keys().cloned().collect())
                        .unwrap_or_default();
                        
                    (ports, env, vols)
                } else {
                    (Vec::new(), Vec::new(), Vec::new())
                };

                items.push(ImageInfo {
                    id: img.id,
                    repo_tags: img.repo_tags,
                    size: img.size,
                    created: img.created,
                    exposed_ports,
                    env,
                    volumes,
                });
            }
            Json(items).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e.to_string() }))).into_response(),
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
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e.to_string() }))).into_response(),
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
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "message": e.to_string() }))).into_response(),
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
        let _src_name = source.clone().unwrap_or_else(|| "docker.io".to_string());
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
#[allow(dead_code)]
pub struct CreateContainerReq {
    image_id: String,
    name: Option<String>,
    cpu_limit: Option<f64>,
    memory_limit: Option<i64>,
    auto_start: Option<bool>,
    ports: Option<Vec<PortMapping>>,
    volumes: Option<Vec<String>>,
    env: Option<Vec<String>>,
    gpu_id: Option<String>,
    privileged: Option<bool>,
    cap_add: Option<Vec<String>>,
    network_mode: Option<String>,
    cmd: Option<Vec<String>>,
    entrypoint: Option<Vec<String>>,
}

#[derive(Deserialize)]
pub struct PortMapping {
    host: String,
    container: String,
}

use uuid::Uuid;

use std::os::unix::fs::PermissionsExt;

pub async fn create_container(State(st): State<AppState>, Extension(user): Extension<AuthUser>, Json(req): Json<CreateContainerReq>) -> impl IntoResponse {
    let docker = docker_client();
    
    // Determine container name to ensure storage isolation
    // If name is not provided, generate a unique one
    let container_name = match &req.name {
        Some(n) if !n.trim().is_empty() => n.trim().to_string(),
        _ => format!("pnas-{}", Uuid::new_v4().simple())
    };

    // Sanitize container name to prevent path traversal
    if container_name.contains("..") || container_name.contains('/') || container_name.contains('\\') {
        return (StatusCode::BAD_REQUEST, "Invalid container name".to_string()).into_response();
    }
    
    let mut host_config = HostConfig {
        ..Default::default()
    };

    if let Some(true) = req.auto_start {
        host_config.restart_policy = Some(RestartPolicy {
            name: Some(RestartPolicyNameEnum::ALWAYS),
            ..Default::default()
        });
    }

    if let Some(privileged) = req.privileged {
        host_config.privileged = Some(privileged);
    }

    if let Some(cap_add) = &req.cap_add {
        host_config.cap_add = Some(cap_add.clone());
    }

    if let Some(network_mode) = &req.network_mode {
        host_config.network_mode = Some(network_mode.clone());
    }
    
    if let Some(cpu) = req.cpu_limit {
        host_config.nano_cpus = Some((cpu * 1_000_000_000.0) as i64);
    }
    
    if let Some(mem) = req.memory_limit {
        host_config.memory = Some(mem * 1024 * 1024 * 1024);
    }
    
    let mut extra_env = Vec::new();

    if let Some(gpu_id) = &req.gpu_id {
        let gpu_config = crate::handlers::gpu::resolve_gpu_config(gpu_id);
        
        if !gpu_config.device_requests.is_empty() {
            let mut reqs = host_config.device_requests.take().unwrap_or_default();
            reqs.extend(gpu_config.device_requests);
            host_config.device_requests = Some(reqs);
        }
        
        if !gpu_config.devices.is_empty() {
             let mut devs = host_config.devices.take().unwrap_or_default();
             devs.extend(gpu_config.devices);
             host_config.devices = Some(devs);
        }

        if !gpu_config.security_opts.is_empty() {
            let mut opts = host_config.security_opt.take().unwrap_or_default();
            opts.extend(gpu_config.security_opts);
            host_config.security_opt = Some(opts);
        }

        if !gpu_config.group_adds.is_empty() {
            let mut groups = host_config.group_add.take().unwrap_or_default();
            groups.extend(gpu_config.group_adds);
            host_config.group_add = Some(groups);
        }

        extra_env.extend(gpu_config.env);
    }

    let mut exposed_ports = std::collections::HashMap::new();
    if let Some(ports) = &req.ports {
        let mut port_bindings = std::collections::HashMap::new();
        for p in ports {
            let key = format!("{}/tcp", p.container.trim());
            let bindings = vec![bollard::models::PortBinding {
                host_ip: None,
                host_port: Some(p.host.trim().to_string()),
            }];
            port_bindings.insert(key.clone(), Some(bindings));
            exposed_ports.insert(key, std::collections::HashMap::new());
        }
        host_config.port_bindings = Some(port_bindings);
    }
    
    if let Some(volumes) = &req.volumes {
        let mut new_volumes = Vec::new();
        // Use resolved container name as app scope

        for v in volumes {
            // Parse volume string "host:container[:mode]"
            let parts: Vec<&str> = v.split(':').collect();
            if parts.len() < 2 {
                continue; 
            }
            
            let host_part = parts[0].trim();
            
            // Security check for directory traversal using path components
            let path = std::path::Path::new(host_part);
            for component in path.components() {
                if let std::path::Component::ParentDir = component {
                    return (StatusCode::BAD_REQUEST, "Directory traversal detected in volume path".to_string()).into_response();
                }
            }

            let container_part = parts[1].trim();
            let mode = if parts.len() > 2 { Some(parts[2].trim()) } else { None };
            
            let new_host_path_buf;
            
            if host_part.starts_with('/') {
                 // Bind mount: Resolve path using user context
                 match crate::handlers::docs::resolve_path(&st, &user.username, host_part).await {
                     Ok(p) => new_host_path_buf = p,
                     Err(e) => return (StatusCode::FORBIDDEN, format!("Access denied to path {}: {}", host_part, e)).into_response(),
                 }
            } else {
                 // Named volume: Pass through, but we need to put it in the list directly
                 let mode_str = if let Some(m) = mode {
                    if m.contains('Z') || m.contains('z') { m.to_string() } else { format!("{},Z", m) }
                 } else { "Z".to_string() };
                 
                 new_volumes.push(format!("{}:{}:{}", host_part, container_part, mode_str));
                 continue;
            }

            let new_host_path = new_host_path_buf.as_path();
                
            // Ensure directory exists for bind mounts
            if let Err(e) = tokio::fs::create_dir_all(&new_host_path).await {
                eprintln!("Failed to create volume directory {:?}: {}", new_host_path, e);
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to create volume directory: {}", e)).into_response();
            }

            // Set permissions to 777 to allow any container user (root or non-root) to write
            // This is necessary because we use "keep-id" which might map users in a way that prevents writing to host-owned dirs
            if let Err(e) = tokio::fs::set_permissions(&new_host_path, std::fs::Permissions::from_mode(0o777)).await {
                eprintln!("Failed to set permissions for volume directory {:?}: {}", new_host_path, e);
            }

            // Apply SELinux label 'Z' to allow container access
            let mode_str = if let Some(m) = mode {
                if m.contains('Z') || m.contains('z') {
                    m.to_string()
                } else {
                    format!("{},Z", m)
                }
            } else {
                "Z".to_string()
            };

            let new_vol_str = format!("{}:{}:{}", new_host_path.to_string_lossy(), container_part, mode_str);
            new_volumes.push(new_vol_str);
        }
        
        host_config.binds = Some(new_volumes);
        // Enable keep-id to map container user to host user for permission access
        host_config.userns_mode = Some("keep-id".to_string());
    } else {
        // Even without volumes, keep-id is good practice for rootless
        host_config.userns_mode = Some("keep-id".to_string());
    }
    
    let mut config = Config {
        image: Some(req.image_id.trim().to_string()),
        host_config: Some(host_config),
        ..Default::default()
    };

    if let Some(cmd) = &req.cmd {
        config.cmd = Some(cmd.clone());
    }

    if let Some(entrypoint) = &req.entrypoint {
        config.entrypoint = Some(entrypoint.clone());
    }
    
    if !exposed_ports.is_empty() {
        config.exposed_ports = Some(exposed_ports);
    }
    
    let mut final_env = req.env.unwrap_or_default();
    final_env.extend(extra_env);
    
    if !final_env.is_empty() {
        config.env = Some(final_env);
    }
    
    // Always use the resolved container_name
    let options = Some(CreateContainerOptions {
        name: container_name,
        ..Default::default()
    });
    
    let res = docker.create_container(options, config).await;
    
    match res {
        Ok(info) => {
            let id = info.id;
            let _ = docker.start_container(&id, None::<StartContainerOptions<String>>).await;
            Json(serde_json::json!({ "ok": true, "id": id })).into_response()
        }
        Err(e) => {
            eprintln!("Create container error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
        }
    }
}

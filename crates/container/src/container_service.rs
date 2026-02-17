use bollard::Docker;
use bollard::container::{Config, CreateContainerOptions, ListContainersOptions, StartContainerOptions, StopContainerOptions, RestartContainerOptions, RemoveContainerOptions};
use bollard::models::{HostConfig, PortBinding, RestartPolicy, RestartPolicyNameEnum};
use bollard::volume::ListVolumesOptions;
use bollard::network::ListNetworksOptions;
use bollard::image::{CreateImageOptions, ListImagesOptions, RemoveImageOptions};
use async_trait::async_trait;
use futures_util::StreamExt;
use domain::{Result, Error, container::{
    ContainerService, ContainerInfo, ImageInfo, VolumeInfo, NetworkInfo, NetworkIpam, NetworkIpamConfig,
    CreateContainerReq, PullImageReq
}};
// Remove models import

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct ContainerServiceImpl {
    docker: Docker,
    storage_path: String,
    pulling: Arc<Mutex<HashMap<String, PullStatus>>>,
}

#[derive(Clone, Default)]
struct PullStatus {
    progress: f64,
    layers: HashMap<String, LayerStatus>,
    error: Option<String>,
}

#[derive(Clone, Default)]
struct LayerStatus {
    current: i64,
    total: i64,
}

impl ContainerServiceImpl {
    pub fn new(storage_path: String) -> Self {
        let docker = Self::docker_client();
        Self { 
            docker, 
            storage_path,
            pulling: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn normalize_path(&self, p: &str) -> String {
        let s = p.replace("\\", "/");
        let parts: Vec<&str> = s.split('/')
            .filter(|x| !x.is_empty() && *x != "." && *x != "..")
            .collect();
        format!("/{}", parts.join("/"))
    }

    fn is_safe_name(&self, name: &str) -> bool {
        if name.is_empty() || name.len() > 128 {
            return false;
        }
        name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
    }

    fn validate_create_req(&self, req: &CreateContainerReq) -> Result<()> {
        if let Some(name) = &req.name {
            if !self.is_safe_name(name) {
                return Err(Error::BadRequest(format!("Invalid container name: {}", name)));
            }
        }

        if req.image_id.is_empty() {
            return Err(Error::BadRequest("Image ID is required".to_string()));
        }

        if let Some(ports) = &req.ports {
            for p in ports {
                if p.host.parse::<u16>().is_err() || p.container.parse::<u16>().is_err() {
                    return Err(Error::BadRequest(format!("Invalid port mapping: {}:{}", p.host, p.container)));
                }
            }
        }

        if let Some(cpu) = req.cpu_limit {
            if cpu < 0.0 || cpu > 1024.0 { // Arbitrary limit
                return Err(Error::BadRequest("CPU limit must be between 0 and 1024".to_string()));
            }
        }

        if let Some(mem) = req.memory_limit {
            if mem < 0.0 || mem > 1024.0 * 1024.0 * 1024.0 * 128.0 { // 128GB limit
                return Err(Error::BadRequest("Memory limit is too high".to_string()));
            }
        }

        if let Some(env) = &req.env {
            if env.len() > 100 {
                return Err(Error::BadRequest("Too many environment variables".to_string()));
            }
            for e in env {
                if e.len() > 4096 {
                    return Err(Error::BadRequest("Environment variable too long".to_string()));
                }
            }
        }

        Ok(())
    }

    fn resolve_physical_path(&self, username: Option<&str>, virtual_path: &str) -> Result<std::path::PathBuf> {
        let clean_path = self.normalize_path(virtual_path);
        let storage_root = std::path::Path::new(&self.storage_path);
        
        let p = if clean_path.starts_with("/AppData/") || clean_path == "/AppData" {
            let parts: Vec<&str> = clean_path.split('/').filter(|x| !x.is_empty()).collect();
            let mut p = storage_root.join("vol1").join("AppData");
            if parts.len() > 1 {
                let rel = parts[1..].join("/");
                p = p.join(rel);
            }
            p
        } else {
            let username = username.unwrap_or("admin");
            let rel = if clean_path.starts_with('/') { &clean_path[1..] } else { &clean_path };
            storage_root.join("vol1").join("User").join(username).join(rel)
        };

        // Final security check: ensure the resolved path is within the storage root
        if !p.starts_with(storage_root) {
            return Err(Error::Forbidden(format!("Path escape detected: {}", virtual_path)));
        }

        Ok(p)
    }

    fn validate_pull_req(&self, req: &PullImageReq) -> Result<()> {
        if req.image.is_empty() {
            return Err(Error::BadRequest("Image name is required".to_string()));
        }
        if !req.image.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' || c == ':') {
            return Err(Error::BadRequest(format!("Invalid image name: {}", req.image)));
        }
        Ok(())
    }

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
        if let Ok(uid) = Self::nix_current_uid() {
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

    fn nix_current_uid() -> std::result::Result<u32, String> {
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
}

#[async_trait]
impl ContainerService for ContainerServiceImpl {
    async fn list_containers(&self) -> Result<Vec<ContainerInfo>> {
        let containers = self.docker
            .list_containers(Some(ListContainersOptions::<String> {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(containers
            .into_iter()
            .map(|c| {
                let ports = c.ports.unwrap_or_default().into_iter().map(|p| {
                    (p.private_port, p.public_port, p.typ.map(|t| format!("{:?}", t)))
                }).collect();

                ContainerInfo {
                    id: c.id.unwrap_or_default(),
                    names: c.names.unwrap_or_default(),
                    image: c.image.unwrap_or_default(),
                    state: c.state.unwrap_or_default(),
                    status: c.status,
                    created: c.created.unwrap_or_default(),
                    ports,
                }
            })
            .collect())
    }

    async fn start_container(&self, id: &str) -> Result<()> {
        self.docker.start_container(id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| Error::Internal(e.to_string()))
    }

    async fn stop_container(&self, id: &str) -> Result<()> {
        self.docker.stop_container(id, None::<StopContainerOptions>)
            .await
            .map_err(|e| Error::Internal(e.to_string()))
    }

    async fn restart_container(&self, id: &str) -> Result<()> {
        self.docker.restart_container(id, None::<RestartContainerOptions>)
            .await
            .map_err(|e| Error::Internal(e.to_string()))
    }

    async fn remove_container(&self, id: &str) -> Result<()> {
        self.docker.remove_container(id, None::<RemoveContainerOptions>)
            .await
            .map_err(|e| Error::Internal(e.to_string()))
    }

    async fn list_images(&self) -> Result<Vec<ImageInfo>> {
        let images = self.docker
            .list_images(Some(ListImagesOptions::<String> {
                all: true,
                ..Default::default()
            }))
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        let mut items = Vec::new();
        for img in images {
            let (exposed_ports, env, volumes) = if let Ok(inspect) = self.docker.inspect_image(&img.id).await {
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
                status: Some("ready".to_string()),
                progress: None,
                error: None,
            });
        }

        // Add pulling images
        let pulling = self.pulling.lock().await;
        for (image, status) in pulling.iter() {
            items.insert(0, ImageInfo {
                id: format!("pulling-{}", image),
                repo_tags: vec![image.clone()],
                size: 0,
                created: chrono::Utc::now().timestamp(),
                exposed_ports: Vec::new(),
                env: Vec::new(),
                volumes: Vec::new(),
                status: Some(if status.error.is_some() { "error".to_string() } else { "pulling".to_string() }),
                progress: Some(status.progress),
                error: status.error.clone(),
            });
        }

        Ok(items)
    }

    async fn remove_image(&self, id: &str) -> Result<()> {
        self.docker.remove_image(id, None::<RemoveImageOptions>, None)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_volumes(&self) -> Result<Vec<VolumeInfo>> {
        let volumes = self.docker.list_volumes(None::<ListVolumesOptions<String>>)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        
        Ok(volumes.volumes.unwrap_or_default().into_iter().map(|v| VolumeInfo {
            name: v.name,
            driver: v.driver,
            mountpoint: v.mountpoint,
            created_at: v.created_at,
        }).collect())
    }

    async fn list_networks(&self) -> Result<Vec<NetworkInfo>> {
        let networks = self.docker.list_networks(None::<ListNetworksOptions<String>>)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        Ok(networks.into_iter().map(|n| {
            let config = if let Some(ipam) = &n.ipam {
                if let Some(config) = &ipam.config {
                    config.iter().map(|c: &bollard::models::IpamConfig| NetworkIpamConfig {
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
        }).collect())
    }

    async fn create_container(&self, req: CreateContainerReq) -> Result<()> {
        self.validate_create_req(&req)?;

        let mut host_config = HostConfig::default();
        
        if let Some(ports) = &req.ports {
            let mut port_bindings = std::collections::HashMap::new();
            for p in ports {
                let container_port = format!("{}/tcp", p.container);
                let binding = PortBinding {
                    host_ip: None,
                    host_port: Some(p.host.clone()),
                };
                port_bindings.insert(container_port, Some(vec![binding]));
            }
            host_config.port_bindings = Some(port_bindings);
        }

        if let Some(volumes) = &req.volumes {
            let mut resolved_binds = Vec::new();
            for v_str in volumes {
                let mut host_part = "";
                let mut container_part = "";
                let mut options_part = "";

                let mut parts = v_str.splitn(3, ':');
                
                host_part = parts.next().unwrap_or("");
                if let Some(c) = parts.next() {
                    container_part = c;
                }
                if let Some(o) = parts.next() {
                    options_part = o;
                }

                let final_host_path: String;
                if host_part.starts_with('/') || host_part.starts_with('.') {
                    let physical_host_path_buf = self.resolve_physical_path(req.username.as_deref(), host_part)?;
                    final_host_path = physical_host_path_buf.display().to_string();
                    
                    if let Err(e) = tokio::fs::create_dir_all(&physical_host_path_buf).await {
                        eprintln!("Failed to create host directory {}: {}", physical_host_path_buf.display(), e);
                    }
                } else {
                    final_host_path = host_part.to_string();
                }

                if !options_part.is_empty() {
                    let valid_options = ["z", "Z", "ro", "rw", "rslave", "rprivate", "rshared", "delegated", "cached", "consistent"];
                    for opt in options_part.split(',') {
                        if !valid_options.contains(&opt) {
                            return Err(Error::BadRequest(format!("Invalid volume option '{}' in volume bind '{}'. Allowed options are: {}", opt, v_str, valid_options.join(", "))));
                        }
                    }
                }

                let mut bind_string = final_host_path;
                bind_string.push(':');
                bind_string.push_str(container_part);
                if !options_part.is_empty() {
                    bind_string.push(':');
                    bind_string.push_str(options_part);
                }
                resolved_binds.push(bind_string);
            }
            host_config.binds = Some(resolved_binds);
        }

        if let Some(cpu) = req.cpu_limit {
            host_config.nano_cpus = Some((cpu * 1e9) as i64);
        }

        if let Some(mem) = req.memory_limit {
            host_config.memory = Some((mem * 1024.0 * 1024.0 * 1024.0) as i64);
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

        let mut env = req.env.clone().unwrap_or_default();

        if let Some(gpu_id) = &req.gpu_id {
            if !gpu_id.is_empty() {
                let gpu_config = gpu::resolve_gpu_config(gpu_id);
                if !gpu_config.device_requests.is_empty() {
                    host_config.device_requests = Some(gpu_config.device_requests);
                }
                if !gpu_config.devices.is_empty() {
                    host_config.devices = Some(gpu_config.devices);
                }
                if !gpu_config.security_opts.is_empty() {
                    host_config.security_opt = Some(gpu_config.security_opts);
                }
                if !gpu_config.env.is_empty() {
                    env.extend(gpu_config.env);
                }
            }
        }

        if req.auto_start.unwrap_or(false) {
            host_config.restart_policy = Some(RestartPolicy {
                name: Some(RestartPolicyNameEnum::ALWAYS),
                maximum_retry_count: None,
            });
        }

        let config = Config {
            image: Some(req.image_id),
            host_config: Some(host_config),
            env: Some(env),
            cmd: req.cmd,
            ..Default::default()
        };

        let options = req.name.as_ref().map(|name| CreateContainerOptions {
            name: name.clone(),
            ..Default::default()
        });

        let res = self.docker.create_container(options, config)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;

        if req.auto_start.unwrap_or(true) {
            self.docker.start_container(&res.id, None::<StartContainerOptions<String>>)
                .await
                .map_err(|e| Error::Internal(e.to_string()))?;
        }

        Ok(())
    }

    async fn pull_image(&self, req: PullImageReq) -> Result<()> {
        self.validate_pull_req(&req)?;
        let from_image = if let Some(tag) = &req.tag {
            if tag.is_empty() {
                req.image.clone()
            } else {
                format!("{}:{}", req.image, tag)
            }
        } else {
            req.image.clone()
        };

        let docker = self.docker.clone();
        let pulling = self.pulling.clone();
        let image_name = from_image.clone();

        tokio::spawn(async move {
            {
                let mut p = pulling.lock().await;
                p.insert(image_name.clone(), PullStatus::default());
            }

            let options = CreateImageOptions {
                from_image: image_name.clone(),
                ..Default::default()
            };

            let mut stream = docker.create_image(Some(options), None, None);
            let mut success = true;
            while let Some(res) = stream.next().await {
                match res {
                    Ok(info) => {
                        let mut p = pulling.lock().await;
                        if let Some(s) = p.get_mut(&image_name) {
                            if let (Some(id), Some(progress_detail)) = (info.id, info.progress_detail) {
                                if let (Some(current), Some(total)) = (progress_detail.current, progress_detail.total) {
                                    if total > 0 {
                                        s.layers.insert(id, LayerStatus { current, total });
                                    }
                                }
                            }
                            
                            // Calculate overall progress
                            let total_current: i64 = s.layers.values().map(|l| l.current).sum();
                            let total_max: i64 = s.layers.values().map(|l| l.total).sum();
                            if total_max > 0 {
                                s.progress = (total_current as f64 / total_max as f64) * 100.0;
                            }
                        }
                    }
                    Err(e) => {
                        let mut p = pulling.lock().await;
                        if let Some(s) = p.get_mut(&image_name) {
                            s.error = Some(e.to_string());
                        }
                        success = false;
                        break;
                    }
                }
            }

            if success {
                let mut p = pulling.lock().await;
                p.remove(&image_name);
            }
        });

        Ok(())
    }
}

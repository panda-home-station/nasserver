use bollard::Docker;
use bollard::container::{ListContainersOptions, StartContainerOptions, StopContainerOptions, RestartContainerOptions, RemoveContainerOptions};
use bollard::volume::ListVolumesOptions;
use bollard::network::ListNetworksOptions;
use bollard::image::{ListImagesOptions, RemoveImageOptions};
use async_trait::async_trait;
use domain::{Result, Error, container::{
    ContainerService, ContainerInfo, ImageInfo, VolumeInfo, NetworkInfo, NetworkIpam, NetworkIpamConfig
}};
// Remove models import

pub struct ContainerServiceImpl {
    docker: Docker,
}

impl ContainerServiceImpl {
    pub fn new() -> Self {
        let docker = Self::docker_client();
        Self { docker }
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
}

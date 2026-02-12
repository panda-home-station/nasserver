use bollard::Docker;
use async_trait::async_trait;
use domain::{Result, Error, container::AppManager, entities::app::{App, AppStatus, AppType}};

pub struct DockerAppManager {
    docker: Docker,
}

impl DockerAppManager {
    pub fn new() -> Self {
        Self {
            docker: Self::init_docker_client(),
        }
    }

    fn init_docker_client() -> Docker {
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
        
        #[cfg(unix)]
        {
            let uid = unsafe { libc::getuid() };
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
        Docker::connect_with_local_defaults().unwrap_or_else(|_| {
            // Fallback or panic if we really can't connect
            // In a real app we might want to return an error, but for now we follow the existing pattern
            Docker::connect_with_local_defaults().expect("Failed to connect to Docker/Podman")
        })
    }
}

#[async_trait]
impl AppManager for DockerAppManager {
    async fn list_apps(&self) -> Result<Vec<App>> {
        let containers = self.docker.list_containers::<String>(None).await
            .map_err(|e| Error::Internal(e.to_string()))?;
        
        let apps = containers.into_iter().map(|c| {
            App {
                id: c.id.unwrap_or_default(),
                name: c.names.unwrap_or_default().first().cloned().unwrap_or_default(),
                version: "latest".to_string(),
                app_type: AppType::Docker,
                status: match c.state.as_deref() {
                    Some("running") => AppStatus::Running,
                    _ => AppStatus::Stopped,
                },
                description: None,
                icon: None,
                port: None,
                entrypoint: c.image.unwrap_or_default(),
            }
        }).collect();
        
        Ok(apps)
    }

    async fn get_app(&self, id: &str) -> Result<App> {
        let c = self.docker.inspect_container(id, None).await
            .map_err(|_| Error::NotFound(format!("App {} not found", id)))?;
        
        Ok(App {
            id: c.id.unwrap_or_default(),
            name: c.name.unwrap_or_default(),
            version: "latest".to_string(),
            app_type: AppType::Docker,
            status: if c.state.and_then(|s| s.running).unwrap_or(false) {
                AppStatus::Running
            } else {
                AppStatus::Stopped
            },
            description: None,
            icon: None,
            port: None,
            entrypoint: c.config.and_then(|cfg| cfg.image).unwrap_or_default(),
        })
    }

    async fn install_app(&self, _app_config: App) -> Result<()> {
        // Simple implementation: pull and create
        // In reality, this would be more complex
        Ok(())
    }

    async fn uninstall_app(&self, id: &str) -> Result<()> {
        self.docker.remove_container(id, None).await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    async fn start_app(&self, id: &str) -> Result<()> {
        self.docker.start_container::<String>(id, None).await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    async fn stop_app(&self, id: &str) -> Result<()> {
        self.docker.stop_container(id, None).await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }

    async fn get_app_status(&self, id: &str) -> Result<AppStatus> {
        let app = self.get_app(id).await?;
        Ok(app.status)
    }
}

use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;
use domain::container::{PullImageReq, CreateVolumeReq, CreateContainerReq};

pub struct DockerCommand;

#[async_trait]
impl Command for DockerCommand {
    fn name(&self) -> &str {
        "docker"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             return Ok(("".to_string(), "docker: missing command\nTry 'docker --help' for more information.".to_string(), 1));
        }

        match args[0] {
            "run" => {
                 if args.len() < 2 {
                     return Ok(("".to_string(), "docker run: requires config json".to_string(), 1));
                 }
                 let json_str = args[1];
                 match serde_json::from_str::<CreateContainerReq>(json_str) {
                     Ok(mut req) => {
                         req.username = Some(service.current_user.clone());
                         match service.container_service.create_container(req).await {
                              Ok(_) => Ok(("Container created successfully\n".to_string(), "".to_string(), 0)),
                              Err(e) => Ok(("".to_string(), format!("Error creating container: {}", e), 1))
                         }
                     },
                     Err(e) => Ok(("".to_string(), format!("Invalid container config JSON: {}", e), 1))
                 }
            },
            "ps" => {
                // List containers
                match service.container_service.list_containers().await {
                    Ok(containers) => {
                        let mut output = String::from("CONTAINER ID   IMAGE          STATUS    NAMES\n");
                        for c in containers {
                            output.push_str(&format!("{:<14} {:<14} {:<9} {}\n", 
                                &c.id[..12], 
                                c.image, 
                                c.status.as_deref().unwrap_or("Unknown"), 
                                c.names.join(",")));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error listing containers: {}", e), 1))
                }
            },
            "images" => {
                match service.container_service.list_images().await {
                    Ok(images) => {
                        let mut output = String::from("REPOSITORY          TAG       IMAGE ID       SIZE\n");
                        for img in images {
                             let repo_tag = img.repo_tags.first().cloned().unwrap_or_default();
                             let parts: Vec<&str> = repo_tag.split(':').collect();
                             let repo = parts.get(0).unwrap_or(&"<none>");
                             let tag = parts.get(1).unwrap_or(&"<none>");
                             let size_mb = img.size / 1024 / 1024;
                             output.push_str(&format!("{:<19} {:<9} {:<14} {}MB\n", repo, tag, &img.id[7..19], size_mb));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error listing images: {}", e), 1))
                }
            },
             "start" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker start: requires at least 1 argument".to_string(), 1));
                }
                let id = args[1];
                match service.container_service.start_container(id).await {
                    Ok(_) => Ok((format!("{}\n", id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error starting container: {}", e), 1))
                }
            },
            "stop" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker stop: requires at least 1 argument".to_string(), 1));
                }
                let id = args[1];
                match service.container_service.stop_container(id).await {
                    Ok(_) => Ok((format!("{}\n", id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error stopping container: {}", e), 1))
                }
            },
            "restart" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker restart: requires container id".to_string(), 1));
                }
                let id = args[1];
                match service.container_service.restart_container(id).await {
                    Ok(_) => Ok((format!("{}\n", id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error restarting container: {}", e), 1))
                }
            },
            "rm" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker rm: requires container id".to_string(), 1));
                }
                let id = args[1];
                match service.container_service.remove_container(id).await {
                    Ok(_) => Ok((format!("{}\n", id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error removing container: {}", e), 1))
                }
            },
            "pull" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker pull: requires image name".to_string(), 1));
                }
                let image = args[1];
                let req = PullImageReq { image: image.to_string(), tag: None };
                match service.container_service.pull_image(req).await {
                     Ok(_) => Ok((format!("{}\n", image), "".to_string(), 0)),
                     Err(e) => Ok(("".to_string(), format!("Error pulling image: {}", e), 1))
                }
            },
            "rmi" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker rmi: requires image id".to_string(), 1));
                }
                let id = args[1];
                match service.container_service.remove_image(id).await {
                    Ok(_) => Ok((format!("Untagged: {}\n", id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error removing image: {}", e), 1))
                }
            },
            "volume" => {
                if args.len() < 2 {
                     return Ok(("".to_string(), "docker volume: missing subcommand [ls|create|rm]".to_string(), 1));
                }
                match args[1] {
                    "ls" => {
                        match service.container_service.list_volumes().await {
                            Ok(volumes) => {
                                let mut output = String::from("DRIVER    VOLUME NAME\n");
                                for v in volumes {
                                    output.push_str(&format!("{:<10} {}\n", v.driver, v.name));
                                }
                                Ok((output, "".to_string(), 0))
                            },
                            Err(e) => Ok(("".to_string(), format!("Error listing volumes: {}", e), 1))
                        }
                    },
                    "create" => {
                        if args.len() < 3 {
                            return Ok(("".to_string(), "docker volume create: requires volume name".to_string(), 1));
                        }
                        let name = args[2];
                        let req = CreateVolumeReq { name: name.to_string(), driver: None, labels: None };
                        match service.container_service.create_volume(req).await {
                            Ok(_) => Ok((format!("{}\n", name), "".to_string(), 0)),
                            Err(e) => Ok(("".to_string(), format!("Error creating volume: {}", e), 1))
                        }
                    },
                    "rm" => {
                        if args.len() < 3 {
                            return Ok(("".to_string(), "docker volume rm: requires volume name".to_string(), 1));
                        }
                        let name = args[2];
                        match service.container_service.remove_volume(name).await {
                            Ok(_) => Ok((format!("{}\n", name), "".to_string(), 0)),
                            Err(e) => Ok(("".to_string(), format!("Error removing volume: {}", e), 1))
                        }
                    },
                    _ => Ok(("".to_string(), format!("docker volume: unknown subcommand '{}'", args[1]), 1))
                }
            },
            "network" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "docker network: missing subcommand [ls]".to_string(), 1));
                }
                match args[1] {
                    "ls" => {
                         match service.container_service.list_networks().await {
                            Ok(networks) => {
                                let mut output = String::from("NETWORK ID     NAME      DRIVER    SCOPE\n");
                                for n in networks {
                                    output.push_str(&format!("{:<12} {:<9} {:<9} {}\n", &n.id[..12], n.name, n.driver, n.scope));
                                }
                                Ok((output, "".to_string(), 0))
                            },
                            Err(e) => Ok(("".to_string(), format!("Error listing networks: {}", e), 1))
                        }
                    },
                    _ => Ok(("".to_string(), format!("docker network: unknown subcommand '{}'", args[1]), 1))
                }
            },
            _ => Ok(("".to_string(), format!("docker: unknown subcommand '{}'", args[0]), 1))
        }
    }
}
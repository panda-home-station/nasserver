use crate::service::TerminalService;
use crate::error::Result;
use crate::commands::Command;
use async_trait::async_trait;

pub struct SysInfoCommand;

#[async_trait]
impl Command for SysInfoCommand {
    fn name(&self) -> &str {
        "sysinfo"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             match service.system_service.get_current_stats().await {
                Ok(stats) => {
                    match serde_json::to_string_pretty(&stats) {
                        Ok(s) => Ok((s, "".to_string(), 0)),
                        Err(e) => Ok(("".to_string(), format!("sysinfo: serialization error: {}", e), 1))
                    }
                },
                Err(e) => Ok(("".to_string(), format!("sysinfo: failed to get stats: {}", e), 1))
            }
        } else {
            match args[0] {
                "health" => {
                    match service.system_service.health().await {
                        Ok(h) => Ok((format!("{}\n", h), "".to_string(), 0)),
                        Err(e) => Ok(("".to_string(), format!("Error getting health: {}", e), 1))
                    }
                },
                "device" => {
                    match service.system_service.get_device_info().await {
                        Ok(info) => Ok((format!("{:?}\n", info), "".to_string(), 0)),
                        Err(e) => Ok(("".to_string(), format!("Error getting device info: {}", e), 1))
                    }
                },
                "gpu" => {
                    let gpus = service.system_service.get_gpus().await;
                    Ok((format!("{:?}\n", gpus), "".to_string(), 0))
                },
                "ports" => {
                    if args.len() < 2 {
                         return Ok(("".to_string(), "sysinfo ports: requires port numbers".to_string(), 1));
                    }
                    let ports: Vec<u16> = args[1..].iter().filter_map(|p| p.parse().ok()).collect();
                    match service.system_service.check_ports(ports).await {
                        Ok(statuses) => Ok((format!("{:?}\n", statuses), "".to_string(), 0)),
                        Err(e) => Ok(("".to_string(), format!("Error checking ports: {}", e), 1))
                    }
                },
                "docker-mirrors" => {
                    if args.len() < 2 {
                         return Ok(("".to_string(), "sysinfo docker-mirrors: requires subcommand [get|set]".to_string(), 1));
                    }
                    match args[1] {
                        "get" => {
                             match service.system_service.get_docker_mirrors().await {
                                 Ok(mirrors) => Ok((format!("{:?}\n", mirrors), "".to_string(), 0)),
                                 Err(e) => Ok(("".to_string(), format!("Error getting docker mirrors: {}", e), 1))
                             }
                        },
                        "set" => {
                             if args.len() < 3 {
                                 return Ok(("".to_string(), "sysinfo docker-mirrors set: requires mirrors json array".to_string(), 1));
                             }
                             let json_str = args[2];
                             match serde_json::from_str::<Vec<serde_json::Value>>(json_str) {
                                 Ok(mirrors) => {
                                     match service.system_service.set_docker_mirrors(mirrors).await {
                                         Ok(_) => Ok(("Docker mirrors updated\n".to_string(), "".to_string(), 0)),
                                         Err(e) => Ok(("".to_string(), format!("Error setting docker mirrors: {}", e), 1))
                                     }
                                 },
                                 Err(e) => Ok(("".to_string(), format!("Invalid mirrors JSON: {}", e), 1))
                             }
                        },
                        _ => Ok(("".to_string(), format!("sysinfo docker-mirrors: unknown subcommand '{}'", args[1]), 1))
                    }
                },
                 "docker-settings" => {
                    if args.len() < 2 {
                         return Ok(("".to_string(), "sysinfo docker-settings: requires subcommand [get|set]".to_string(), 1));
                    }
                    match args[1] {
                        "get" => {
                            match service.system_service.get_docker_settings().await {
                                Ok(settings) => Ok((format!("{:?}\n", settings), "".to_string(), 0)),
                                Err(e) => Ok(("".to_string(), format!("Error getting docker settings: {}", e), 1))
                            }
                        },
                        "set" => {
                             if args.len() < 3 {
                                 return Ok(("".to_string(), "sysinfo docker-settings set: requires settings json object".to_string(), 1));
                             }
                             let json_str = args[2];
                             match serde_json::from_str::<serde_json::Value>(json_str) {
                                 Ok(settings) => {
                                     match service.system_service.set_docker_settings(settings).await {
                                         Ok(_) => Ok(("Docker settings updated\n".to_string(), "".to_string(), 0)),
                                         Err(e) => Ok(("".to_string(), format!("Error setting docker settings: {}", e), 1))
                                     }
                                 },
                                 Err(e) => Ok(("".to_string(), format!("Invalid settings JSON: {}", e), 1))
                             }
                        },
                        _ => Ok(("".to_string(), format!("sysinfo docker-settings: unknown subcommand '{}'", args[1]), 1))
                    }
                },
                "history" => {
                     let query = domain::system::StatsHistoryQuery {
                         start: None,
                         end: None,
                         limit: Some(10), // Default to last 10
                     };
                     match service.system_service.get_stats_history(query).await {
                         Ok(history) => {
                             let mut output = String::from("TIMESTAMP                    CPU(%)  MEM(%)  DISK(%)\n");
                             for stats in history {
                                 output.push_str(&format!("{:?}  {:.1}    {:.1}    {:.1}\n", 
                                     stats.created_at.unwrap_or_default(), 
                                     stats.cpu_usage, 
                                     stats.memory_usage, 
                                     stats.disk_usage
                                 ));
                             }
                             Ok((output, "".to_string(), 0))
                         },
                         Err(e) => Ok(("".to_string(), format!("Error getting stats history: {}", e), 1))
                     }
                },
                _ => Ok(("".to_string(), format!("sysinfo: unknown subcommand '{}'", args[0]), 1))
            }
        }
    }
}
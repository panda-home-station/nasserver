use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;
use domain::downloader::{CreateDownloadReq, ControlDownloadReq, ResolveMagnetReq, StartMagnetDownloadReq};

pub struct DownloadCommand;

#[async_trait]
impl Command for DownloadCommand {
    fn name(&self) -> &str {
        "dl"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             return Ok(("".to_string(), "dl: missing subcommand\nUsage: dl [list|add|pause|resume|delete|stats|magnet]".to_string(), 1));
        }

        match args[0] {
            "list" => {
                match service.downloader_service.list_tasks().await {
                    Ok(resp_list) => {
                        let mut output = String::from("ID      FILENAME                        STATUS     PROGRESS   SPEED\n");
                        for resp in resp_list {
                             let task = resp.task;
                             let progress = if task.total_bytes > 0 {
                                 (task.downloaded_bytes as f64 / task.total_bytes as f64 * 100.0) as u64
                             } else {
                                 0
                             };
                             output.push_str(&format!("{:<7} {:<31} {:<10} {:>3}%       {}/s\n", 
                                 &task.id.to_string()[..6], 
                                 &task.filename.chars().take(30).collect::<String>(), 
                                 task.status, 
                                 progress,
                                 task.speed
                             ));
                        }
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error listing downloads: {}", e), 1))
                }
            },
            "add" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "dl add: requires url [path]".to_string(), 1));
                }
                let url = args[1];
                let path = if args.len() > 2 { Some(args[2].to_string()) } else { None };
                let req = CreateDownloadReq {
                    url: url.to_string(),
                    path,
                };
                match service.downloader_service.create_task(&service.current_user, req).await {
                    Ok(_) => Ok((format!("Download added: {}\n", url), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error adding download: {}", e), 1))
                }
            },
            "pause" | "resume" | "delete" => {
                if args.len() < 2 {
                     return Ok(("".to_string(), format!("dl {}: requires task id", args[0]), 1));
                }
                let id = args[1];
                let action = match args[0] {
                    "pause" => "pause",
                    "resume" => "resume",
                    "delete" => "delete",
                    _ => unreachable!(),
                };
                
                let req = ControlDownloadReq {
                    action: action.to_string(),
                };
                
                match service.downloader_service.control_task(id, req).await {
                     Ok(_) => Ok((format!("Task {} {}\n", id, action), "".to_string(), 0)),
                     Err(e) => Ok(("".to_string(), format!("Error controlling task: {}", e), 1))
                }
            },
            "stats" => {
                 match service.downloader_service.get_stats().await {
                     Ok(stats) => {
                         let output = format!("Total Tasks: {}\nActive Tasks: {}\nDownload Speed: {} bytes/s\n", 
                             stats.total_tasks, stats.active_tasks, stats.download_speed);
                         Ok((output, "".to_string(), 0))
                     },
                     Err(e) => Ok(("".to_string(), format!("Error getting stats: {}", e), 1))
                 }
            },
            "magnet" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "dl magnet: requires subcommand [resolve|start]".to_string(), 1));
                }
                match args[1] {
                    "resolve" => {
                        if args.len() < 3 {
                            return Ok(("".to_string(), "dl magnet resolve: requires magnet url".to_string(), 1));
                        }
                        let url = args[2];
                        let req = ResolveMagnetReq { magnet_url: url.to_string() };
                        match service.downloader_service.resolve_magnet(req).await {
                             Ok(resp) => {
                                 let mut output = format!("Magnet Resolved: {}\nToken: {}\nFiles:\n", resp.name.as_deref().unwrap_or("Unknown"), resp.token);
                                 for file in resp.files {
                                     output.push_str(&format!("  [{}] {} ({})\n", file.index, file.name, file.size));
                                 }
                                 Ok((output, "".to_string(), 0))
                             },
                             Err(e) => Ok(("".to_string(), format!("Error resolving magnet: {}", e), 1))
                        }
                    },
                    "start" => {
                        // dl magnet start <token> <file_indices>
                        if args.len() < 4 {
                            return Ok(("".to_string(), "dl magnet start: requires token and file indices (comma separated)".to_string(), 1));
                        }
                        let token = args[2];
                        let indices_str = args[3];
                        let files: Vec<usize> = indices_str.split(',')
                            .filter_map(|s| s.trim().parse().ok())
                            .collect();
                            
                        let req = StartMagnetDownloadReq { 
                            token: token.to_string(),
                            files,
                            path: None, 
                        };
                         match service.downloader_service.start_magnet_download(&service.current_user, req).await {
                             Ok(_) => Ok((format!("Magnet download started with token {}\n", token), "".to_string(), 0)),
                             Err(e) => Ok(("".to_string(), format!("Error starting magnet download: {}", e), 1))
                        }
                    },
                     _ => Ok(("".to_string(), format!("dl magnet: unknown subcommand '{}'", args[1]), 1))
                }
            },
            _ => Ok(("".to_string(), format!("dl: unknown subcommand '{}'", args[0]), 1))
        }
    }
}
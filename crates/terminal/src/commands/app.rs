use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;

pub struct AppCommand;

#[async_trait]
impl Command for AppCommand {
    fn name(&self) -> &str {
        "app"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             return Ok(("".to_string(), "app: missing subcommand\nUsage: app [list|start|stop|uninstall|status]".to_string(), 1));
        }

        match args[0] {
            "list" => {
                match service.app_manager.list_apps().await {
                    Ok(apps) => {
                         let mut output = String::from("ID              NAME            STATUS\n");
                         for app in apps {
                             // Fetch status for each app? might be slow.
                             // For now just list them.
                             let status = match service.app_manager.get_app_status(&app.id).await {
                                 Ok(s) => format!("{:?}", s),
                                 Err(_) => "Unknown".to_string(),
                             };
                             output.push_str(&format!("{:<15} {:<15} {}\n", app.id, app.name, status));
                         }
                         Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("Error listing apps: {}", e), 1))
                }
            },
            "info" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "app info: requires app id".to_string(), 1));
                }
                match service.app_manager.get_app(args[1]).await {
                    Ok(app) => Ok((format!("{:#?}\n", app), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error getting app info: {}", e), 1))
                }
            },
            "install" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "app install: requires app config json".to_string(), 1));
                }
                let json_str = args[1];
                match serde_json::from_str::<domain::entities::app::App>(json_str) {
                    Ok(app) => {
                        match service.app_manager.install_app(app).await {
                             Ok(_) => Ok(("App installed successfully\n".to_string(), "".to_string(), 0)),
                             Err(e) => Ok(("".to_string(), format!("Error installing app: {}", e), 1))
                        }
                    },
                    Err(e) => Ok(("".to_string(), format!("Invalid app config JSON: {}", e), 1))
                }
            },
            "start" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "app start: requires app id".to_string(), 1));
                }
                match service.app_manager.start_app(args[1]).await {
                    Ok(_) => Ok((format!("App {} started\n", args[1]), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error starting app: {}", e), 1))
                }
            },
             "stop" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "app stop: requires app id".to_string(), 1));
                }
                match service.app_manager.stop_app(args[1]).await {
                    Ok(_) => Ok((format!("App {} stopped\n", args[1]), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error stopping app: {}", e), 1))
                }
            },
             "uninstall" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "app uninstall: requires app id".to_string(), 1));
                }
                match service.app_manager.uninstall_app(args[1]).await {
                    Ok(_) => Ok((format!("App {} uninstalled\n", args[1]), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error uninstalling app: {}", e), 1))
                }
            },
             "status" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "app status: requires app id".to_string(), 1));
                }
                match service.app_manager.get_app_status(args[1]).await {
                    Ok(status) => Ok((format!("{:?}\n", status), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Error getting app status: {}", e), 1))
                }
            },
            _ => Ok(("".to_string(), format!("app: unknown subcommand '{}'", args[0]), 1))
        }
    }
}
use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;
use domain::auth::{LoginReq, SignupReq, SecuritySettings};

pub struct AuthCommand;

#[async_trait]
impl Command for AuthCommand {
    fn name(&self) -> &str {
        "auth"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
             return Ok(("".to_string(), "auth: missing subcommand\nUsage: auth [login|signup|whoami|settings]".to_string(), 1));
        }

        match args[0] {
            "login" => {
                if args.len() < 3 {
                    return Ok(("".to_string(), "auth login: requires username and password".to_string(), 1));
                }
                let username = args[1];
                let password = args[2];
                let req = LoginReq {
                    username: username.to_string(),
                    password: password.to_string(),
                };
                
                match service.auth_service.login(req).await {
                    Ok(resp) => Ok((format!("Logged in as {}. Token: {}\n", resp.user_id, resp.token), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Login failed: {}", e), 1))
                }
            },
            "signup" => {
                 if args.len() < 3 {
                    return Ok(("".to_string(), "auth signup: requires username and password".to_string(), 1));
                }
                let username = args[1];
                let password = args[2];
                let req = SignupReq {
                    username: username.to_string(),
                    password: password.to_string(),
                };
                
                match service.auth_service.signup(req).await {
                    Ok(resp) => Ok((format!("User created with ID: {}\n", resp.user_id), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Signup failed: {}", e), 1))
                }
            },
            "whoami" => {
                 Ok((format!("{}\n", service.current_user), "".to_string(), 0))
            },
            "settings" => {
                 if args.len() < 2 {
                      return Ok(("".to_string(), "auth settings: requires subcommand [get|set]".to_string(), 1));
                 }
                 match args[1] {
                     "get" => {
                         match service.auth_service.get_security_settings(&service.current_user).await {
                             Ok(settings) => {
                                 Ok((format!("Security Settings for {}:\n{:?}\n", service.current_user, settings), "".to_string(), 0))
                             },
                             Err(e) => Ok(("".to_string(), format!("Error getting settings: {}", e), 1))
                         }
                     },
                     "set" => {
                         if args.len() < 4 {
                             return Ok(("".to_string(), "auth settings set: requires idle_timeout and idle_action".to_string(), 1));
                         }
                         let idle_timeout: i32 = match args[2].parse() {
                             Ok(t) => t,
                             Err(_) => return Ok(("".to_string(), "Invalid idle_timeout".to_string(), 1)),
                         };
                         let idle_action = args[3].to_string();
                         let settings = SecuritySettings {
                             idle_timeout,
                             idle_action,
                         };
                         match service.auth_service.set_security_settings(&service.current_user, settings).await {
                             Ok(_) => Ok(("Security settings updated\n".to_string(), "".to_string(), 0)),
                             Err(e) => Ok(("".to_string(), format!("Error setting settings: {}", e), 1))
                         }
                     },
                     _ => Ok(("".to_string(), format!("auth settings: unknown subcommand '{}'", args[1]), 1))
                 }
            },
            "wallpaper" => {
                 if args.len() < 2 {
                      return Ok(("".to_string(), "auth wallpaper: requires subcommand [get|set]".to_string(), 1));
                 }
                 match args[1] {
                     "get" => {
                          match service.auth_service.get_wallpaper(&service.current_user).await {
                              Ok(Some(path)) => Ok((format!("Wallpaper: {}\n", path), "".to_string(), 0)),
                              Ok(None) => Ok(("No wallpaper set\n".to_string(), "".to_string(), 0)),
                              Err(e) => Ok(("".to_string(), format!("Error getting wallpaper: {}", e), 1))
                          }
                     },
                     "set" => {
                         if args.len() < 3 {
                             return Ok(("".to_string(), "auth wallpaper set: requires path".to_string(), 1));
                         }
                         let path = args[2];
                         match service.auth_service.set_wallpaper(&service.current_user, path).await {
                             Ok(_) => Ok((format!("Wallpaper set to {}\n", path), "".to_string(), 0)),
                             Err(e) => Ok(("".to_string(), format!("Error setting wallpaper: {}", e), 1))
                         }
                     },
                     _ => Ok(("".to_string(), format!("auth wallpaper: unknown subcommand '{}'", args[1]), 1))
                 }
            },
            _ => Ok(("".to_string(), format!("auth: unknown subcommand '{}'", args[0]), 1))
        }
    }
}
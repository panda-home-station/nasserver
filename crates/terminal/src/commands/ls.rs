use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;
use domain::dtos::docs::DocsListQuery;

pub struct LsCommand;

#[async_trait]
impl Command for LsCommand {
    fn name(&self) -> &str {
        "ls"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str]) -> Result<(String, String, i32)> {
        let mut target = ".";
        let mut long_format = false;
        let mut one_per_line = false;
        
        for arg in args {
            if arg.starts_with('-') {
                if arg.contains('l') {
                    long_format = true;
                }
                if arg.contains('1') {
                    one_per_line = true;
                }
            } else {
                target = arg;
            }
        }
        
        let path = service.resolve_path(target);
        
        if path == "/" {
            if long_format || one_per_line {
                return Ok(("AppData\nUser\n".to_string(), "".to_string(), 0));
            } else {
                return Ok(("AppData  User\n".to_string(), "".to_string(), 0));
            }
        }
        
        if path == "/User" {
            if long_format || one_per_line {
                return Ok((format!("{}\n", service.current_user), "".to_string(), 0));
            } else {
                return Ok((format!("{}\n", service.current_user), "".to_string(), 0));
            }
        }
        
        let storage_path;
        let username;
        
        if path.starts_with("/AppData") {
            storage_path = Some(path.clone());
            username = service.current_user.clone();
        } else if path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = path.trim_start_matches(&format!("/User/{}", service.current_user));
            storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
            username = service.current_user.clone();
        } else {
            return Ok(("".to_string(), format!("ls: cannot access '{}': Permission denied", path), 1));
        }

        let query = DocsListQuery {
            path: storage_path,
            limit: Some(1000),
            offset: Some(0),
        };

        match service.storage_service.list(&username, query).await {
            Ok(resp) => {
                let mut output = String::new();
                let entries_names: Vec<String> = resp.entries.into_iter().map(|e| e.name).collect();
                
                if long_format || one_per_line {
                    for name in entries_names {
                        output.push_str(&name);
                        output.push('\n');
                    }
                } else {
                    // Join with spaces
                    if !entries_names.is_empty() {
                         output = entries_names.join("  ");
                         output.push('\n'); // Trailing newline
                    }
                }
                Ok((output, "".to_string(), 0))
            },
            Err(e) => Ok(("".to_string(), format!("ls: {}: {}", target, e), 1))
        }
    }
}

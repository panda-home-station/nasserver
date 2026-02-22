use std::sync::{Arc, Mutex};
use std::collections::HashMap;

use crate::error::Result;
use domain::storage::StorageService;
use domain::auth::AuthService;
use domain::system::SystemService;
use domain::dtos::docs::DocsListQuery;

use crate::commands::{
    Command, 
    ls::LsCommand, 
    cd::CdCommand,
    sysinfo::SysInfoCommand,
    fs::{CatCommand, MkdirCommand, RmCommand, MvCommand, TouchCommand, CpCommand},
};

#[derive(Clone)]
pub struct TerminalService {
    pub(crate) user_cwd: Arc<Mutex<String>>,
    pub(crate) current_user: String,
    pub(crate) storage_service: Arc<dyn StorageService>,
    pub(crate) _auth_service: Arc<dyn AuthService>,
    pub(crate) system_service: Arc<dyn SystemService>,
}

impl TerminalService {
    pub fn new(
        storage_service: Arc<dyn StorageService>,
        auth_service: Arc<dyn AuthService>,
        system_service: Arc<dyn SystemService>,
        current_user: String,
    ) -> Self {
        Self {
            user_cwd: Arc::new(Mutex::new(format!("/User/{}", current_user))),
            current_user,
            storage_service,
            _auth_service: auth_service,
            system_service,
        }
    }

    pub fn with_cwd(self, cwd: String) -> Self {
        *self.user_cwd.lock().unwrap() = cwd;
        self
    }

    pub fn with_cwd_ref(mut self, cwd: Arc<Mutex<String>>) -> Self {
        self.user_cwd = cwd;
        self
    }

    pub fn get_user_cwd(&self) -> String {
        self.user_cwd.lock().unwrap().clone()
    }

    pub(crate) fn resolve_path(&self, path: &str) -> String {
        let cwd = self.get_user_cwd();
        let path = path.trim();
        
        if path.is_empty() {
             return cwd;
        }

        let full_path = if path.starts_with('/') {
            path.to_string()
        } else if path == "~" {
            format!("/User/{}", self.current_user)
        } else if path.starts_with("~/") {
             format!("/User/{}/{}", self.current_user, path.trim_start_matches("~/"))
        } else {
            if cwd == "/" {
                format!("/{}", path)
            } else {
                format!("{}/{}", cwd, path)
            }
        };

        // Normalize path
        let mut parts = Vec::new();
        for part in full_path.split('/') {
            if part.is_empty() || part == "." {
                continue;
            }
            if part == ".." {
                parts.pop();
            } else {
                parts.push(part);
            }
        }
        
        let normalized = format!("/{}", parts.join("/"));
        if normalized.is_empty() {
            "/".to_string()
        } else {
            normalized
        }
    }

    pub async fn execute_script(&self, command: &str, _env_type: &str) -> Result<(String, String, i32)> {
        let command = command.trim().to_string();
        self.execute_user_command(&command).await
    }

    async fn execute_user_command(&self, command: &str) -> Result<(String, String, i32)> {
        let parts: Vec<&str> = command.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(("".to_string(), "".to_string(), 0));
        }

        let cmd = parts[0];
        let args = &parts[1..];

        match cmd {
            "cd" => {
                CdCommand.execute(self, args).await
            },
            "ls" => {
                LsCommand.execute(self, args).await
            },
            "cat" => {
                CatCommand.execute(self, args).await
            },
            "sysinfo" => {
                SysInfoCommand.execute(self, args).await
            },
            "mkdir" => {
                MkdirCommand.execute(self, args).await
            },
            "rm" => {
                RmCommand.execute(self, args).await
            },
            "mv" => {
                MvCommand.execute(self, args).await
            },
            "touch" => {
                TouchCommand.execute(self, args).await
            },
            "cp" => {
                CpCommand.execute(self, args).await
            },

            "pwd" => {
                let cwd = self.get_user_cwd();
                Ok((format!("{}\n", cwd), "".to_string(), 0))
            },
            "whoami" => {
                Ok((format!("{}\n", self.current_user), "".to_string(), 0))
            },

            _ => {
                Ok(("".to_string(), format!("{}: command not found", cmd), 127))
            }
        }
    }

    pub async fn complete(&self, line: &str) -> Vec<String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let ends_with_space = line.ends_with(' ');
        
        // If empty line, return all commands? Or maybe nothing. Let's return nothing for empty line.
        if line.trim().is_empty() {
             return vec![];
        }

        // Determine what we are completing: command or argument
        // If ends_with_space, we are starting a new argument
        let (prefix, is_command) = if ends_with_space {
            ("", false)
        } else {
            if parts.len() == 1 {
                (parts[0], true)
            } else {
                (*parts.last().unwrap_or(&""), false)
            }
        };

        if is_command {
            let cmds = vec!["ls", "cd", "mkdir", "rm", "touch", "cp", "mv", "cat", "pwd", "whoami", "sysinfo", "clear", "echo"];
            return cmds.into_iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| c.to_string())
                .collect();
        }

        // File/Directory completion
        // 1. Resolve parent directory and partial name
        let (parent, partial) = if prefix.ends_with('/') {
            (prefix, "")
        } else {
            match prefix.rfind('/') {
                Some(idx) => (&prefix[..idx+1], &prefix[idx+1..]),
                None => ("", prefix),
            }
        };

        // 2. Resolve parent to absolute virtual path
        let resolved_parent = if parent.is_empty() {
            self.resolve_path(".") // current directory
        } else {
            self.resolve_path(parent)
        };
        
        // 3. List directory contents
        let mut candidates = Vec::new();
        
        // Handle root/special paths listing logic similar to `ls`
        if resolved_parent == "/" {
            candidates.push("AppData".to_string());
            candidates.push("User".to_string());
        } else if resolved_parent == "/User" {
            candidates.push(self.current_user.clone());
        } else {
            // Map to storage path
            let storage_path;
            let username;
            
            let mut can_list = false;
            if resolved_parent.starts_with("/AppData") {
                storage_path = Some(resolved_parent.clone());
                username = self.current_user.clone();
                can_list = true;
            } else if resolved_parent.starts_with(&format!("/User/{}", self.current_user)) {
                let rel = resolved_parent.trim_start_matches(&format!("/User/{}", self.current_user));
                storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
                username = self.current_user.clone();
                can_list = true;
            } else {
                storage_path = None;
                username = "".to_string();
            }

            if can_list {
                let query = DocsListQuery {
                    path: storage_path,
                    limit: Some(1000),
                    offset: Some(0),
                };

                if let Ok(resp) = self.storage_service.list(&username, query).await {
                    for entry in resp.entries {
                        candidates.push(entry.name);
                    }
                }
            }
        }

        // 4. Filter by partial name and format output
        // We return the full string that should replace the current token
        candidates.into_iter()
            .filter(|name| name.starts_with(partial))
            .map(|name| {
                // If we found a match, construct the full path part to return
                // e.g. input "Do", match "Downloads" -> return "Downloads"
                // e.g. input "Us/Do", match "Downloads" -> return "Us/Downloads"
                if parent.is_empty() {
                    name
                } else {
                    format!("{}{}", parent, name)
                }
            })
            .collect()
    }
}

use std::sync::{Arc, Mutex};
use std::path::Path;
use crate::error::Result;
use domain::storage::StorageService;
use domain::auth::AuthService;
use domain::system::SystemService;
use domain::dtos::docs::{DocsListQuery, DocsMkdirReq, DocsRenameReq, DocsDeleteQuery};

use crate::commands::{Command, ls::LsCommand, cd::CdCommand};

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
            "touch" => {
                if args.is_empty() {
                    return Ok(("".to_string(), "touch: missing operand".to_string(), 1));
                }
                let target = args[0];
                let path = self.resolve_path(target);
                
                let storage_path;
                let username;

                if path.starts_with("/AppData") {
                    storage_path = path;
                    username = self.current_user.clone();
                } else if path.starts_with(&format!("/User/{}", self.current_user)) {
                    let rel = path.trim_start_matches(&format!("/User/{}", self.current_user));
                    storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                    username = self.current_user.clone();
                } else {
                    return Ok(("".to_string(), format!("touch: cannot touch '{}': Permission denied", target), 1));
                }

                // Split into parent and name
                let p = std::path::Path::new(&storage_path);
                let parent = p.parent().unwrap_or(Path::new("/")).to_str().unwrap_or("/");
                let name = p.file_name().unwrap_or_default().to_str().unwrap_or("");

                if name.is_empty() {
                     return Ok(("".to_string(), "touch: invalid path".to_string(), 1));
                }

                // Check if exists? storage_service.save_file overwrites. 
                // For touch, we want to create empty file if not exists.
                // If exists, ideally update timestamp, but we don't have API for that easily.
                // We'll just skip if exists, or overwrite? 'touch' usually updates timestamp.
                // If we overwrite with empty, we lose data. So we MUST check existence.
                
                let query = DocsListQuery {
                    path: Some(parent.to_string()),
                    limit: Some(1000),
                    offset: Some(0),
                };
                
                let exists = match self.storage_service.list(&username, query).await {
                    Ok(resp) => resp.entries.iter().any(|e| e.name == name),
                    Err(_) => false,
                };

                if exists {
                    // TODO: Update timestamp when API supports it
                    return Ok(("".to_string(), "".to_string(), 0));
                }

                match self.storage_service.save_file(&username, parent, name, bytes::Bytes::new()).await {
                    Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("touch: cannot create '{}': {}", target, e), 1))
                }
            },
            "cp" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "cp: missing operand".to_string(), 1));
                }
                let source = args[0];
                let dest = args[1];
                let source_path = self.resolve_path(source);
                let dest_path = self.resolve_path(dest);
                
                // Source check
                let s_storage_path;
                let s_username;
                if source_path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = source_path.trim_start_matches(&format!("/User/{}", self.current_user));
                     s_storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                     s_username = self.current_user.clone();
                } else if source_path.starts_with("/AppData") {
                     s_storage_path = source_path;
                     s_username = self.current_user.clone();
                } else {
                     return Ok(("".to_string(), format!("cp: cannot access '{}': Permission denied", source), 1));
                }

                // Dest check
                let d_storage_path;
                let d_username;
                if dest_path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = dest_path.trim_start_matches(&format!("/User/{}", self.current_user));
                     d_storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                     d_username = self.current_user.clone();
                } else if dest_path.starts_with("/AppData") {
                     d_storage_path = dest_path;
                     d_username = self.current_user.clone();
                } else {
                     return Ok(("".to_string(), format!("cp: cannot access '{}': Permission denied", dest), 1));
                }

                // Read source
                let data = match self.storage_service.get_file_path(&s_username, &s_storage_path).await {
                    Ok(physical_path) => {
                        match tokio::fs::read(&physical_path).await {
                            Ok(content) => bytes::Bytes::from(content),
                            Err(e) => return Ok(("".to_string(), format!("cp: cannot read '{}': {}", source, e), 1))
                        }
                    },
                    Err(e) => return Ok(("".to_string(), format!("cp: cannot find '{}': {}", source, e), 1))
                };

                // Write dest
                // Handle if dest is a directory? For now assume dest is full file path or simple name.
                // If dest ends with /, treat as directory and append source filename?
                // For simplicity, assume dest includes filename.
                
                let p = std::path::Path::new(&d_storage_path);
                let parent = p.parent().unwrap_or(Path::new("/")).to_str().unwrap_or("/");
                let name = p.file_name().unwrap_or_default().to_str().unwrap_or("");
                
                if name.is_empty() {
                     return Ok(("".to_string(), "cp: invalid destination".to_string(), 1));
                }

                match self.storage_service.save_file(&d_username, parent, name, data).await {
                    Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("cp: cannot create '{}': {}", dest, e), 1))
                }
            },
            "mkdir" => {
                 let mut target = "";
                 for arg in args {
                     if !arg.starts_with('-') {
                         target = arg;
                         break;
                     }
                 }
                 if target.is_empty() {
                     return Ok(("".to_string(), "mkdir: missing operand".to_string(), 1));
                 }
                 let path = self.resolve_path(target);
                 
                 let storage_path;
                 let username;

                 if path.starts_with("/AppData") {
                     storage_path = path;
                     username = self.current_user.clone();
                 } else if path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = path.trim_start_matches(&format!("/User/{}", self.current_user));
                     storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                     username = self.current_user.clone();
                 } else {
                     return Ok(("".to_string(), format!("mkdir: cannot create '{}': Permission denied", target), 1));
                 }

                 let req = DocsMkdirReq { path: storage_path };
                 match self.storage_service.mkdir(&username, req).await {
                     Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
                     Err(e) => Ok(("".to_string(), format!("mkdir: cannot create directory '{}': {}", target, e), 1))
                 }
            },
            "rm" => {
                let mut target = "";
                for arg in args {
                    if !arg.starts_with('-') {
                        target = arg;
                        break;
                    }
                }
                if target.is_empty() {
                    return Ok(("".to_string(), "rm: missing operand".to_string(), 1));
                }
                let path = self.resolve_path(target);

                 let storage_path;
                 let username;

                 if path.starts_with("/AppData") {
                     storage_path = Some(path);
                     username = self.current_user.clone();
                 } else if path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = path.trim_start_matches(&format!("/User/{}", self.current_user));
                     storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
                     username = self.current_user.clone();
                 } else {
                     return Ok(("".to_string(), format!("rm: cannot remove '{}': Permission denied", target), 1));
                 }
                
                let query = DocsDeleteQuery { path: storage_path };
                match self.storage_service.delete(&username, query).await {
                    Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("rm: cannot remove '{}': {}", target, e), 1))
                }
            },
            "cat" => {
                let mut target = "";
                for arg in args {
                    if !arg.starts_with('-') {
                        target = arg;
                        break;
                    }
                }
                if target.is_empty() {
                    return Ok(("".to_string(), "cat: missing operand".to_string(), 1));
                }
                let path = self.resolve_path(target);
                
                let storage_path;
                let username;

                 if path.starts_with("/AppData") {
                     storage_path = path;
                     username = self.current_user.clone();
                 } else if path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = path.trim_start_matches(&format!("/User/{}", self.current_user));
                     storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
                     username = self.current_user.clone();
                 } else {
                     return Ok(("".to_string(), format!("cat: cannot open '{}': Permission denied", target), 1));
                 }

                match self.storage_service.get_file_path(&username, &storage_path).await {
                    Ok(physical_path) => {
                        match tokio::fs::read_to_string(&physical_path).await {
                            Ok(content) => Ok((content, "".to_string(), 0)),
                            Err(e) => Ok(("".to_string(), format!("cat: {}: {}", target, e), 1))
                        }
                    },
                    Err(e) => Ok(("".to_string(), format!("cat: {}: {}", target, e), 1))
                }
            },
            "pwd" => {
                let cwd = self.get_user_cwd();
                Ok((format!("{}\n", cwd), "".to_string(), 0))
            },
            "whoami" => {
                Ok((format!("{}\n", self.current_user), "".to_string(), 0))
            },
            "mv" => {
                if args.len() < 2 {
                    return Ok(("".to_string(), "mv: missing operand".to_string(), 1));
                }
                let source = args[0];
                let dest = args[1];
                let source_path = self.resolve_path(source);
                let dest_path = self.resolve_path(dest);
                
                let s_storage_path;
                let s_username;
                if source_path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = source_path.trim_start_matches(&format!("/User/{}", self.current_user));
                     s_storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
                     s_username = self.current_user.clone();
                } else {
                     return Ok(("".to_string(), "mv: source must be in user home".to_string(), 1));
                }

                let d_storage_path;
                if dest_path.starts_with(&format!("/User/{}", self.current_user)) {
                     let rel = dest_path.trim_start_matches(&format!("/User/{}", self.current_user));
                     d_storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
                } else {
                     return Ok(("".to_string(), "mv: destination must be in user home".to_string(), 1));
                }

                let req = DocsRenameReq {
                    from: s_storage_path,
                    to: d_storage_path
                };

                match self.storage_service.rename(&s_username, req).await {
                    Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("mv: cannot move: {}", e), 1))
                }
            },
            "sysinfo" => {
                match self.system_service.get_current_stats().await {
                    Ok(stats) => {
                        let output = format!(
                            "CPU Usage: {:.1}%\nMemory Usage: {:.1}%\nDisk Usage: {:.1}%\n",
                            stats.cpu_usage, stats.memory_usage, stats.disk_usage
                        );
                        Ok((output, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("sysinfo: failed to get stats: {}", e), 1))
                }
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

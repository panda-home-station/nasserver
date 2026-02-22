use std::sync::{Arc, Mutex};

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
    echo::EchoCommand,
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

    pub fn get_available_commands() -> Vec<&'static str> {
        vec!["ls", "cd", "cat", "sysinfo", "mkdir", "rm", "mv", "touch", "cp", "pwd", "whoami", "echo", "help"]
    }

    pub fn get_help_text() -> String {
        format!("Available commands: {}\n", Self::get_available_commands().join(", "))
    }

    pub async fn execute_script(&self, command: &str, _env_type: &str) -> Result<(String, String, i32)> {
        let command = command.trim().to_string();
        self.execute_user_command(&command).await
    }

    async fn execute_user_command(&self, command: &str) -> Result<(String, String, i32)> {
        // 1. Pre-process for Heredoc (<<EOF)
        // We handle this manually because we need to preserve newlines for the content
        let mut processed_command = command.to_string();
        let mut initial_stdin = None;

        if let Some(idx) = processed_command.find("<<") {
            let after = &processed_command[idx+2..];
            // Find end of marker (whitespace or non-alphanumeric usually, but let's just say whitespace)
            let marker_end = after.find(|c: char| c.is_whitespace()).unwrap_or(after.len());
            let marker = &after[..marker_end];
            
            if !marker.is_empty() {
                // Find the first newline after the marker definition to start content extraction
                if let Some(nl_pos_rel) = processed_command[idx..].find('\n') {
                    let abs_nl_pos = idx + nl_pos_rel;
                    let content_start = abs_nl_pos + 1;
                    
                    if content_start < processed_command.len() {
                        let rest = &processed_command[content_start..];
                        let mut content = String::new();
                        let mut found_end = false;
                        
                        for line in rest.lines() {
                            if line.trim() == marker {
                                found_end = true;
                                break;
                            }
                            content.push_str(line);
                            content.push('\n');
                        }
                        
                        if found_end {
                            initial_stdin = Some(content);
                            
                            // Reconstruct command string:
                            // 1. Keep everything before `<<`
                            // 2. Keep everything after `<<MARKER` but before the newline (if any args there)
                            // 3. Ignore the content lines
                            
                            // For simplicity, we assume the command line ends at the first newline
                            // and we just remove `<<MARKER` from it.
                            let line_end = abs_nl_pos;
                            let mut command_line = processed_command[..line_end].to_string();
                            
                            // Remove `<<MARKER` from command_line
                            // We need to be careful about indices in command_line vs processed_command
                            // `idx` is valid for command_line too since it's before line_end
                            if let Some(local_idx) = command_line.find("<<") {
                                // We expect local_idx to be equal to idx
                                // Remove `<<` + marker
                                let range_end = local_idx + 2 + marker.len();
                                if range_end <= command_line.len() {
                                    command_line.replace_range(local_idx..range_end, "");
                                }
                            }
                            
                            processed_command = command_line;
                        }
                    }
                }
            }
        }

        // 2. Tokenize with shlex
        let tokens = match shlex::split(&processed_command) {
            Some(t) => t,
            None => return Ok(("".to_string(), "Syntax error: unmatched quote".to_string(), 2)),
        };

        // 3. Build Pipeline
        let mut pipeline = Vec::new();
        let mut current_segment = Vec::new();
        
        for token in tokens {
            if token == "|" {
                if !current_segment.is_empty() {
                    pipeline.push(current_segment);
                    current_segment = Vec::new();
                }
            } else {
                current_segment.push(token);
            }
        }
        if !current_segment.is_empty() {
            pipeline.push(current_segment);
        }

        if pipeline.is_empty() {
             return Ok(("".to_string(), "".to_string(), 0));
        }

        // 4. Execute Pipeline
        let mut current_input = initial_stdin;
        let mut final_output = (String::new(), String::new(), 0);

        for segment in pipeline {
            let mut args = Vec::new();
            let mut redirect_target = None;
            let mut append_mode = false;
            
            let mut iter = segment.into_iter();
            while let Some(arg) = iter.next() {
                if arg == ">" {
                    if let Some(target) = iter.next() {
                        redirect_target = Some(target);
                        append_mode = false;
                    } else {
                        return Ok(("".to_string(), "Syntax error: missing file for redirection".to_string(), 2));
                    }
                } else if arg == ">>" {
                    if let Some(target) = iter.next() {
                        redirect_target = Some(target);
                        append_mode = true;
                    } else {
                        return Ok(("".to_string(), "Syntax error: missing file for redirection".to_string(), 2));
                    }
                } else {
                    args.push(arg);
                }
            }

            if args.is_empty() { continue; }

            let cmd_name = &args[0];
            let cmd_args: Vec<&str> = args.iter().skip(1).map(|s| s.as_str()).collect();

            let result = match cmd_name.as_str() {
                "cd" => CdCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "ls" => LsCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "cat" => CatCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "sysinfo" => SysInfoCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "mkdir" => MkdirCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "rm" => RmCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "mv" => MvCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "touch" => TouchCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "cp" => CpCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "pwd" => {
                    let cwd = self.get_user_cwd();
                    Ok((format!("{}\n", cwd), "".to_string(), 0))
                },
                "whoami" => {
                    Ok((format!("{}\n", self.current_user), "".to_string(), 0))
                },
                "echo" => EchoCommand.execute(self, &cmd_args, current_input.as_deref()).await,
                "help" => {
                    Ok((Self::get_help_text(), "".to_string(), 0))
                },
                _ => {
                    Ok(("".to_string(), format!("{}: command not found", cmd_name), 127))
                }
            };

            let (stdout, stderr, code) = result?;

            if code != 0 {
                return Ok((stdout, stderr, code));
            }

            // Handle Output
            if let Some(target) = redirect_target {
                let path = self.resolve_path(&target);
                
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
                     return Ok(("".to_string(), format!("Cannot write to '{}': Permission denied", target), 1));
                }

                let p = std::path::Path::new(&storage_path);
                let parent_path = p.parent().unwrap_or(std::path::Path::new("/")).to_string_lossy().to_string();
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                
                let mut content = bytes::Bytes::from(stdout.clone());
                
                if append_mode {
                    if let Ok(existing_path) = self.storage_service.get_file_path(&username, &storage_path).await {
                        if let Ok(existing_content) = tokio::fs::read(&existing_path).await {
                            let mut new_content = existing_content;
                            new_content.extend_from_slice(&content);
                            content = bytes::Bytes::from(new_content);
                        }
                    }
                }

                if let Err(e) = self.storage_service.save_file(&username, &parent_path, &name, content).await {
                     return Ok(("".to_string(), format!("Failed to write to '{}': {}", target, e), 1));
                }
                
                current_input = None;
                final_output = ("".to_string(), stderr, code);
            } else {
                current_input = Some(stdout.clone());
                final_output = (stdout, stderr, code);
            }
        }
        
        Ok(final_output)
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

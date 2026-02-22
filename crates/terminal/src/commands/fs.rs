use crate::service::TerminalService;
use crate::error::Result;
use crate::commands::Command;
use async_trait::async_trait;
use domain::dtos::docs::{DocsMkdirReq, DocsDeleteQuery, DocsRenameReq, DocsListQuery};
use std::path::Path;

pub struct CatCommand;

#[async_trait]
impl Command for CatCommand {
    fn name(&self) -> &str {
        "cat"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut targets = Vec::new();
        let mut parsing_options = true;

        for arg in args {
            if parsing_options {
                if *arg == "--" {
                    parsing_options = false;
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
            }
            targets.push(*arg);
        }

        if targets.is_empty() {
            if let Some(input) = stdin {
                return Ok((input.to_string(), "".to_string(), 0));
            }
            return Ok(("".to_string(), "cat: missing operand".to_string(), 1));
        }

        let target = targets[0];
        let path = service.resolve_path(target);
        
        let storage_path;
        let username;

        if path.starts_with("/AppData") {
            storage_path = path;
            username = service.current_user.clone();
        } else if path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = path.trim_start_matches(&format!("/User/{}", service.current_user));
            storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
            username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("cat: cannot open '{}': Permission denied", target), 1));
        }

        match service.storage_service.get_file_path(&username, &storage_path).await {
            Ok(p) => {
                match tokio::fs::read(&p).await {
                    Ok(bytes) => {
                        let content = String::from_utf8_lossy(&bytes).to_string();
                        Ok((content, "".to_string(), 0))
                    },
                    Err(e) => Ok(("".to_string(), format!("cat: {}: {}", target, e), 1))
                }
            },
            Err(e) => Ok(("".to_string(), format!("cat: {}: {}", target, e), 1))
        }
    }
}

pub struct MkdirCommand;

#[async_trait]
impl Command for MkdirCommand {
    fn name(&self) -> &str {
        "mkdir"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut targets = Vec::new();
        let mut parsing_options = true;

        for arg in args {
            if parsing_options {
                if *arg == "--" {
                    parsing_options = false;
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
            }
            targets.push(*arg);
        }

        if targets.is_empty() {
            return Ok(("".to_string(), "mkdir: missing operand".to_string(), 1));
        }

        let target = targets[0];
        let path = service.resolve_path(target);
        
        let storage_path;
        let username;

        if path.starts_with("/AppData") {
            storage_path = path;
            username = service.current_user.clone();
        } else if path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = path.trim_start_matches(&format!("/User/{}", service.current_user));
            storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
            username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("mkdir: cannot create directory '{}': Permission denied", target), 1));
        }
        
        // DocsMkdirReq expects parent and name
        let p = std::path::Path::new(&storage_path);
        let _parent = p.parent().unwrap_or(std::path::Path::new("/")).to_str().unwrap_or("/");
        let _name = p.file_name().unwrap_or_default().to_str().unwrap_or("");
        
        // The API actually uses full path for mkdir in recent versions if I recall correctly from my fix?
        // Wait, looking at my previous read of mkdir.rs:
        // let req = DocsMkdirReq { path: storage_path };
        // Yes, it uses path only.
        
        let req = DocsMkdirReq {
            path: storage_path,
        };

        match service.storage_service.mkdir(&username, req).await {
            Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
            Err(e) => Ok(("".to_string(), format!("mkdir: cannot create directory '{}': {}", target, e), 1))
        }
    }
}

pub struct RmCommand;

#[async_trait]
impl Command for RmCommand {
    fn name(&self) -> &str {
        "rm"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut targets = Vec::new();
        let mut parsing_options = true;

        for arg in args {
            if parsing_options {
                if *arg == "--" {
                    parsing_options = false;
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
            }
            targets.push(*arg);
        }

        if targets.is_empty() {
            return Ok(("".to_string(), "rm: missing operand".to_string(), 1));
        }

        let target = targets[0];
        let path = service.resolve_path(target);
        
        let storage_path;
        let username;

        if path.starts_with("/AppData") {
            storage_path = path;
            username = service.current_user.clone();
        } else if path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = path.trim_start_matches(&format!("/User/{}", service.current_user));
            storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
            username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("rm: cannot remove '{}': Permission denied", target), 1));
        }
        
        let query = DocsDeleteQuery {
            path: Some(storage_path),
        };

        match service.storage_service.delete(&username, query).await {
            Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
            Err(e) => Ok(("".to_string(), format!("rm: cannot remove '{}': {}", target, e), 1))
        }
    }
}

pub struct MvCommand;

#[async_trait]
impl Command for MvCommand {
    fn name(&self) -> &str {
        "mv"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut targets = Vec::new();
        let mut parsing_options = true;

        for arg in args {
            if parsing_options {
                if *arg == "--" {
                    parsing_options = false;
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
            }
            targets.push(*arg);
        }

        if targets.len() < 2 {
            return Ok(("".to_string(), "mv: missing operand".to_string(), 1));
        }

        let source = targets[0];
        let dest = targets[1];
        
        // Resolve source
        let source_path = service.resolve_path(source);
        let s_storage_path;
        let s_username;
        if source_path.starts_with("/AppData") {
            s_storage_path = Some(source_path);
            s_username = service.current_user.clone();
        } else if source_path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = source_path.trim_start_matches(&format!("/User/{}", service.current_user));
            s_storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
            s_username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("mv: cannot access '{}': Permission denied", source), 1));
        }

        // Resolve dest
        let dest_path = service.resolve_path(dest);
        let d_storage_path;
        let d_username;
        if dest_path.starts_with("/AppData") {
            d_storage_path = Some(dest_path);
            d_username = service.current_user.clone();
        } else if dest_path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = dest_path.trim_start_matches(&format!("/User/{}", service.current_user));
            d_storage_path = Some(if rel.is_empty() { "/".to_string() } else { rel.to_string() });
            d_username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("mv: cannot access '{}': Permission denied", dest), 1));
        }

        if s_username != d_username {
            return Ok(("".to_string(), "mv: cannot move between different users".to_string(), 1));
        }

        let req = DocsRenameReq {
            from: s_storage_path,
            to: d_storage_path,
        };

        match service.storage_service.rename(&s_username, req).await {
            Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
            Err(e) => Ok(("".to_string(), format!("mv: cannot move '{}' to '{}': {}", source, dest, e), 1))
        }
    }
}

pub struct TouchCommand;

#[async_trait]
impl Command for TouchCommand {
    fn name(&self) -> &str {
        "touch"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut targets = Vec::new();
        let mut parsing_options = true;

        for arg in args {
            if parsing_options {
                if *arg == "--" {
                    parsing_options = false;
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
            }
            targets.push(*arg);
        }

        if targets.is_empty() {
            return Ok(("".to_string(), "touch: missing operand".to_string(), 1));
        }
        let target = targets[0];
        let path = service.resolve_path(target);
        
        let storage_path;
        let username;

        if path.starts_with("/AppData") {
            storage_path = path;
            username = service.current_user.clone();
        } else if path.starts_with(&format!("/User/{}", service.current_user)) {
            let rel = path.trim_start_matches(&format!("/User/{}", service.current_user));
            storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
            username = service.current_user.clone();
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
        
        let query = DocsListQuery {
            path: Some(parent.to_string()),
            limit: Some(1000),
            offset: Some(0),
        };
        
        let exists = match service.storage_service.list(&username, query).await {
            Ok(resp) => resp.entries.iter().any(|e| e.name == name),
            Err(_) => false,
        };

        if exists {
            return Ok(("".to_string(), "".to_string(), 0));
        }

        match service.storage_service.save_file(&username, parent, name, bytes::Bytes::new()).await {
            Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
            Err(e) => Ok(("".to_string(), format!("touch: cannot create '{}': {}", target, e), 1))
        }
    }
}

pub struct CpCommand;

#[async_trait]
impl Command for CpCommand {
    fn name(&self) -> &str {
        "cp"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let mut targets = Vec::new();
        let mut parsing_options = true;

        for arg in args {
            if parsing_options {
                if *arg == "--" {
                    parsing_options = false;
                    continue;
                }
                if arg.starts_with('-') {
                    continue;
                }
            }
            targets.push(*arg);
        }

        if targets.len() < 2 {
            return Ok(("".to_string(), "cp: missing operand".to_string(), 1));
        }
        let source = targets[0];
        let dest = targets[1];
        let source_path = service.resolve_path(source);
        let dest_path = service.resolve_path(dest);
        
        // Source check
        let s_storage_path;
        let s_username;
        if source_path.starts_with(&format!("/User/{}", service.current_user)) {
             let rel = source_path.trim_start_matches(&format!("/User/{}", service.current_user));
             s_storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
             s_username = service.current_user.clone();
        } else if source_path.starts_with("/AppData") {
             s_storage_path = source_path;
             s_username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("cp: cannot access '{}': Permission denied", source), 1));
        }

        // Dest check
        let d_storage_path;
        let d_username;
        if dest_path.starts_with(&format!("/User/{}", service.current_user)) {
             let rel = dest_path.trim_start_matches(&format!("/User/{}", service.current_user));
             d_storage_path = if rel.is_empty() { "/".to_string() } else { rel.to_string() };
             d_username = service.current_user.clone();
        } else if dest_path.starts_with("/AppData") {
             d_storage_path = dest_path;
             d_username = service.current_user.clone();
        } else {
             return Ok(("".to_string(), format!("cp: cannot access '{}': Permission denied", dest), 1));
        }

        // Read source
        let data = match service.storage_service.get_file_path(&s_username, &s_storage_path).await {
            Ok(physical_path) => {
                match tokio::fs::read(&physical_path).await {
                    Ok(content) => bytes::Bytes::from(content),
                    Err(e) => return Ok(("".to_string(), format!("cp: cannot read '{}': {}", source, e), 1))
                }
            },
            Err(e) => return Ok(("".to_string(), format!("cp: cannot find '{}': {}", source, e), 1))
        };

        // Write dest
        let p = std::path::Path::new(&d_storage_path);
        let parent = p.parent().unwrap_or(Path::new("/")).to_str().unwrap_or("/");
        let name = p.file_name().unwrap_or_default().to_str().unwrap_or("");
        
        if name.is_empty() {
             return Ok(("".to_string(), "cp: invalid destination".to_string(), 1));
        }

        match service.storage_service.save_file(&d_username, parent, name, data).await {
            Ok(_) => Ok(("".to_string(), "".to_string(), 0)),
            Err(e) => Ok(("".to_string(), format!("cp: cannot create '{}': {}", dest, e), 1))
        }
    }
}

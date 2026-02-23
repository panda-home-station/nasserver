use crate::service::TerminalService;
use crate::error::Result;
use super::Command;
use async_trait::async_trait;

pub struct CdCommand;

#[async_trait]
impl Command for CdCommand {
    fn name(&self) -> &str {
        "cd"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        let targets: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).cloned().collect();
        let default = "~";
        let target = targets.first().unwrap_or(&default);
        let path = service.resolve_path(target);
        
        if path == "/" || path == "/AppData" || path == "/User" {
            *service.user_cwd.lock().unwrap() = path;
            return Ok(("".to_string(), "".to_string(), 0));
        }
        
        if path.starts_with("/User/") {
                let parts: Vec<&str> = path.split('/').filter(|x| !x.is_empty()).collect();
                if parts.len() == 2 {
                    *service.user_cwd.lock().unwrap() = path;
                    return Ok(("".to_string(), "".to_string(), 0));
                }
        }

        *service.user_cwd.lock().unwrap() = path;
        Ok(("".to_string(), "".to_string(), 0))
    }
}

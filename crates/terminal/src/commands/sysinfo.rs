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

    async fn execute(&self, service: &TerminalService, _args: &[&str]) -> Result<(String, String, i32)> {
        match service.system_service.get_current_stats().await {
            Ok(stats) => {
                match serde_json::to_string_pretty(&stats) {
                    Ok(s) => Ok((s, "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("sysinfo: serialization error: {}", e), 1))
                }
            },
            Err(e) => Ok(("".to_string(), format!("sysinfo: failed to get stats: {}", e), 1))
        }
    }
}

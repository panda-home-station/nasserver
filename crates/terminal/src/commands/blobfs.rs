use crate::service::TerminalService;
use crate::error::Result;
use crate::commands::Command;
use async_trait::async_trait;

pub struct BlobFsCommand;

#[async_trait]
impl Command for BlobFsCommand {
    fn name(&self) -> &str {
        "blobfs"
    }

    async fn execute(&self, service: &TerminalService, args: &[&str], _stdin: Option<&str>) -> Result<(String, String, i32)> {
        if args.is_empty() {
            return Ok(("".to_string(), "Usage: blobfs <command> [args]\nCommands:\n  mount - Mount BlobFs for current user".to_string(), 1));
        }

        match args[0] {
            "mount" => {
                match service.blobfs_service.mount_for_user(&service.current_user).await {
                    Ok(_) => Ok((format!("BlobFs mounted for user {}\n", service.current_user), "".to_string(), 0)),
                    Err(e) => Ok(("".to_string(), format!("Failed to mount BlobFs: {}\n", e), 1))
                }
            },
            _ => Ok(("".to_string(), format!("Unknown command: {}\n", args[0]), 1))
        }
    }
}
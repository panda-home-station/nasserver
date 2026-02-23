use crate::service::TerminalService;
use crate::error::Result;
use async_trait::async_trait;

pub mod fs;
pub mod ls;
pub mod cd;
pub mod sysinfo;
pub mod echo;
pub mod container;
pub mod app;
pub mod download;
pub mod task;
pub mod auth;
pub mod blobfs;
pub mod agent;

#[async_trait]
pub trait Command: Send + Sync {
    /// The name of the command (e.g., "ls", "cd")
    fn name(&self) -> &str;
    
    /// Execute the command
    async fn execute(&self, service: &TerminalService, args: &[&str], stdin: Option<&str>) -> Result<(String, String, i32)>;
}

use std::process::Command;
use crate::traits::{AgentError, Result};

pub struct DockerManager;

impl DockerManager {
    /// Ensures a Docker container exists for the given user.
    /// 
    /// # Arguments
    /// * `user_id` - The unique identifier for the user (e.g., UUID or username).
    /// * `user_data_dir` - The absolute path to the user's data directory on the Host (e.g., /mnt/blobfs/users/101).
    /// 
    /// # Returns
    /// * `Result<String>` - The container name/ID to use for execution.
    pub fn ensure_user_container(user_id: &str, user_data_dir: &str) -> Result<String> {
        let container_name = format!("nas-workspace-{}", user_id);
        
        // 1. Check if container exists and is running
        let status = Command::new("docker")
            .args(&["inspect", "-f", "{{.State.Running}}", &container_name])
            .output();

        match status {
            Ok(output) => {
                if output.status.success() {
                    let output_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if output_str == "true" {
                        return Ok(container_name);
                    } else {
                        // Container exists but stopped, start it
                        let _ = Command::new("docker").args(&["start", &container_name]).output();
                        return Ok(container_name);
                    }
                }
                // If inspect failed, assume container doesn't exist, proceed to create
            },
            Err(_) => {
                // Docker command failed (daemon down?), return error
                return Err(AgentError::SandboxError("Failed to contact Docker daemon".to_string()));
            }
        }

        // 2. Create and start the container
        // We use a "fat" image that has common tools. For now, use 'ubuntu:latest' with 'tail -f /dev/null' to keep it alive.
        // In production, this should be a custom image with python, node, etc.
        // Mount: Host user_data_dir -> Container /workspace
        let output = Command::new("docker")
            .args(&[
                "run", "-d",
                "--name", &container_name,
                "-v", &format!("{}:/workspace", user_data_dir),
                "-w", "/workspace", // Default working directory
                "ubuntu:latest", // TODO: Configurable image
                "tail", "-f", "/dev/null"
            ])
            .output()
            .map_err(|e| AgentError::SandboxError(format!("Failed to execute docker run: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgentError::SandboxError(format!("Failed to start user container: {}", stderr)));
        }

        Ok(container_name)
    }
}

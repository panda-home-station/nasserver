use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TerminalError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Execution error: {0}")]
    Execution(String),
}

pub type Result<T> = std::result::Result<T, TerminalError>;

/// Abstraction for environment where command runs (Host vs Docker)
pub trait Sandbox: Send + Sync {
    fn wrap_command(&self, cmd: &mut Command) -> std::io::Result<()>;
}

/// A simple No-Op sandbox that runs commands directly on host
pub struct NoopSandbox;

impl Sandbox for NoopSandbox {
    fn wrap_command(&self, _cmd: &mut Command) -> std::io::Result<()> {
        Ok(())
    }
}

/// Core Terminal Service
#[derive(Clone)]
pub struct TerminalService {
    host_cwd: Arc<Mutex<String>>,
    // We can allow injecting custom sandboxes
    user_sandbox: Arc<dyn Sandbox>,
    host_sandbox: Arc<dyn Sandbox>,
}

impl TerminalService {
    pub fn new(host_sandbox: Arc<dyn Sandbox>, user_sandbox: Arc<dyn Sandbox>) -> Self {
        Self {
            host_cwd: Arc::new(Mutex::new(std::env::current_dir().unwrap_or_default().to_string_lossy().to_string())),
            host_sandbox,
            user_sandbox,
        }
    }

    pub fn get_host_cwd(&self) -> String {
        self.host_cwd.lock().unwrap().clone()
    }

    /// Execute a command script
    pub async fn execute_script(&self, command: &str, env_type: &str) -> Result<(String, String, i32)> {
        let command = command.trim();

        // 1. Handle 'cd' (Host Only)
        if env_type == "host" && (command.starts_with("cd ") || command == "cd") {
             let target = if command == "cd" {
                "~"
            } else {
                command.strip_prefix("cd ").unwrap().trim()
            };

            let target_path = if target == "~" || target.starts_with("~/") {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
                if target == "~" {
                    home
                } else {
                    target.replace("~", &home)
                }
            } else {
                target.to_string()
            };

            // Check if directory exists before changing
             let new_path = match std::fs::canonicalize(&target_path) {
                Ok(p) => p.to_string_lossy().to_string(),
                Err(e) => return Ok(("".to_string(), format!("cd: {}: {}", target_path, e), 1)),
            };

            // Update state
            *self.host_cwd.lock().unwrap() = new_path;
            return Ok(("".to_string(), "".to_string(), 0));
        }

        // 2. Select Sandbox
        let sandbox = match env_type {
            "user" => &self.user_sandbox,
            _ => &self.host_sandbox,
        };

        // 3. Prepare Command
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);

        // Set CWD only for Host execution
        if env_type == "host" {
            let current_cwd = self.host_cwd.lock().unwrap().clone();
            cmd.current_dir(&current_cwd);
        }

        // 4. Wrap & Execute
        sandbox.wrap_command(&mut cmd)?;

        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.stdin(Stdio::null());

        match cmd.output() {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let code = output.status.code().unwrap_or(-1);
                Ok((stdout, stderr, code))
            },
            Err(e) => {
                 Ok(("".to_string(), format!("Failed to execute command: {}", e), -1))
            }
        }
    }
}

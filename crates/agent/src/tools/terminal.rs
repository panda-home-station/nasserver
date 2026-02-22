use crate::traits::{AgentError, Sandbox, Tool, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use terminal::TerminalService;

/// A tool that executes shell commands using the TerminalService
#[derive(Clone)]
pub struct TerminalTool {
    service: Arc<TerminalService>,
}

impl TerminalTool {
    pub fn new(service: Arc<TerminalService>) -> Self {
        Self { service }
    }

    pub fn get_host_cwd(&self) -> String {
        self.service.get_user_cwd()
    }

    /// Execute a command and return structured output
    pub async fn execute_script(&self, command: &str, env_type: &str) -> Result<(String, String, i32)> {
        self.service.execute_script(command, env_type)
            .await
            .map_err(|e| AgentError::ToolError(e.to_string()))
    }
}

#[async_trait]
impl Tool for TerminalTool {
    fn name(&self) -> &str {
        "terminal"
    }

    fn description(&self) -> &str {
        "Execute a shell command. \
        Use 'environment: host' for NAS file management, system checks (ls, cd, cp, mv). \
        Use 'environment: user' for running user scripts, python, npm, and untrusted code. \
        Supports stateful 'cd' for host environment."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute."
                },
                "environment": {
                    "type": "string",
                    "enum": ["host", "user"],
                    "description": "Where to execute the command. 'host' for NAS system, 'user' for docker environment.",
                    "default": "host"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, _default_sandbox: &dyn Sandbox) -> Result<String> {
        let command = args["command"].as_str().ok_or(AgentError::ToolError("Missing command".to_string()))?;
        let env_type = args["environment"].as_str().unwrap_or("host");
        
        let (stdout, stderr, code) = self.execute_script(command, env_type).await?;
        
        if code != 0 {
            Ok(format!("Command failed with code {}:\nSTDOUT:\n{}\nSTDERR:\n{}", code, stdout, stderr))
        } else {
            Ok(stdout)
        }
    }
}

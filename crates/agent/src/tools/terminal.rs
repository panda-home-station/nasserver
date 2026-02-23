use crate::traits::{AgentError, Sandbox, Tool, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use kernel::TerminalService;

/// A tool that executes shell commands using the TerminalService
#[derive(Clone)]
pub struct TerminalTool {
    service: Arc<TerminalService>,
    description: String,
}

impl TerminalTool {
    pub fn new(service: Arc<TerminalService>) -> Self {
        let commands = TerminalService::get_available_commands().join(", ");
        let description = format!(
            "Execute Linux shell commands. You are in a NAS terminal environment with standard Linux capabilities. \
             You can manage files, check system status, and perform administrative tasks. \
             Available commands include but are not limited to: {}. \
             Use 'help' to see the full list.",
            commands
        );
        Self { service, description }
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
        &self.description
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value, _default_sandbox: &dyn Sandbox) -> Result<String> {
        let command = args["command"].as_str().ok_or(AgentError::ToolError("Missing command".to_string()))?;
        
        let (stdout, stderr, code) = self.execute_script(command, "host").await?;
        
        if code != 0 {
            Ok(format!("Command failed with code {}:\nSTDOUT:\n{}\nSTDERR:\n{}", code, stdout, stderr))
        } else {
            let mut output = stdout;
            if !stderr.is_empty() {
                if !output.is_empty() {
                    output.push_str("\nSTDERR:\n");
                }
                output.push_str(&stderr);
            }
            
            if output.is_empty() {
                Ok("(Command executed successfully)".to_string())
            } else {
                Ok(output)
            }
        }
    }
}

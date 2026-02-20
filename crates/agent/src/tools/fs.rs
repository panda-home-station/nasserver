use crate::traits::{AgentError, Sandbox, Tool, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::process::Command;

#[derive(Clone)]
pub struct ListFilesTool;

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "fs_list"
    }

    fn description(&self) -> &str {
        "List files in a directory"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, sandbox: &dyn Sandbox) -> Result<String> {
        let path = args["path"].as_str().ok_or(AgentError::ToolError("Missing path".to_string()))?;
        
        // Security check: only allow /fs/user or /tmp for now
        // if !path.starts_with("/fs/user") && !path.starts_with("/tmp") {
        //      return Err(AgentError::ToolError("Access denied. Only /fs/user is allowed.".to_string()));
        // }

        let mut cmd = Command::new("ls");
        cmd.arg("-la").arg(path);

        sandbox.wrap_command(&mut cmd).map_err(|e| AgentError::SandboxError(e.to_string()))?;

        let output = cmd.output().map_err(|e| AgentError::ToolError(e.to_string()))?;
        
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(AgentError::ToolError(String::from_utf8_lossy(&output.stderr).to_string()))
        }
    }
}

#[derive(Clone)]
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "fs_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The file path to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value, sandbox: &dyn Sandbox) -> Result<String> {
        let path = args["path"].as_str().ok_or(AgentError::ToolError("Missing path".to_string()))?;

        let mut cmd = Command::new("cat");
        cmd.arg(path);

        sandbox.wrap_command(&mut cmd).map_err(|e| AgentError::SandboxError(e.to_string()))?;

        let output = cmd.output().map_err(|e| AgentError::ToolError(e.to_string()))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            Err(AgentError::ToolError(String::from_utf8_lossy(&output.stderr).to_string()))
        }
    }
}

use crate::traits::{AgentError, Sandbox, Tool, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use domain::system::SystemService;
use std::sync::Arc;

pub struct SystemInfoTool {
    service: Arc<dyn SystemService>,
}

impl SystemInfoTool {
    pub fn new(service: Arc<dyn SystemService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Tool for SystemInfoTool {
    fn name(&self) -> &str {
        "system_info"
    }

    fn description(&self) -> &str {
        "Get current system information (CPU, Memory, Disk)"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value, _sandbox: &dyn Sandbox) -> Result<String> {
        let stats = self.service.get_current_stats().await
            .map_err(|e| AgentError::ToolError(format!("Failed to get stats: {}", e)))?;
        
        serde_json::to_string_pretty(&stats)
            .map_err(|e| AgentError::ToolError(format!("Failed to serialize stats: {}", e)))
    }
}

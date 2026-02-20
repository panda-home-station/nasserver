use crate::traits::{
    Agent, AgentConfig, AgentError, AgentEvent, Message, Provider, Result, Role, Sandbox, Tool,
    ToolDefinition,
};
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{info, instrument};

#[derive(Clone)]
pub struct AgentRuntime {
    provider: Arc<dyn Provider>,
    tools: Arc<HashMap<String, Arc<dyn Tool>>>,
    sandbox: Arc<dyn Sandbox>,
    system_prompt: String,
}

impl AgentRuntime {
    pub fn new(
        provider: Arc<dyn Provider>,
        tools: Vec<Arc<dyn Tool>>,
        sandbox: Arc<dyn Sandbox>,
    ) -> Self {
        let mut tool_map = HashMap::new();
        for tool in tools {
            tool_map.insert(tool.name().to_string(), tool);
        }

        Self {
            provider,
            tools: Arc::new(tool_map),
            sandbox,
            system_prompt: "You are a helpful NAS assistant.".to_string(),
        }
    }

    pub fn with_system_prompt(mut self, prompt: String) -> Self {
        self.system_prompt = prompt;
        self
    }

    fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|t| ToolDefinition {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters(),
            })
            .collect()
    }

    async fn execute_tool(&self, name: &str, args: &str) -> Result<String> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| AgentError::ToolError(format!("Tool not found: {}", name)))?;

        let args_value: Value = serde_json::from_str(args)
            .map_err(|e| AgentError::ToolError(format!("Invalid JSON args: {}", e)))?;

        info!("Executing tool {} with args: {}", name, args);
        tool.execute(args_value, self.sandbox.as_ref()).await
    }
}

#[async_trait]
impl Agent for AgentRuntime {
    #[instrument(skip(self))]
    async fn chat(&self, _session_id: &str, input: &str, history: Vec<Message>, config: Option<AgentConfig>) -> Result<BoxStream<'static, Result<AgentEvent>>> {
        let (tx, rx) = mpsc::channel(100);
        
        let runtime = self.clone();
        let input = input.to_string();
        let mut messages = history;
        let config = config;

        tokio::spawn(async move {
            // Ensure system prompt is present
            if !runtime.system_prompt.is_empty() {
                // Only add if not already present at the start (simple check)
                let has_system = messages.first().map(|m| matches!(m.role, Role::System)).unwrap_or(false);
                if !has_system {
                     messages.insert(0, Message {
                        role: Role::System,
                        content: Some(runtime.system_prompt.clone()),
                        tool_calls: None,
                        tool_call_id: None,
                    });
                }
            }

            // Append user message
            messages.push(Message {
                role: Role::User,
                content: Some(input.clone()),
                tool_calls: None,
                tool_call_id: None,
            });

            let tool_defs = runtime.get_tool_definitions();
            let max_iterations = 10;
            let mut current_iteration = 0;

            loop {
                if current_iteration >= max_iterations {
                    let _ = tx.send(Err(AgentError::Unknown("Max iterations reached".to_string()))).await;
                    break;
                }
                current_iteration += 1;

                // Notify thinking (implied by loop start)
                
                let response = match runtime.provider.complete(&messages, &tool_defs, config.as_ref()).await {
                    Ok(resp) => resp,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                };

                // Append assistant message
                let assistant_msg = Message {
                    role: Role::Assistant,
                    content: response.content.clone(),
                    tool_calls: response.tool_calls.clone(),
                    tool_call_id: None,
                };
                messages.push(assistant_msg.clone());

                // Emit content if any
                if let Some(content) = &response.content {
                    if response.tool_calls.is_none() || response.tool_calls.as_ref().unwrap().is_empty() {
                        let _ = tx.send(Ok(AgentEvent::Answer(content.clone()))).await;
                    } else {
                         let _ = tx.send(Ok(AgentEvent::Thought(content.clone()))).await;
                    }
                }

                // Check if tool calls needed
                if let Some(tool_calls) = response.tool_calls {
                    if tool_calls.is_empty() {
                         if response.content.is_some() {
                            // Already emitted answer
                            break;
                        }
                    }

                    for tool_call in tool_calls {
                        // Emit ToolCall event
                        let _ = tx.send(Ok(AgentEvent::ToolCall(tool_call.clone()))).await;

                        let result = runtime
                            .execute_tool(&tool_call.function.name, &tool_call.function.arguments)
                            .await;

                        let content = match result {
                            Ok(s) => s,
                            Err(e) => format!("Error: {}", e),
                        };

                        // Emit ToolResult event
                        let _ = tx.send(Ok(AgentEvent::ToolResult {
                            id: tool_call.id.clone(),
                            result: content.clone(),
                        })).await;

                        messages.push(Message {
                            role: Role::Tool,
                            content: Some(content),
                            tool_calls: None,
                            tool_call_id: Some(tool_call.id),
                        });
                    }
                } else {
                    // No tool calls, return content
                    if response.content.is_some() {
                        // Already emitted answer
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)) as BoxStream<'static, Result<AgentEvent>>)
    }
}

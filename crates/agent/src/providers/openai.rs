use crate::traits::{AgentConfig, CompletionResponse, Message, Provider, Result, ToolCall, ToolDefinition};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use crate::utils::append_chat_log;

#[derive(Debug, Clone)]
pub struct OpenAIProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAIProvider {
    pub fn new(api_key: String, model: String) -> Self {
        let base_url = env::var("OPENAI_BASE_URL").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
        Self {
            client: Client::new(),
            api_key,
            base_url,
            model,
        }
    }

    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }
}

#[derive(Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAIToolDefinition>>,
}

#[derive(Serialize)]
struct OpenAIMessage {
    role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OpenAIToolDefinition {
    r#type: String,
    function: OpenAIFunctionDefinition,
}

#[derive(Serialize)]
struct OpenAIFunctionDefinition {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIToolCall {
    id: String,
    r#type: String,
    function: OpenAIFunctionCall,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Deserialize)]
struct OpenAIResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OpenAIChoice {
    message: OpenAIMessageResponse,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct OpenAIMessageResponse {
    role: String,
    content: Option<String>,
    reasoning: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn complete(
        &self,
        session_id: &str,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: Option<&AgentConfig>,
    ) -> Result<CompletionResponse> {
        let model = config
            .and_then(|c| c.model.clone())
            .unwrap_or_else(|| self.model.clone());

        let base_url = config
            .and_then(|c| c.endpoint.clone())
            .map(|endpoint| {
                if endpoint.ends_with("/v1") {
                    endpoint
                } else {
                    format!("{}/v1", endpoint.trim_end_matches('/'))
                }
            })
            .unwrap_or_else(|| self.base_url.clone());

        let openai_messages: Vec<OpenAIMessage> = messages
            .iter()
            .map(|m| OpenAIMessage {
                role: match m.role {
                    crate::traits::Role::User => "user".to_string(),
                    crate::traits::Role::Assistant => "assistant".to_string(),
                    crate::traits::Role::System => "system".to_string(),
                    crate::traits::Role::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
                tool_calls: m.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| OpenAIToolCall {
                            id: tc.id.clone(),
                            r#type: "function".to_string(),
                            function: OpenAIFunctionCall {
                                name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            },
                        })
                        .collect()
                }),
                tool_call_id: m.tool_call_id.clone(),
            })
            .collect();

        let openai_tools: Option<Vec<OpenAIToolDefinition>> = if tools.is_empty() {
            None
        } else {
            Some(
                tools
                    .iter()
                    .map(|t| OpenAIToolDefinition {
                        r#type: "function".to_string(),
                        function: OpenAIFunctionDefinition {
                            name: t.name.clone(),
                            description: t.description.clone(),
                            parameters: t.parameters.clone(),
                        },
                    })
                    .collect(),
            )
        };

        let request = OpenAIRequest {
            model,
            messages: openai_messages,
            tools: openai_tools,
        };

        if let Ok(json_req) = serde_json::to_string_pretty(&request) {
             append_chat_log(session_id, format!("API_REQUEST: {}", json_req));
        }

        let url = format!("{}/chat/completions", base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&request)
            .send()
            .await
            .map_err(|e| crate::traits::AgentError::ProviderError(e.to_string()))?;

        if !resp.status().is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            append_chat_log(session_id, format!("API_ERROR: {}", error_text));
            return Err(crate::traits::AgentError::ProviderError(format!(
                "OpenAI API error: {}",
                error_text
            )));
        }

        let response_text = resp
            .text()
            .await
            .map_err(|e| crate::traits::AgentError::ProviderError(e.to_string()))?;
        
        append_chat_log(session_id, format!("API_RESPONSE: {}", response_text));

        let response_body: OpenAIResponse = serde_json::from_str(&response_text)
            .map_err(|e| crate::traits::AgentError::ProviderError(e.to_string()))?;

        let choice = response_body
            .choices
            .first()
            .ok_or_else(|| crate::traits::AgentError::ProviderError("No choices returned".to_string()))?;

        let tool_calls = choice.message.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| ToolCall {
                    id: tc.id.clone(),
                    function: crate::traits::FunctionCall {
                        name: tc.function.name.clone(),
                        arguments: tc.function.arguments.clone(),
                    },
                })
                .collect()
        });

        // Handle reasoning models: If content is empty but reasoning is present, use reasoning as content
        let content = if let Some(reasoning) = &choice.message.reasoning {
            if choice.message.content.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                Some(format!("(Reasoning: {})\n", reasoning))
            } else {
                choice.message.content.clone()
            }
        } else {
            choice.message.content.clone()
        };

        Ok(CompletionResponse {
            content,
            tool_calls,
        })
    }
}

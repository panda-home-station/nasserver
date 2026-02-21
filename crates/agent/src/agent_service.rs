use domain::{Result, Error, agent::{AgentService, AgentTask, TaskRequest, TaskResponse, ChatRequest, TaskStep, ChatSession, ChatMessageEntity}, system::SystemService};
use async_trait::async_trait;
use axum::response::sse::Event;
use futures_util::stream::{BoxStream, StreamExt};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use crate::search::SearchService;
use crate::runtime::AgentRuntime;
use crate::providers::openai::OpenAIProvider;
use crate::tools::{ListFilesTool, ReadFileTool, SystemInfoTool, TerminalTool};
use crate::sandbox::NoopSandbox;
use crate::traits::{Agent, AgentConfig, AgentEvent, Tool, Sandbox};
use terminal::{TerminalService, NoopSandbox as TerminalNoopSandbox};

use sqlx::{Pool, Postgres};

pub struct AgentServiceImpl {
    tasks: Arc<Mutex<HashMap<String, AgentTask>>>,
    search_service: SearchService,
    db: Pool<Postgres>,
    agent: Arc<dyn Agent>,
    terminal: Arc<TerminalTool>,
}

impl AgentServiceImpl {
    pub fn new(db: Pool<Postgres>, system_service: Arc<dyn SystemService>) -> Self {
        // Initialize Agent
        // Default to Ollama local if not provided via request
        let api_key = std::env::var("PNAS_AGENT_OLLAMA_API_KEY").unwrap_or_else(|_| "sk-dummy".to_string());
        let model = std::env::var("PNAS_AGENT_OLLAMA_MODEL").unwrap_or_else(|_| "gpt-oss:20b".to_string());
        let endpoint = std::env::var("PNAS_AGENT_OLLAMA_ENDPOINT").unwrap_or_else(|_| "http://localhost:11434".to_string());
        
        // Ensure /v1 for OpenAI compatibility
        let base_url = if endpoint.ends_with("/v1") {
            endpoint.to_string()
        } else {
            format!("{}/v1", endpoint.trim_end_matches('/'))
        };

        let provider = Arc::new(OpenAIProvider::new(api_key, model).with_base_url(base_url));
        
        // Use NoopSandbox for now to be safe, can switch to DockerSandbox
        let sandbox: Arc<dyn Sandbox> = Arc::new(NoopSandbox); 
        
        // Terminal Service setup
        let terminal_sandbox = Arc::new(TerminalNoopSandbox);
        let terminal_service = Arc::new(TerminalService::new(terminal_sandbox.clone(), terminal_sandbox.clone()));
        let terminal = Arc::new(TerminalTool::new(terminal_service));

        let tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(ListFilesTool),
            Arc::new(ReadFileTool),
            Arc::new(SystemInfoTool::new(system_service)),
            terminal.clone(),
        ];
        
        let agent = Arc::new(AgentRuntime::new(provider, tools, sandbox));

        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            search_service: SearchService::new(),
            db,
            agent,
            terminal,
        }
    }
}

#[async_trait]
impl AgentService for AgentServiceImpl {
    async fn create_task(&self, req: TaskRequest) -> Result<TaskResponse> {
        let task_id = Uuid::new_v4().to_string();
        let task = AgentTask {
            id: task_id.clone(),
            query: req.query.clone(),
            status: "processing".to_string(),
            plan: Vec::new(),
            logs: vec!["Task initialized".to_string()],
        };

        {
            let mut tasks = self.tasks.lock().unwrap();
            tasks.insert(task_id.clone(), task);
        }

        // Start background processing (Mock for now, can use Agent later)
        let tasks_clone = self.tasks.clone();
        let task_id_clone = task_id.clone();
        let _query_clone = req.query.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            {
                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(t) = tasks.get_mut(&task_id_clone) {
                    t.logs.push("Planning...".to_string());
                    // Mock plan
                    t.plan = vec![
                        TaskStep {
                            id: "1".to_string(),
                            description: "Analyze Request".to_string(),
                            status: "completed".to_string(),
                            result: Some("Request analysis done".to_string()),
                            tool_calls: Vec::new(),
                        },
                    ];
                    t.status = "completed".to_string();
                }
            }
        });

        self.get_task(&task_id).await
    }

    async fn get_task(&self, task_id: &str) -> Result<TaskResponse> {
        let tasks = self.tasks.lock().unwrap();
        match tasks.get(task_id) {
            Some(task) => Ok(TaskResponse {
                task_id: task.id.clone(),
                status: task.status.clone(),
                plan: task.plan.clone(),
                logs: task.logs.clone(),
            }),
            None => Err(Error::NotFound("Task not found".to_string())),
        }
    }

    async fn chat(&self, req: ChatRequest) -> Result<BoxStream<'static, Result<Event>>> {
        let last_message = req.messages.last()
            .ok_or_else(|| Error::BadRequest("No messages provided".to_string()))?;

        if last_message.role != "user" {
             return Err(Error::BadRequest("Last message must be from user".to_string()));
        }

        let session_id = if let Some(sid) = &req.session_id {
            Uuid::parse_str(sid).map_err(|_| Error::BadRequest("Invalid session_id".to_string()))?
        } else {
            // New session
            let user_id_str = req.user_id.clone().ok_or_else(|| Error::BadRequest("user_id required for new session".to_string()))?;
            let user_id = Uuid::parse_str(&user_id_str).map_err(|_| Error::BadRequest("Invalid user_id".to_string()))?;
            let agent_id = req.agent_id.clone().unwrap_or_else(|| "default".to_string());
            let title = last_message.content.chars().take(20).collect::<String>();
            
            let session = self.create_session(user_id, agent_id, title).await?;
            session.id
        };

        // Save user message
        self.save_message(session_id, "user".to_string(), last_message.content.clone(), None).await?;
        
        // Fetch history
        let history_entities = self.get_session_messages(session_id).await?;
        let history: Vec<crate::traits::Message> = history_entities.into_iter().map(|e| {
            crate::traits::Message {
                role: match e.role.as_str() {
                    "user" => crate::traits::Role::User,
                    "assistant" => crate::traits::Role::Assistant,
                    "system" => crate::traits::Role::System,
                    "tool" => crate::traits::Role::Tool,
                    _ => crate::traits::Role::User,
                },
                content: Some(e.content),
                tool_calls: e.tool_calls.map(|v| serde_json::from_value(v).unwrap_or_default()),
                tool_call_id: None, // We don't store this yet in DB or need to parse from content?
                // Actually tool_role messages need tool_call_id.
                // Our DB schema `chat_messages` doesn't seem to have `tool_call_id`.
                // For now, ignore tool_call_id restoration or assume it's part of content/tool_calls.
            }
        }).collect();

        // Call Agent
        let config = Some(AgentConfig {
            model: req.model.clone(),
            endpoint: req.endpoint.clone(),
        });

        let stream = self.agent.chat(&session_id.to_string(), &last_message.content, history, config).await
            .map_err(|e| Error::Internal(e.to_string()))?;

        // Map AgentEvent to SSE Event
        let sse_stream = stream.map(|result| {
            match result {
                Ok(event) => {
                    match event {
                        AgentEvent::Thought(thought) => {
                             let data = serde_json::json!({ "type": "thought", "content": thought }).to_string();
                             Ok(Event::default().event("thought").data(data))
                        },
                        AgentEvent::ToolCall(call) => {
                             let data = serde_json::json!({ "type": "tool_call", "content": { "id": call.id, "function": call.function } }).to_string();
                             Ok(Event::default().event("tool_call").data(data))
                        },
                        AgentEvent::ToolResult { id, result } => {
                             let data = serde_json::json!({ "type": "tool_result", "content": { "id": id, "result": result } }).to_string();
                             Ok(Event::default().event("tool_result").data(data))
                        },
                        AgentEvent::Answer(answer) => {
                             let data = serde_json::json!({ "type": "answer", "content": answer }).to_string();
                             Ok(Event::default().event("message").data(data))
                        }
                    }
                },
                Err(e) => {
                     let data = serde_json::json!({ "type": "error", "content": e.to_string() }).to_string();
                     Ok(Event::default().event("error").data(data))
                }
            }
        });

        Ok(sse_stream.boxed())
    }

    async fn search(&self, query: &str) -> Result<serde_json::Value> {
        let results = self.search_service.search(query).await?;
        Ok(serde_json::to_value(results).unwrap_or(serde_json::json!([])))
    }

    async fn list_sessions(&self, user_id: Uuid) -> Result<Vec<ChatSession>> {
        let sessions = sqlx::query_as::<_, ChatSession>(
            "SELECT * FROM agent.chat_sessions WHERE user_id = $1 ORDER BY updated_at DESC"
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| Error::Database(e.to_string()))?;
        Ok(sessions)
    }

    async fn get_session_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessageEntity>> {
        let messages = sqlx::query_as::<_, ChatMessageEntity>(
            "SELECT * FROM agent.chat_messages WHERE session_id = $1 ORDER BY created_at ASC"
        )
        .bind(session_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| Error::Database(e.to_string()))?;
        Ok(messages)
    }

    async fn create_session(&self, user_id: Uuid, agent_id: String, title: String) -> Result<ChatSession> {
        let session = sqlx::query_as::<_, ChatSession>(
            "INSERT INTO agent.chat_sessions (id, user_id, agent_id, title) VALUES ($1, $2, $3, $4) RETURNING *"
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(agent_id)
        .bind(title)
        .fetch_one(&self.db)
        .await
        .map_err(|e| Error::Database(e.to_string()))?;
        Ok(session)
    }

    async fn save_message(&self, session_id: Uuid, role: String, content: String, tool_calls: Option<serde_json::Value>) -> Result<ChatMessageEntity> {
        let message = sqlx::query_as::<_, ChatMessageEntity>(
            "INSERT INTO agent.chat_messages (id, session_id, role, content, tool_calls) VALUES ($1, $2, $3, $4, $5) RETURNING *"
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(role)
        .bind(&content)
        .bind(tool_calls)
        .fetch_one(&self.db)
        .await
        .map_err(|e| Error::Database(e.to_string()))?;
        
        // Update session updated_at and last_message
        let _ = sqlx::query("UPDATE agent.chat_sessions SET updated_at = NOW(), last_message = $1 WHERE id = $2")
            .bind(&content)
            .bind(session_id)
            .execute(&self.db)
            .await;
            
        Ok(message)
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM agent.chat_sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.db)
            .await
            .map_err(|e| Error::Database(e.to_string()))?;
        Ok(())
    }

    async fn execute_command(&self, command: String) -> Result<serde_json::Value> {
        let (stdout, stderr, code) = self.terminal.execute_script(&command, "host").await.map_err(|e| Error::Internal(e.to_string()))?;
        let cwd = self.terminal.get_host_cwd();
        
        Ok(serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": code,
            "cwd": cwd
        }))
    }
}

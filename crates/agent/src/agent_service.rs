use domain::{Result, Error, agent::{AgentService, AgentTask, TaskRequest, TaskResponse, ChatRequest, TaskStep, ChatSession, ChatMessageEntity}};
use async_trait::async_trait;
use axum::response::sse::Event;
use futures_util::stream::{BoxStream, StreamExt};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use crate::search::SearchService;

use sqlx::{Pool, Postgres};

pub struct AgentServiceImpl {
    tasks: Arc<Mutex<HashMap<String, AgentTask>>>,
    client: Client,
    search_service: SearchService,
    db: Pool<Postgres>,
}

impl AgentServiceImpl {
    pub fn new(db: Pool<Postgres>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            client: Client::new(),
            search_service: SearchService::new(),
            db,
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

        // Start background processing
        let tasks_clone = self.tasks.clone();
        let task_id_clone = task_id.clone();
        let query_clone = req.query.clone();

        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            {
                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(t) = tasks.get_mut(&task_id_clone) {
                    t.logs.push("Planning...".to_string());
                    t.plan = vec![
                        TaskStep {
                            id: "1".to_string(),
                            description: "Search for movie".to_string(),
                            status: "pending".to_string(),
                            result: None,
                            tool_calls: Vec::new(),
                        },
                        TaskStep {
                            id: "2".to_string(),
                            description: "Find magnet link".to_string(),
                            status: "pending".to_string(),
                            result: None,
                            tool_calls: Vec::new(),
                        },
                        TaskStep {
                            id: "3".to_string(),
                            description: "Download".to_string(),
                            status: "pending".to_string(),
                            result: None,
                            tool_calls: Vec::new(),
                        },
                        TaskStep {
                            id: "4".to_string(),
                            description: "Archive".to_string(),
                            status: "pending".to_string(),
                            result: None,
                            tool_calls: Vec::new(),
                        },
                    ];
                }
            }

            tokio::time::sleep(Duration::from_secs(1)).await;
            {
                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(t) = tasks.get_mut(&task_id_clone) {
                    if let Some(step) = t.plan.get_mut(0) {
                        step.status = "running".to_string();
                    }
                    t.logs.push(format!("Executing step 1: {}", query_clone));
                }
            }

            tokio::time::sleep(Duration::from_secs(2)).await;
            {
                let mut tasks = tasks_clone.lock().unwrap();
                if let Some(t) = tasks.get_mut(&task_id_clone) {
                    if let Some(step) = t.plan.get_mut(0) {
                        step.status = "completed".to_string();
                        step.result = Some("Found results".to_string());
                    }
                    t.status = "completed".to_string();
                    t.logs.push("Task completed successfully".to_string());
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
        if req.messages.is_empty() {
            return Err(Error::BadRequest("Empty messages".to_string()));
        }

        let endpoint = req.endpoint.clone().or_else(|| std::env::var("PNAS_AGENT_OLLAMA_ENDPOINT").ok())
            .ok_or_else(|| Error::BadRequest("Ollama endpoint is not configured".to_string()))?;

        let model = req.model.clone().or_else(|| std::env::var("PNAS_AGENT_OLLAMA_MODEL").ok())
            .ok_or_else(|| Error::BadRequest("Model is not specified".to_string()))?;

        let url = format!("{}/api/chat", endpoint.trim_end_matches('/'));

        let payload = serde_json::json!({
            "model": model,
            "messages": req.messages.iter().map(|m| serde_json::json!({ "role": m.role, "content": m.content })).collect::<Vec<_>>(),
            "stream": true
        });

        let resp = self.client.post(&url).json(&payload).send().await
            .map_err(|e| Error::Internal(format!("Ollama unreachable: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body_text = resp.text().await.unwrap_or_default();
            return Err(Error::Internal(format!("Ollama error: status={}, body={}", status, body_text)));
        }

        let mut buffer = String::new();
        let stream = resp.bytes_stream().map(move |result| {
            match result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    buffer.push_str(&text);
                    let mut events = Vec::new();

                    while let Some(pos) = buffer.find('\n') {
                        let line = buffer.drain(..pos + 1).collect::<String>();
                        let line = line.trim();
                        if line.is_empty() { continue; }

                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                            if let Some(content) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
                                // 使用 JSON 封装内容，确保换行符等特殊字符不会丢失
                                let data = serde_json::json!({ "content": content }).to_string();
                                events.push(Ok(Event::default().data(data)));
                            }
                        }
                    }
                    events
                },
                Err(e) => vec![Err(Error::Internal(e.to_string()))],
            }
        })
        .flat_map(|events| futures_util::stream::iter(events))
        .boxed();

        Ok(stream)
    }

    async fn search(&self, query: &str) -> Result<serde_json::Value> {
        let results = self.search_service.search(query).await?;
        Ok(serde_json::to_value(results).unwrap_or(serde_json::json!([])))
    }

    async fn list_sessions(&self, user_id: Uuid) -> Result<Vec<ChatSession>> {
        let sessions = sqlx::query_as::<_, ChatSession>(
            r#"
            SELECT 
                s.id, 
                s.user_id, 
                s.agent_id, 
                s.title, 
                s.created_at, 
                s.updated_at,
                (SELECT content FROM agent.chat_messages WHERE session_id = s.id ORDER BY created_at DESC LIMIT 1)::TEXT as last_message
            FROM agent.chat_sessions s
            WHERE s.user_id = $1
            ORDER BY s.updated_at DESC
            "#
        )
        .bind(user_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            eprintln!("list_sessions error: {}", e);
            Error::Internal(e.to_string())
        })?;

        Ok(sessions)
    }

    async fn get_session_messages(&self, session_id: Uuid) -> Result<Vec<ChatMessageEntity>> {
        let messages = sqlx::query_as::<_, ChatMessageEntity>(
            r#"
            SELECT id, session_id, role, content, tool_calls, created_at
            FROM agent.chat_messages
            WHERE session_id = $1
            ORDER BY created_at ASC
            "#
        )
        .bind(session_id)
        .fetch_all(&self.db)
        .await
        .map_err(|e| {
            eprintln!("get_session_messages error: {}", e);
            Error::Internal(e.to_string())
        })?;

        Ok(messages)
    }

    async fn create_session(&self, user_id: Uuid, agent_id: String, title: String) -> Result<ChatSession> {
        let id = Uuid::new_v4();
        let session = sqlx::query_as::<_, ChatSession>(
            r#"
            INSERT INTO agent.chat_sessions (id, user_id, agent_id, title)
            VALUES ($1, $2, $3, $4)
            RETURNING id, user_id, agent_id, title, NULL::TEXT as last_message, created_at, updated_at
            "#
        )
        .bind(id)
        .bind(user_id)
        .bind(agent_id)
        .bind(title)
        .fetch_one(&self.db)
        .await
        .map_err(|e| {
            eprintln!("create_session error: {}", e);
            Error::Internal(e.to_string())
        })?;

        Ok(session)
    }

    async fn save_message(&self, session_id: Uuid, role: String, content: String, tool_calls: Option<serde_json::Value>) -> Result<ChatMessageEntity> {
        let mut tx = self.db.begin().await.map_err(|e| Error::Internal(e.to_string()))?;
        
        let message = sqlx::query_as::<_, ChatMessageEntity>(
            r#"
            INSERT INTO agent.chat_messages (id, session_id, role, content, tool_calls)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id, session_id, role, content, tool_calls, created_at
            "#
        )
        .bind(Uuid::new_v4())
        .bind(session_id)
        .bind(role)
        .bind(content)
        .bind(tool_calls)
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            eprintln!("save_message error: {}", e);
            Error::Internal(e.to_string())
        })?;

        // Update session's updated_at
        sqlx::query("UPDATE agent.chat_sessions SET updated_at = CURRENT_TIMESTAMP WHERE id = $1")
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| {
                eprintln!("update session error: {}", e);
                Error::Internal(e.to_string())
            })?;

        tx.commit().await.map_err(|e| {
            eprintln!("commit error: {}", e);
            Error::Internal(e.to_string())
        })?;

        Ok(message)
    }

    async fn delete_session(&self, session_id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM agent.chat_sessions WHERE id = $1")
            .bind(session_id)
            .execute(&self.db)
            .await
            .map_err(|e| Error::Internal(e.to_string()))?;
        Ok(())
    }
}

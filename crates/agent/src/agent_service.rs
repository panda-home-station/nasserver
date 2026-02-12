use domain::{Result, Error, agent::{AgentService, AgentTask, TaskRequest, TaskResponse, ChatRequest, TaskStep}};
use async_trait::async_trait;
use axum::response::sse::Event;
use futures_util::stream::{BoxStream, StreamExt};
use reqwest::Client;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub struct AgentServiceImpl {
    tasks: Arc<Mutex<HashMap<String, AgentTask>>>,
    client: Client,
}

impl AgentServiceImpl {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(HashMap::new())),
            client: Client::new(),
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

        let stream = resp.bytes_stream().map(|result| {
            match result {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    let mut events = Vec::new();
                    for line in text.lines() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                            if let Some(content) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
                                events.push(Ok(Event::default().data(content)));
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
}

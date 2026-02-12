use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event, sse::Sse, IntoResponse},
    Json,
};
use futures_util::StreamExt;
use reqwest::Client;
use std::convert::Infallible;
use std::time::Duration;
use uuid::Uuid;

use crate::models::agent::{AgentTask, ChatRequest, TaskRequest, TaskResponse, TaskStep};
use crate::state::AppState;

pub async fn create_task(
    State(st): State<AppState>,
    Json(req): Json<TaskRequest>,
) -> impl IntoResponse {
    let task_id = Uuid::new_v4().to_string();
    let task = AgentTask {
        id: task_id.clone(),
        query: req.query.clone(),
        status: "processing".to_string(),
        plan: Vec::new(),
        logs: vec!["Task initialized".to_string()],
    };

    {
        let mut tasks = st.agent_tasks.lock().unwrap();
        tasks.insert(task_id.clone(), task);
    }

    // Start background processing (Mock logic from Python)
    let st_clone = st.clone();
    let task_id_clone = task_id.clone();
    let query_clone = req.query.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        {
            let mut tasks = st_clone.agent_tasks.lock().unwrap();
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
            let mut tasks = st_clone.agent_tasks.lock().unwrap();
            if let Some(t) = tasks.get_mut(&task_id_clone) {
                if let Some(step) = t.plan.get_mut(0) {
                    step.status = "running".to_string();
                }
                t.logs
                    .push(format!("Executing step 1: {}", query_clone));
            }
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
        {
            let mut tasks = st_clone.agent_tasks.lock().unwrap();
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

    let tasks = st.agent_tasks.lock().unwrap();
    let task = tasks.get(&task_id).unwrap();

    (
        StatusCode::OK,
        Json(TaskResponse {
            task_id: task.id.clone(),
            status: task.status.clone(),
            plan: task.plan.clone(),
            logs: task.logs.clone(),
        }),
    )
}

pub async fn get_task(
    State(st): State<AppState>,
    Path(task_id): Path<String>,
) -> impl IntoResponse {
    let tasks = st.agent_tasks.lock().unwrap();
    match tasks.get(&task_id) {
        Some(task) => (
            StatusCode::OK,
            Json(serde_json::json!(TaskResponse {
                task_id: task.id.clone(),
                status: task.status.clone(),
                plan: task.plan.clone(),
                logs: task.logs.clone(),
            })),
        )
            .into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Task not found" })),
        )
            .into_response(),
    }
}

pub async fn chat(
    State(_st): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    println!("[Agent] Received chat request: messages={}, model={:?}", req.messages.len(), req.model);
    
    if req.messages.is_empty() {
        println!("[Agent] Error: Empty messages");
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "empty_messages" })),
        )
            .into_response();
    }

    let endpoint = match req.endpoint.clone() {
        Some(e) => e,
        None => {
            match std::env::var("PNAS_AGENT_OLLAMA_ENDPOINT") {
                Ok(v) => v,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "missing_endpoint", "detail": "Ollama endpoint is not configured" })),
                    )
                        .into_response();
                }
            }
        }
    };

    let model = match req.model.clone() {
        Some(m) => m,
        None => {
            match std::env::var("PNAS_AGENT_OLLAMA_MODEL") {
                Ok(v) => v,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({ "error": "missing_model", "detail": "Model is not specified" })),
                    )
                        .into_response();
                }
            }
        }
    };

    println!("[Agent] Forwarding to Ollama: endpoint={}, model={}", endpoint, model);

    let client = Client::new();
    let url = format!("{}/api/chat", endpoint.trim_end_matches('/'));

    let payload = serde_json::json!({
        "model": model,
        "messages": req
            .messages
            .iter()
            .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
            .collect::<Vec<_>>(),
        "stream": true
    });

    let resp = match client.post(&url).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            println!("[Agent] Ollama unreachable: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "ollama_unreachable",
                    "detail": e.to_string()
                })),
            )
                .into_response();
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        println!("[Agent] Ollama error: status={}, body={}", status, body_text);
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({
                "error": "ollama_error",
                "status": status.as_u16(),
                "body": body_text
            })),
        )
            .into_response();
    }

    let stream = resp.bytes_stream().map(|result| {
        match result {
            Ok(bytes) => {
                // Ollama returns a series of JSON objects, one per line or in a stream
                // We need to extract the content from each JSON chunk
                let text = String::from_utf8_lossy(&bytes);
                // Each line is a JSON object
                let mut events = Vec::new();
                for line in text.lines() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                        if let Some(content) = v.get("message").and_then(|m| m.get("content")).and_then(|c| c.as_str()) {
                            events.push(Ok(Event::default().data(content)));
                        }
                        if let Some(done) = v.get("done").and_then(|d| d.as_bool()) {
                            if done {
                                // Optionally send a special event or just end
                            }
                        }
                    }
                }
                events
            },
            Err(e) => vec![Err(e)],
        }
    }).flat_map(|events| futures_util::stream::iter(events))
    .map(|result| {
        match result {
            Ok(event) => Ok::<Event, Infallible>(event),
            Err(e) => {
                println!("[Agent] Stream error: {}", e);
                // On error, we just end the stream or send an error event
                Ok::<Event, Infallible>(Event::default().event("error").data(e.to_string()))
            }
        }
    });

    Sse::new(stream)
        .keep_alive(axum::response::sse::KeepAlive::default())
        .into_response()
}

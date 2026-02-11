use axum::{
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use reqwest::Client;

use crate::models::agent::ChatRequest;
use crate::state::AppState;

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
        );
    }

    let endpoint =
        std::env::var("PNAS_AGENT_OLLAMA_ENDPOINT").unwrap_or_else(|_| "http://192.168.1.178:11434".to_string());
    let model = req
        .model
        .clone()
        .or_else(|| std::env::var("PNAS_AGENT_OLLAMA_MODEL").ok())
        .unwrap_or_else(|| "qwen3:8b".to_string());

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
        "stream": false
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
            );
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
        );
    }

    let body: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            println!("[Agent] Invalid Ollama response: {}", e);
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": "invalid_ollama_response",
                    "detail": e.to_string()
                })),
            );
        }
    };

    let reply = body
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();

    println!("[Agent] Success. Reply length: {}", reply.len());

    (
        StatusCode::OK,
        Json(serde_json::json!({ "reply": reply })),
    )
}

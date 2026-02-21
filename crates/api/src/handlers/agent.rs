use axum::{
    extract::{Path, State},
    response::sse::Sse,
    Json,
    response::IntoResponse,
    Extension,
};
use domain::{agent::{ChatRequest, TaskRequest, TaskResponse, ChatSession, ChatMessageEntity}, auth::AuthUser};
use infra::AppState;
use crate::error::ApiResult;
use tokio_stream::StreamExt;
use uuid::Uuid;

pub async fn create_task(
    State(st): State<AppState>,
    Json(req): Json<TaskRequest>,
) -> ApiResult<Json<TaskResponse>> {
    let resp = st.agent_service.create_task(req).await?;
    Ok(Json(resp))
}

pub async fn get_task(
    State(st): State<AppState>,
    Path(task_id): Path<String>,
) -> ApiResult<Json<TaskResponse>> {
    let resp = st.agent_service.get_task(&task_id).await?;
    Ok(Json(resp))
}

#[derive(serde::Deserialize)]
pub struct SearchRequest {
    pub q: String,
}

pub async fn search(
    State(st): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let resp = st.agent_service.search(&req.q).await?;
    Ok(Json(resp))
}

pub async fn list_sessions(
    State(st): State<AppState>,
    Extension(user): Extension<AuthUser>,
) -> ApiResult<Json<Vec<ChatSession>>> {
    let sessions = st.agent_service.list_sessions(user.user_id).await?;
    Ok(Json(sessions))
}

pub async fn get_session_messages(
    State(st): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> ApiResult<Json<Vec<ChatMessageEntity>>> {
    let messages = st.agent_service.get_session_messages(session_id).await?;
    Ok(Json(messages))
}

#[derive(serde::Deserialize)]
pub struct CreateSessionRequest {
    pub agent_id: String,
    pub title: String,
}

pub async fn create_session(
    State(st): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(req): Json<CreateSessionRequest>,
) -> ApiResult<Json<ChatSession>> {
    let session = st.agent_service.create_session(user.user_id, req.agent_id, req.title).await?;
    Ok(Json(session))
}

pub async fn delete_session(
    State(st): State<AppState>,
    Path(session_id): Path<Uuid>,
) -> ApiResult<impl IntoResponse> {
    st.agent_service.delete_session(session_id).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize)]
pub struct SaveMessageRequest {
    pub role: String,
    pub content: String,
    pub tool_calls: Option<serde_json::Value>,
}

pub async fn save_message(
    State(st): State<AppState>,
    Path(session_id): Path<Uuid>,
    Json(req): Json<SaveMessageRequest>,
) -> ApiResult<Json<ChatMessageEntity>> {
    let msg = st.agent_service.save_message(session_id, req.role, req.content, req.tool_calls).await?;
    Ok(Json(msg))
}

#[derive(serde::Deserialize)]
pub struct ExecuteCommandRequest {
    pub command: String,
}

pub async fn execute_command(
    State(st): State<AppState>,
    Json(req): Json<ExecuteCommandRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let result = st.agent_service.execute_command(req.command).await?;
    Ok(Json(result))
}

pub async fn chat(
    State(st): State<AppState>,
    Extension(user): Extension<AuthUser>,
    Json(mut req): Json<ChatRequest>,
) -> ApiResult<impl IntoResponse> {
    // Inject user_id into request
    req.user_id = Some(user.user_id.to_string());

    let stream = st.agent_service.chat(req).await?;
    
    // Convert domain::Result<Event> stream to Result<Event, Infallible> for Sse
    let sse_stream = stream.map(|res: domain::Result<axum::response::sse::Event>| {
        match res {
            Ok(event) => Ok::<axum::response::sse::Event, std::convert::Infallible>(event),
            Err(e) => {
                // Send error as an event
                Ok::<axum::response::sse::Event, std::convert::Infallible>(axum::response::sse::Event::default().event("error").data(e.to_string()))
            }
        }
    });

    Ok(Sse::new(sse_stream).keep_alive(axum::response::sse::KeepAlive::default()))
}

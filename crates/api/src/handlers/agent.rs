use axum::{
    extract::{Path, State},
    response::sse::Sse,
    Json,
    response::IntoResponse,
};
use models::agent::{ChatRequest, TaskRequest, TaskResponse};
use infra::AppState;
use common::core::Result;
use tokio_stream::StreamExt;

pub async fn create_task(
    State(st): State<AppState>,
    Json(req): Json<TaskRequest>,
) -> Result<Json<TaskResponse>> {
    let resp = st.agent_service.create_task(req).await?;
    Ok(Json(resp))
}

pub async fn get_task(
    State(st): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskResponse>> {
    let resp = st.agent_service.get_task(&task_id).await?;
    Ok(Json(resp))
}

pub async fn chat(
    State(st): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<impl IntoResponse> {
    let stream = st.agent_service.chat(req).await?;
    
    // Convert Result<Event> stream to Result<Event, Infallible> for Sse
    let sse_stream = stream.map(|res: Result<axum::response::sse::Event>| {
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

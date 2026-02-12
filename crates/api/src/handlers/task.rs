use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
    http::StatusCode,
};
use infra::AppState;
use models::task::{CreateTaskReq, UpdateTaskReq};
use common::core::Result;

pub async fn list_tasks(State(state): State<AppState>) -> Result<impl IntoResponse> {
    let tasks = state.task_service.list_tasks().await?;
    Ok(Json(tasks))
}

pub async fn create_task(State(state): State<AppState>, Json(payload): Json<CreateTaskReq>) -> Result<impl IntoResponse> {
    state.task_service.create_task(payload).await?;
    Ok(StatusCode::OK)
}

pub async fn update_task(State(state): State<AppState>, Path(id): Path<String>, Json(payload): Json<UpdateTaskReq>) -> Result<impl IntoResponse> {
    state.task_service.update_task(id, payload).await?;
    Ok(StatusCode::OK)
}

pub async fn clear_completed_tasks(State(state): State<AppState>) -> Result<impl IntoResponse> {
    state.task_service.clear_completed_tasks().await?;
    Ok(StatusCode::OK)
}

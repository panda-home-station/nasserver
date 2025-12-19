use axum::{
    extract::{Path, State},
    response::{IntoResponse, Json},
    http::StatusCode,
};
use crate::state::AppState;
use crate::models::task::{FileTask, CreateTaskReq, UpdateTaskReq};

pub async fn list_tasks(State(state): State<AppState>) -> impl IntoResponse {
    let tasks = sqlx::query_as::<_, FileTask>("select * from file_tasks order by created_at asc")
        .fetch_all(&state.db)
        .await;

    match tasks {
        Ok(t) => Json(t).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn create_task(State(state): State<AppState>, Json(payload): Json<CreateTaskReq>) -> impl IntoResponse {
    let res = sqlx::query(
        "insert into file_tasks (id, type, name, dir, progress, status) values ($1, $2, $3, $4, $5, $6)"
    )
    .bind(&payload.id)
    .bind(&payload.task_type)
    .bind(&payload.name)
    .bind(&payload.dir)
    .bind(payload.progress)
    .bind(&payload.status)
    .execute(&state.db)
    .await;

    match res {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn update_task(State(state): State<AppState>, Path(id): Path<String>, Json(payload): Json<UpdateTaskReq>) -> impl IntoResponse {
    if let Some(p) = payload.progress {
         let _ = sqlx::query("update file_tasks set progress = $1, updated_at = now() where id = $2")
            .bind(p)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    
    if let Some(s) = &payload.status {
         let _ = sqlx::query("update file_tasks set status = $1, updated_at = now() where id = $2")
            .bind(s)
            .bind(&id)
            .execute(&state.db)
            .await;
    }
    
    StatusCode::OK.into_response()
}

pub async fn clear_completed_tasks(State(state): State<AppState>) -> impl IntoResponse {
    let res = sqlx::query("delete from file_tasks where status in ('done', 'error')")
        .execute(&state.db)
        .await;

    match res {
        Ok(_) => StatusCode::OK.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

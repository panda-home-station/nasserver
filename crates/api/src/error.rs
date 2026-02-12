use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use domain::Error;

pub struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        Self(err)
    }
}

impl From<std::io::Error> for ApiError {
    fn from(err: std::io::Error) -> Self {
        Self(Error::from(err))
    }
}

impl From<axum::extract::multipart::MultipartError> for ApiError {
    fn from(err: axum::extract::multipart::MultipartError) -> Self {
        Self(Error::Internal(err.to_string()))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self.0 {
            Error::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            Error::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Error::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            Error::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
            Error::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Error::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            Error::Database(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.clone()),
            Error::Io(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.clone()),
        };

        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

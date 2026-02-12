pub mod handlers;
pub mod middleware;
pub mod routes;
pub mod api;
pub mod error;

pub use error::{ApiError, ApiResult};
pub use routes::{api_app, static_app};

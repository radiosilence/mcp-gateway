//! Application error type with an axum `IntoResponse` impl.
//!
//! Errors log their full detail server-side but return terse messages to
//! clients — we never leak internals (or, critically, token material) in a
//! response body.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream error: {0:#}")]
    Upstream(anyhow::Error),
    #[error("unavailable: {0:#}")]
    Unavailable(anyhow::Error),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::Forbidden => StatusCode::FORBIDDEN,
            AppError::NotFound => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Log the real cause; hand the client only the status-appropriate label.
        if status.is_server_error() {
            tracing::error!(error = format!("{self:#}"), "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
        }
        let public = match &self {
            AppError::Unauthorized => "unauthorized".to_string(),
            AppError::Forbidden => "forbidden".to_string(),
            AppError::NotFound => "not found".to_string(),
            AppError::BadRequest(m) => m.clone(),
            AppError::Upstream(_) => "upstream error".to_string(),
            AppError::Unavailable(_) => "temporarily unavailable".to_string(),
            AppError::Internal(_) => "internal error".to_string(),
        };
        (status, public).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

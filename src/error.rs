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
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::Unauthorized => StatusCode::UNAUTHORIZED,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
            AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        // Log the real cause; hand the client only the status-appropriate label.
        if status.is_server_error() {
            tracing::error!(error = %self, "request failed");
        } else {
            tracing::debug!(error = %self, "request rejected");
        }
        let public = match &self {
            AppError::Unauthorized => "unauthorized".to_string(),
            AppError::BadRequest(m) => m.clone(),
            AppError::Upstream(_) => "upstream error".to_string(),
            AppError::Internal(_) => "internal error".to_string(),
        };
        (status, public).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;

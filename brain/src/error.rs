use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub code: Option<String>,
}

impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            message: message.into(),
            code: None,
        }
    }
    pub fn malformed() -> Self {
        Self::new(400, "Malformed input")
    }
    pub fn provider() -> Self {
        Self::new(
            502,
            "Required service unavailable; retry with the same request",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({"error": self.message}))).into_response()
    }
}
impl From<reqwest::Error> for ApiError {
    fn from(_: reqwest::Error) -> Self {
        Self::provider()
    }
}
impl From<serde_json::Error> for ApiError {
    fn from(_: serde_json::Error) -> Self {
        Self::malformed()
    }
}
pub type Result<T> = std::result::Result<T, ApiError>;

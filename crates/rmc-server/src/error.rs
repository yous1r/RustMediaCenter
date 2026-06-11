use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Internal Server Error: {0}")]
    Internal(#[from] anyhow::Error),

    #[error("Not Found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "Not Found"),
            AppError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "Unauthorized"),
        };

        let body = Json(json!({
            "error": error_message,
            "detail": self.to_string(),
        }));

        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    #[test]
    fn test_app_error_response() {
        let err = AppError::Internal(anyhow::anyhow!("test error"));
        let res = err.into_response();
        assert_eq!(res.status(), 500);
    }

    #[test]
    fn test_unauthorized_error_response() {
        let err = AppError::Unauthorized("missing token".to_string());
        let res = err.into_response();
        assert_eq!(res.status(), 401);
    }
}

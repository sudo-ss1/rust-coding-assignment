use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum TwoFactorError {
    #[error("invalid challenge")]
    InvalidChallenge,
    #[error("challenge expired")]
    Expired,
    #[error("challenge already used")]
    AlreadyUsed,
    #[error("invalid code")]
    InvalidCode,
}

impl TwoFactorError {
    fn code(&self) -> &'static str {
        match self {
            Self::InvalidChallenge => "invalid_challenge",
            Self::Expired => "challenge_expired",
            Self::AlreadyUsed => "challenge_already_used",
            Self::InvalidCode => "invalid_code",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Validation(String),
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error(transparent)]
    TwoFactor(#[from] TwoFactorError),
    #[error("authentication required")]
    Unauthenticated,
    #[error("forbidden")]
    Forbidden,
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    Conflict(String),
    #[error("too many attempts")]
    TooManyAttempts,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Cache(#[from] redis::RedisError),
    #[error("{0}")]
    Internal(String),
}

pub type AppResult<T> = Result<T, AppError>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::Validation(m) => (StatusCode::BAD_REQUEST, "validation_error", m.clone()),
            AppError::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "invalid_credentials",
                "invalid credentials".to_string(),
            ),
            AppError::TwoFactor(e) => (StatusCode::UNAUTHORIZED, e.code(), e.to_string()),
            AppError::Unauthenticated => (
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "authentication required".to_string(),
            ),
            AppError::Forbidden => {
                (StatusCode::FORBIDDEN, "forbidden", "forbidden".to_string())
            }
            AppError::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found".to_string()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m.clone()),
            AppError::TooManyAttempts => (
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_attempts",
                "too many attempts".to_string(),
            ),
            AppError::Database(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error".to_string(),
                )
            }
            AppError::Cache(e) => {
                tracing::error!(error = %e, "cache error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error".to_string(),
                )
            }
            AppError::Internal(m) => {
                tracing::error!(error = %m, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": { "code": code, "message": message } }))).into_response()
    }
}

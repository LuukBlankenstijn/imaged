use serde::{Deserialize, Serialize};

pub type Result<T = ()> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error, Serialize, Deserialize)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("already exists: {0}")]
    AlreadyExists(String),

    #[error("failed precondition: {0}")]
    FailedPrecondition(String),

    #[error("internal: {0}")]
    Internal(String),

    #[error("database: {0}")]
    Database(String),
}

#[cfg(feature = "serverfn")]
impl From<dioxus_fullstack::ServerFnError> for AppError {
    fn from(e: dioxus_fullstack::ServerFnError) -> Self {
        AppError::Internal(e.to_string())
    }
}

#[cfg(feature = "serverfn")]
impl dioxus_fullstack::AsStatusCode for AppError {
    fn as_status_code(&self) -> dioxus_fullstack::http::StatusCode {
        use dioxus_fullstack::http::StatusCode;
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::InvalidArgument(_) | AppError::AlreadyExists(_) => StatusCode::BAD_REQUEST,
            AppError::FailedPrecondition(_) => StatusCode::PRECONDITION_FAILED,
            AppError::Internal(_) | AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[cfg(feature = "axum-error")]
impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, error_message) = match self {
            AppError::NotFound(msg) => (axum::http::StatusCode::NOT_FOUND, msg),
            AppError::InvalidArgument(msg) => (axum::http::StatusCode::BAD_REQUEST, msg),
            AppError::FailedPrecondition(msg) => (axum::http::StatusCode::PRECONDITION_FAILED, msg),
            AppError::Internal(msg) => {
                tracing::error!("Internal server error: {}", msg);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "An internal server error occurred".to_string(),
                )
            }
            AppError::AlreadyExists(msg) => (axum::http::StatusCode::BAD_REQUEST, msg),
            AppError::Database(msg) => {
                tracing::error!("Database error: {}", msg);
                (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    "An internal server error occurred".to_string(),
                )
            }
        };

        let body = axum::Json(serde_json::json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}

#[cfg(feature = "sqlx-error")]
impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        match value {
            sqlx::Error::RowNotFound => Self::NotFound("record not found".into()),
            sqlx::Error::ColumnNotFound(name) => {
                Self::Internal(format!("column not found: {}", name))
            }
            sqlx::Error::ColumnIndexOutOfBounds { index, len } => Self::Internal(format!(
                "column index out of bounds: {} (len: {})",
                index, len
            )),
            sqlx::Error::PoolTimedOut => {
                Self::Database("database pool timed out - server under heavy load".into())
            }
            sqlx::Error::PoolClosed => Self::Internal("database pool was closed".into()),
            sqlx::Error::Database(db_err) => {
                if let Some(code) = db_err.code() {
                    if code == "23505" {
                        return Self::AlreadyExists(db_err.message().into());
                    }
                    if code == "23503" {
                        return Self::FailedPrecondition(format!(
                            "foreign key violation: {}",
                            db_err.message()
                        ));
                    }
                }
                Self::Database(db_err.message().into())
            }
            sqlx::Error::Io(e) => Self::Database(format!("IO error: {}", e)),
            sqlx::Error::Tls(e) => Self::Internal(format!("TLS error: {}", e)),
            sqlx::Error::Protocol(e) => Self::Internal(format!("protocol error: {}", e)),
            sqlx::Error::InvalidArgument(e) => Self::InvalidArgument(e.to_string()),
            sqlx::Error::TypeNotFound { type_name } => {
                Self::Internal(format!("type not found: {}", type_name))
            }
            sqlx::Error::ColumnDecode { index, source } => {
                Self::Internal(format!("decode error at index {}: {}", index, source))
            }
            sqlx::Error::Encode(e) => Self::Internal(format!("encode error: {}", e)),
            sqlx::Error::Decode(e) => Self::Internal(format!("decode error: {}", e)),
            sqlx::Error::Configuration(e) => Self::Internal(format!("config error: {}", e)),
            sqlx::Error::Migrate(e) => Self::Internal(format!("migration error: {}", e)),
            sqlx::Error::WorkerCrashed => Self::Internal("internal worker crashed".into()),
            sqlx::Error::BeginFailed => Self::Database("failed to start transaction".into()),
            sqlx::Error::InvalidSavePointStatement => Self::Internal("invalid savepoint".into()),
            sqlx::Error::AnyDriverError(e) => Self::Internal(format!("driver error: {}", e)),
            _ => Self::Internal("an unexpected database error occurred".into()),
        }
    }
}

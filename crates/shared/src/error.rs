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
            sqlx::Error::Database(db_err) => match db_err.kind() {
                sqlx::error::ErrorKind::UniqueViolation => {
                    Self::AlreadyExists(db_err.message().into())
                }
                sqlx::error::ErrorKind::ForeignKeyViolation => {
                    Self::FailedPrecondition(format!("foreign key violation: {}", db_err.message()))
                }
                sqlx::error::ErrorKind::NotNullViolation
                | sqlx::error::ErrorKind::CheckViolation => {
                    Self::InvalidArgument(db_err.message().into())
                }
                _ => Self::Database(db_err.message().into()),
            },
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

#[cfg(test)]
mod tests {
    use super::*;

    fn msg() -> String {
        "the message".to_string()
    }

    #[cfg(feature = "serverfn")]
    #[test]
    fn as_status_code_maps_every_variant_exhaustively() {
        use dioxus_fullstack::AsStatusCode;
        use dioxus_fullstack::http::StatusCode;
        for err in [
            AppError::NotFound(msg()),
            AppError::InvalidArgument(msg()),
            AppError::AlreadyExists(msg()),
            AppError::FailedPrecondition(msg()),
            AppError::Internal(msg()),
            AppError::Database(msg()),
        ] {
            let expected = match &err {
                AppError::NotFound(_) => StatusCode::NOT_FOUND,
                AppError::InvalidArgument(_) => StatusCode::BAD_REQUEST,
                AppError::AlreadyExists(_) => StatusCode::BAD_REQUEST,
                AppError::FailedPrecondition(_) => StatusCode::PRECONDITION_FAILED,
                AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
                AppError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            assert_eq!(err.as_status_code(), expected, "wrong status for {err:?}");
        }
    }

    #[test]
    fn display_renders_the_documented_prefix_for_every_variant() {
        assert_eq!(
            AppError::NotFound(msg()).to_string(),
            "not found: the message"
        );
        assert_eq!(
            AppError::InvalidArgument(msg()).to_string(),
            "invalid argument: the message"
        );
        assert_eq!(
            AppError::AlreadyExists(msg()).to_string(),
            "already exists: the message"
        );
        assert_eq!(
            AppError::FailedPrecondition(msg()).to_string(),
            "failed precondition: the message"
        );
        assert_eq!(
            AppError::Internal(msg()).to_string(),
            "internal: the message"
        );
        assert_eq!(
            AppError::Database(msg()).to_string(),
            "database: the message"
        );
    }

    #[test]
    fn serde_uses_external_tagging_and_round_trips() {
        let json = serde_json::to_string(&AppError::NotFound("boom".into())).unwrap();
        assert_eq!(json, r#"{"NotFound":"boom"}"#);
        let back: AppError = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppError::NotFound(s) if s == "boom"));

        let json = serde_json::to_string(&AppError::FailedPrecondition("no".into())).unwrap();
        assert_eq!(json, r#"{"FailedPrecondition":"no"}"#);
        let back: AppError = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AppError::FailedPrecondition(s) if s == "no"));
    }

    #[cfg(feature = "axum-error")]
    async fn body_string(resp: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[cfg(feature = "axum-error")]
    #[tokio::test]
    async fn into_response_exposes_client_errors_verbatim() {
        use axum::response::IntoResponse;
        for (err, status) in [
            (
                AppError::NotFound("host 5".into()),
                axum::http::StatusCode::NOT_FOUND,
            ),
            (
                AppError::InvalidArgument("host 5".into()),
                axum::http::StatusCode::BAD_REQUEST,
            ),
            (
                AppError::AlreadyExists("host 5".into()),
                axum::http::StatusCode::BAD_REQUEST,
            ),
            (
                AppError::FailedPrecondition("host 5".into()),
                axum::http::StatusCode::PRECONDITION_FAILED,
            ),
        ] {
            let resp = err.into_response();
            assert_eq!(resp.status(), status);
            assert_eq!(body_string(resp).await, r#"{"error":"host 5"}"#);
        }
    }

    #[cfg(feature = "axum-error")]
    #[tokio::test]
    async fn into_response_hides_internal_and_database_detail_from_clients() {
        use axum::response::IntoResponse;
        for err in [
            AppError::Internal("secret connection string".into()),
            AppError::Database("secret connection string".into()),
        ] {
            let resp = err.into_response();
            assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
            let body = body_string(resp).await;
            assert_eq!(body, r#"{"error":"An internal server error occurred"}"#);
            assert!(!body.contains("secret connection string"));
        }
    }

    #[cfg(feature = "sqlx-error")]
    async fn mem_pool() -> sqlx::SqlitePool {
        use sqlx::sqlite::SqlitePoolOptions;
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[cfg(feature = "sqlx-error")]
    #[tokio::test]
    async fn row_not_found_maps_to_not_found() {
        let pool = mem_pool().await;
        let err = match sqlx::query("SELECT 1 WHERE 1 = 0").fetch_one(&pool).await {
            Ok(_) => panic!("expected RowNotFound"),
            Err(e) => e,
        };
        assert!(matches!(err, sqlx::Error::RowNotFound));
        assert!(matches!(AppError::from(err), AppError::NotFound(_)));
    }

    #[cfg(feature = "sqlx-error")]
    #[tokio::test]
    async fn a_unique_violation_maps_to_already_exists_so_the_dashboard_sees_400() {
        use dioxus_fullstack::AsStatusCode;
        let pool = mem_pool().await;
        sqlx::query("CREATE TABLE t (name TEXT UNIQUE)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t (name) VALUES ('a')")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO t (name) VALUES ('a')")
            .execute(&pool)
            .await
            .unwrap_err();

        let mapped = AppError::from(err);
        assert!(
            matches!(mapped, AppError::AlreadyExists(_)),
            "got {mapped:?}"
        );
        assert_eq!(
            mapped.as_status_code(),
            dioxus_fullstack::http::StatusCode::BAD_REQUEST
        );
    }

    #[cfg(feature = "sqlx-error")]
    #[tokio::test]
    async fn a_foreign_key_violation_maps_to_failed_precondition_so_the_dashboard_sees_412() {
        use dioxus_fullstack::AsStatusCode;
        let pool = mem_pool().await;
        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE parent (id INTEGER PRIMARY KEY)")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("CREATE TABLE child (parent_id INTEGER REFERENCES parent(id))")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO child (parent_id) VALUES (999)")
            .execute(&pool)
            .await
            .unwrap_err();

        let mapped = AppError::from(err);
        assert!(
            matches!(mapped, AppError::FailedPrecondition(_)),
            "got {mapped:?}"
        );
        assert_eq!(
            mapped.as_status_code(),
            dioxus_fullstack::http::StatusCode::PRECONDITION_FAILED
        );
    }

    #[cfg(feature = "sqlx-error")]
    #[tokio::test]
    async fn a_not_null_violation_maps_to_invalid_argument_so_the_dashboard_sees_400() {
        use dioxus_fullstack::AsStatusCode;
        let pool = mem_pool().await;
        sqlx::query("CREATE TABLE t (name TEXT NOT NULL)")
            .execute(&pool)
            .await
            .unwrap();
        let err = sqlx::query("INSERT INTO t (name) VALUES (NULL)")
            .execute(&pool)
            .await
            .unwrap_err();

        let mapped = AppError::from(err);
        assert!(
            matches!(mapped, AppError::InvalidArgument(_)),
            "got {mapped:?}"
        );
        assert_eq!(
            mapped.as_status_code(),
            dioxus_fullstack::http::StatusCode::BAD_REQUEST
        );
    }

    #[cfg(feature = "sqlx-error")]
    #[tokio::test]
    async fn pool_timeout_maps_to_database() {
        use sqlx::sqlite::SqlitePoolOptions;
        use std::time::Duration;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .acquire_timeout(Duration::from_millis(50))
            .connect("sqlite::memory:")
            .await
            .unwrap();
        let held = pool.acquire().await.unwrap();
        let err = match sqlx::query("SELECT 1").fetch_one(&pool).await {
            Ok(_) => panic!("expected PoolTimedOut"),
            Err(e) => e,
        };
        assert!(matches!(err, sqlx::Error::PoolTimedOut));
        assert!(matches!(AppError::from(err), AppError::Database(_)));
        drop(held);
    }
}

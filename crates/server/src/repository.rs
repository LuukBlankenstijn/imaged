use std::sync::Arc;

use sqlx::SqlitePool;

use crate::{
    domain::{host::HostRepository, image::ImageRepository, task::TaskRepository},
    error::AppError,
};

mod host;
mod image;
mod task;

pub fn host_repo(pool: SqlitePool) -> Arc<dyn HostRepository> {
    Arc::new(host::SqliteHostRepository::new(pool))
}

pub fn image_repo(pool: SqlitePool) -> Arc<dyn ImageRepository> {
    Arc::new(image::SqliteImageRepository::new(pool))
}

pub fn task_repo(pool: SqlitePool) -> Arc<dyn TaskRepository> {
    Arc::new(task::SqliteTaskRepository::new(pool))
}

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

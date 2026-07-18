use dioxus::prelude::ServerFnError;
use imaged_server_core::error::AppError;

pub fn sfe(e: AppError) -> ServerFnError {
    let (code, message): (u16, String) = match e {
        AppError::NotFound(m) => (404, m),
        AppError::InvalidArgument(m) => (400, m),
        AppError::AlreadyExists(m) => (409, m),
        AppError::FailedPrecondition(m) => (412, m),
        AppError::Internal(m) => {
            tracing::error!(error = %m, "internal error");
            (500, "internal server error".to_string())
        }
        AppError::Database(m) => {
            tracing::error!(error = %m, "database error");
            (500, "internal server error".to_string())
        }
    };
    ServerFnError::ServerError {
        message,
        code,
        details: None,
    }
}

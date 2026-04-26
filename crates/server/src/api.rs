use tonic::Status;

use crate::error::AppError;

pub mod client;
pub mod dashboard;

impl From<AppError> for Status {
    fn from(err: AppError) -> Self {
        match &err {
            AppError::NotFound(msg) => Status::not_found(msg.clone()),
            AppError::InvalidArgument(msg) => Status::invalid_argument(msg.clone()),
            AppError::AlreadyExists(msg) => Status::already_exists(msg.clone()),
            AppError::FailedPrecondition(msg) => Status::failed_precondition(msg.clone()),
            AppError::Internal(msg) => {
                tracing::error!(error = %msg, "internal error");
                Status::internal("internal server error")
            }
            AppError::Database(e) => {
                tracing::error!(error = %e, "database error");
                Status::internal("internal database error")
            }
        }
    }
}

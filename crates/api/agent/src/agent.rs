use axum::{extract::FromRequestParts, http::request::Parts};
use imaged_model::{
    agent::{IP_HEADER, MAC_HEADER},
    error::AppError,
};

pub struct AgentInfo {
    pub mac: String,
    pub ip: Option<String>,
}

impl<S> FromRequestParts<S> for AgentInfo
where
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        };
        let mac =
            header(MAC_HEADER).ok_or_else(|| AppError::InvalidArgument("missing mac".into()))?;
        Ok(AgentInfo {
            mac,
            ip: header(IP_HEADER),
        })
    }
}

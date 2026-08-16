use dioxus_fullstack::extract::FromRequestParts;
use dioxus_fullstack::http::request::Parts;
use imaged_core::error::AppError;

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
        let mac = parts
            .headers
            .get("X-Agent-Mac")
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::InvalidArgument("missing mac".into()))?
            .to_string();

        let ip = parts
            .headers
            .get("X-Agent-Ip")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);

        Ok(AgentInfo { mac, ip })
    }
}

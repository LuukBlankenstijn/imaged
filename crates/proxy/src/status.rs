use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    BadRequest,
    Forbidden,
    RequestTimeout,
    HeaderTooLarge,
    BadGateway,
    ServiceUnavailable,
    GatewayTimeout,
}

impl Status {
    fn code(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Forbidden => 403,
            Self::RequestTimeout => 408,
            Self::HeaderTooLarge => 431,
            Self::BadGateway => 502,
            Self::ServiceUnavailable => 503,
            Self::GatewayTimeout => 504,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::BadRequest => "Bad Request",
            Self::Forbidden => "Forbidden",
            Self::RequestTimeout => "Request Timeout",
            Self::HeaderTooLarge => "Request Header Fields Too Large",
            Self::BadGateway => "Bad Gateway",
            Self::ServiceUnavailable => "Service Unavailable",
            Self::GatewayTimeout => "Gateway Timeout",
        }
    }

    pub(crate) fn response(self) -> String {
        format!("HTTP/1.1 {self}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.code(), self.reason())
    }
}

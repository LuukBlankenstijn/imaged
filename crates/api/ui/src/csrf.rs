use dioxus::server::axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

const SEC_FETCH_SITE: &str = "sec-fetch-site";
const UPLOAD_SIZE: &str = "x-content-size";
const ACCEPTED_CONTENT_TYPES: [&str; 2] = ["application/json", "application/octet-stream"];

pub(crate) async fn refuse_cross_site_requests(request: Request, next: Next) -> Response {
    if request.method().is_safe() {
        return next.run(request).await;
    }
    if !is_same_origin(&request) {
        return (StatusCode::FORBIDDEN, "cross-site request refused").into_response();
    }
    if !has_accepted_body(&request) {
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "expected an application/json or application/octet-stream request body",
        )
            .into_response();
    }
    next.run(request).await
}

fn is_same_origin(request: &Request) -> bool {
    let headers = request.headers();
    if let Some(site) = headers.get(SEC_FETCH_SITE) {
        return matches!(site.as_bytes(), b"same-origin" | b"none");
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.as_str())
        });
    let origin_authority = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, authority)| authority);
    matches!((origin_authority, host), (Some(origin), Some(host)) if origin.eq_ignore_ascii_case(host))
}

fn has_accepted_body(request: &Request) -> bool {
    let headers = request.headers();
    headers.contains_key(UPLOAD_SIZE)
        || headers
            .get(header::CONTENT_TYPE)
            .and_then(|content_type| content_type.to_str().ok())
            .and_then(|content_type| content_type.split(';').next())
            .is_some_and(|essence| {
                ACCEPTED_CONTENT_TYPES
                    .iter()
                    .any(|accepted| essence.trim().eq_ignore_ascii_case(accepted))
            })
}

#[cfg(test)]
mod tests {
    use dioxus::server::axum::{
        Router,
        body::Body,
        http::{self, Method},
        middleware,
        routing::post,
    };
    use tower::ServiceExt;

    use super::*;

    async fn status(method: Method, headers: &[(&str, &str)]) -> StatusCode {
        let app = Router::new()
            .route("/api/ui/action", post(|| async {}).get(|| async {}))
            .layer(middleware::from_fn(refuse_cross_site_requests));
        let mut request = http::Request::builder()
            .method(method)
            .uri("/api/ui/action")
            .header(header::HOST, "imaged.lan:8081");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = app
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap();
        response.status()
    }

    const JSON: (&str, &str) = ("content-type", "application/json");

    #[tokio::test]
    async fn the_dashboard_and_scripts_without_browser_headers_are_served() {
        for headers in [
            vec![
                JSON,
                ("sec-fetch-site", "same-origin"),
                ("origin", "http://imaged.lan:8081"),
            ],
            vec![JSON, ("origin", "http://imaged.lan:8081")],
            vec![JSON, ("sec-fetch-site", "none")],
            vec![("content-type", "application/json; charset=utf-8")],
            vec![("content-type", "application/octet-stream")],
        ] {
            assert_eq!(
                status(Method::POST, &headers).await,
                StatusCode::OK,
                "{headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_dashboards_archive_upload_passes_with_the_files_own_type() {
        for content_type in ["", "application/x-tar"] {
            let upload = [
                ("sec-fetch-site", "same-origin"),
                ("content-type", content_type),
                ("x-content-size", "1024"),
            ];
            assert_eq!(
                status(Method::POST, &upload).await,
                StatusCode::OK,
                "{content_type:?}"
            );
        }
        let cross_site = [("sec-fetch-site", "cross-site"), ("x-content-size", "1024")];
        assert_eq!(
            status(Method::POST, &cross_site).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn sec_fetch_site_decides_even_when_a_proxy_rewrote_the_host() {
        let proxied = [
            JSON,
            ("sec-fetch-site", "same-origin"),
            ("origin", "http://localhost:8080"),
        ];
        assert_eq!(status(Method::POST, &proxied).await, StatusCode::OK);
        let cross = [
            JSON,
            ("sec-fetch-site", "same-site"),
            ("origin", "http://imaged.lan:8081"),
        ];
        assert_eq!(status(Method::POST, &cross).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn cross_site_state_changes_are_forbidden() {
        for headers in [
            vec![JSON, ("sec-fetch-site", "cross-site")],
            vec![JSON, ("origin", "http://evil.example")],
            vec![JSON, ("origin", "http://imaged.lan:9999")],
            vec![JSON, ("origin", "null")],
        ] {
            assert_eq!(
                status(Method::POST, &headers).await,
                StatusCode::FORBIDDEN,
                "{headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn bodies_a_page_can_post_without_a_preflight_are_refused() {
        for headers in [
            vec![("content-type", "text/plain;charset=UTF-8")],
            vec![("content-type", "application/x-www-form-urlencoded")],
            vec![("content-type", "multipart/form-data; boundary=x")],
            vec![],
        ] {
            assert_eq!(
                status(Method::POST, &headers).await,
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "{headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn cross_site_reads_and_navigations_are_served() {
        let headers = [
            ("sec-fetch-site", "cross-site"),
            ("origin", "http://evil.example"),
        ];
        assert_eq!(status(Method::GET, &headers).await, StatusCode::OK);
    }
}

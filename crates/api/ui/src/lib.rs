pub mod model;

pub mod groups;
pub mod hosts;
pub mod images;
pub mod realtime;
pub mod scripts;
pub mod tasks;

#[cfg(feature = "server")]
mod convert;
#[cfg(feature = "server")]
mod csrf;

#[cfg(feature = "server")]
pub fn router() -> dioxus::server::axum::Router<dioxus::server::FullstackState> {
    use dioxus::server::{
        ServerFunction,
        axum::{Router, middleware},
    };

    ServerFunction::collect()
        .into_iter()
        .fold(Router::new(), |router, f| {
            router.route(f.path(), f.method_router())
        })
        .layer(middleware::from_fn(csrf::refuse_cross_site_requests))
}

#[cfg(all(test, feature = "server"))]
mod tests;

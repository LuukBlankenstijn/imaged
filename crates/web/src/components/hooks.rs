use dioxus::prelude::*;

pub fn use_poll<T, F>(interval_ms: u32, fetcher: impl FnMut() -> F + 'static) -> Resource<T>
where
    T: 'static,
    F: Future<Output = T> + 'static,
{
    use_poll_while(interval_ms, || true, fetcher)
}

pub fn use_poll_while<T, F>(
    interval_ms: u32,
    active: impl Fn() -> bool + Copy + 'static,
    fetcher: impl FnMut() -> F + 'static,
) -> Resource<T>
where
    T: 'static,
    F: Future<Output = T> + 'static,
{
    let resource = use_resource(fetcher);
    use_future(move || async move {
        let mut resource = resource;
        let mut was_active = true;
        loop {
            sleep(interval_ms).await;
            let is_active = active();
            if is_active || was_active {
                resource.restart();
            }
            was_active = is_active;
        }
    });
    resource
}

#[cfg(feature = "web")]
async fn sleep(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(not(feature = "web"))]
async fn sleep(_ms: u32) {
    std::future::pending::<()>().await
}

use std::fmt::Display;

use dioxus::prelude::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Polled<T> {
    pub value: Option<T>,
    pub error: Option<String>,
}

impl<T> Default for Polled<T> {
    fn default() -> Self {
        Self {
            value: None,
            error: None,
        }
    }
}

impl<T> Polled<T> {
    pub(crate) fn record<E: Display>(&mut self, result: Result<T, E>) {
        match result {
            Ok(value) => {
                self.value = Some(value);
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }
}

pub struct Poll<T: 'static> {
    state: Signal<Polled<T>>,
    future: UseFuture,
}

impl<T: 'static> Clone for Poll<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for Poll<T> {}

impl<T: 'static> Poll<T> {
    pub fn read(&self) -> ReadableRef<'_, Signal<Polled<T>>> {
        self.state.read()
    }

    pub fn restart(&mut self) {
        self.future.restart();
    }

    pub fn reload(&mut self) {
        self.state.set(Polled::default());
        self.future.restart();
    }
}

pub fn use_poll<T, E, F>(interval_ms: u32, fetch: impl FnMut() -> F + 'static) -> Poll<T>
where
    T: 'static,
    E: Display + 'static,
    F: Future<Output = Result<T, E>> + 'static,
{
    use_poll_while(interval_ms, || true, fetch)
}

pub fn use_poll_while<T, E, F>(
    interval_ms: u32,
    active: impl Fn() -> bool + Copy + 'static,
    mut fetch: impl FnMut() -> F + 'static,
) -> Poll<T>
where
    T: 'static,
    E: Display + 'static,
    F: Future<Output = Result<T, E>> + 'static,
{
    let mut state = use_signal(Polled::default);
    let fetch = use_callback(move |()| fetch());
    let future = use_future(move || async move {
        loop {
            let was_active = active();
            let result = fetch.call(()).await;
            state.write().record(result);
            sleep(interval_ms).await;
            if !was_active {
                while !active() {
                    sleep(interval_ms).await;
                }
            }
        }
    });
    Poll { state, future }
}

#[cfg(feature = "web")]
async fn sleep(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(not(feature = "web"))]
async fn sleep(_ms: u32) {
    std::future::pending::<()>().await
}

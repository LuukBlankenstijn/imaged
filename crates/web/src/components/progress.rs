use dioxus::prelude::*;

use crate::model::TaskProgress;

#[derive(Clone, Copy)]
pub struct Progress(pub Signal<Vec<TaskProgress>>);

pub fn use_progress_provider() {
    let progress = use_context_provider(|| Progress(Signal::new(Vec::new())));
    let mut current = progress.0;
    use_future(move || async move {
        #[cfg(feature = "web")]
        loop {
            if let Ok(mut stream) = crate::api::realtime::task_progress().await {
                while let Some(Ok(tasks)) = stream.recv().await {
                    current.set(tasks);
                }
            }
            current.set(Vec::new());
            gloo_timers::future::TimeoutFuture::new(1500).await;
        }
        #[cfg(not(feature = "web"))]
        let _ = &mut current;
    });
}

pub fn use_task_progress(task_id: i64) -> Option<TaskProgress> {
    let progress = use_context::<Progress>();
    progress
        .0
        .read()
        .iter()
        .find(|p| p.task_id == task_id)
        .cloned()
}

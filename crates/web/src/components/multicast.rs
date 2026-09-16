use dioxus::prelude::*;

use crate::model::MulticastProgress;

#[derive(Clone, Copy)]
pub struct Transfer(pub Signal<Option<MulticastProgress>>);

pub fn use_transfer_provider() {
    let transfer = use_context_provider(|| Transfer(Signal::new(None)));
    let mut current = transfer.0;
    use_future(move || async move {
        #[cfg(feature = "web")]
        loop {
            if let Ok(mut stream) = crate::api::realtime::multicast_progress().await {
                while let Some(Ok(progress)) = stream.recv().await {
                    current.set(progress);
                }
            }
            current.set(None);
            gloo_timers::future::TimeoutFuture::new(1500).await;
        }
        #[cfg(not(feature = "web"))]
        let _ = &mut current;
    });
}

pub fn use_transfer(task_id: i64) -> Option<MulticastProgress> {
    let transfer = use_context::<Transfer>();
    transfer.0.read().filter(|p| p.task_id == task_id)
}

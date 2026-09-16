use std::collections::HashMap;

use dioxus::prelude::*;

#[cfg(feature = "web")]
use crate::model::ConnectionUpdate;

#[derive(Clone, Copy)]
pub struct Connections(pub Signal<HashMap<i64, bool>>);

pub fn use_connection_provider() {
    let conns = use_context_provider(|| Connections(Signal::new(HashMap::new())));
    let mut map = conns.0;
    use_future(move || async move {
        #[cfg(feature = "web")]
        loop {
            if let Ok(mut stream) = crate::api::realtime::connection_state().await {
                while let Some(Ok(update)) = stream.recv().await {
                    match update {
                        ConnectionUpdate::Connected(hosts) => {
                            map.set(hosts.into_iter().map(|id| (id, true)).collect())
                        }
                        ConnectionUpdate::Changed(event) => {
                            map.write().insert(event.id, event.connected);
                        }
                    }
                }
            }
            gloo_timers::future::TimeoutFuture::new(1500).await;
        }
        #[cfg(not(feature = "web"))]
        let _ = &mut map;
    });
}

pub fn use_connection(id: i64) -> bool {
    let conns = use_context::<Connections>();
    conns.0.read().get(&id).copied().unwrap_or(false)
}

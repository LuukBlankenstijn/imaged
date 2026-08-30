mod common;

use common::{StubConfig, init_transport, stub_server};
use futures::StreamExt;

// dioxus-fullstack 0.7 wraps every native response body in a `send_wrapper::SendWrapper`
// (client.rs `ClientResponseDriver for reqwest::Response`, payloads/stream.rs
// `FromResponse for Streaming<Bytes>`), which panics when polled from a thread other
// than the one that built it. That is why the agent runs on a current_thread runtime.
// When this test starts failing, upstream has stopped wrapping and the runtime flavour
// in src/main.rs is free again.
#[tokio::test]
async fn a_response_stream_cannot_be_polled_from_another_thread() {
    let server = stub_server(StubConfig::new().with_partition(1, vec![7u8; 512 * 1024])).await;
    init_transport(&server.base_url, "aa:bb:cc:dd:ee:ff");

    let stream = api::deploy::download_partition_data(42, 1)
        .await
        .expect("download starts on this thread");

    let drained_elsewhere = std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async move {
                let mut inner = stream.into_inner();
                while let Some(chunk) = inner.next().await {
                    chunk.expect("chunk");
                }
            })
    })
    .join();

    assert!(
        drained_elsewhere.is_err(),
        "the agent's current_thread runtime is load bearing: polling a response \
         stream off its creating thread must still panic"
    );
}

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{StubConfig, StubServer, agent_server, stub_server};
use imaged_agent::connection::{self, Timing};
use imaged_agent::task::ClientState;

const MAC: &str = "aa:bb:cc:dd:ee:01";

const TEST_TIMING: Timing = Timing {
    retry_start: Duration::from_millis(5),
    retry_cap: Duration::from_millis(20),
    server_silence: Duration::from_secs(60),
};

async fn wait_for(mut ready: impl FnMut() -> bool, within: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while tokio::time::Instant::now() < deadline {
        if ready() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    ready()
}

// The server evicts an agent that stops answering pings, so the agent must answer them.
// It never sends a pong itself: the reply comes from tungstenite while the event stream
// is polled. Nothing else in the suite covers that, because the e2e harness pings a
// tokio-tungstenite socket rather than the real agent client.
#[tokio::test]
async fn the_agent_answers_server_pings_while_it_waits_for_events() {
    let server: StubServer = stub_server(StubConfig::new()).await;
    let agent = agent_server(&server.base_url, MAC);

    let state = Arc::new(ClientState::default());

    let driver = async {
        assert!(
            wait_for(|| server.upgrades() >= 1, Duration::from_secs(5)).await,
            "the agent must open a websocket"
        );

        for _ in 0..3 {
            server.ping_connections();
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            wait_for(|| server.pongs() >= 3, Duration::from_secs(5)).await,
            "the agent must answer every server ping, got {} pong(s); \
             without this the server's liveness deadline evicts healthy agents",
            server.pongs()
        );

        assert_eq!(
            server.upgrades(),
            1,
            "answering pings must not have cost the agent its connection"
        );
    };

    tokio::select! {
        _ = connection::run(agent, state.clone(), Ok(1024), TEST_TIMING) => {
            unreachable!("the agent must never exit while the connection is healthy")
        }
        _ = driver => {}
    }
}

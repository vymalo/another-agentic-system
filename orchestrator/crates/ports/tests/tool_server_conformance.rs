//! Runs the `ToolServerClient` conformance testkit against the in-memory tool servers.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use orch_ports::memory::{MemoryToolServer, MemoryToolServers};
use orch_ports::testkit::tool_server::{SeenRequest, ToolServerFixture};
use orch_ports::{ToolSecret, ToolServerEndpoint};

const URL: &str = "memory://tools/mcp";
const BEARER: &str = "bearer-0123456789-very-secret";
const HEADER: (&str, &str) = ("X-Fixture-Key", "header-0123456789-very-secret");

struct Memory(MemoryToolServers);

impl ToolServerFixture for Memory {
    type Client = MemoryToolServers;

    fn client(&self) -> &MemoryToolServers {
        &self.0
    }

    fn bearer(&self) -> &str {
        BEARER
    }

    fn header(&self) -> (&str, &str) {
        HEADER
    }

    fn endpoint(&self) -> ToolServerEndpoint {
        ToolServerEndpoint::new("fixture", URL, Duration::from_secs(10))
            .with_bearer(ToolSecret::new(BEARER))
            .with_header(HEADER.0, ToolSecret::new(HEADER.1))
    }

    fn unreachable_endpoint(&self) -> ToolServerEndpoint {
        ToolServerEndpoint {
            url: "memory://nobody/mcp".to_owned(),
            ..self.endpoint()
        }
    }

    fn seen(&self) -> Vec<SeenRequest> {
        self.0.seen(URL)
    }
}

async fn make() -> Option<Memory> {
    let servers = MemoryToolServers::new();
    servers.add(
        URL,
        MemoryToolServer::new()
            .with_bearer(BEARER)
            .with_required_header(HEADER.0, HEADER.1)
            .with_standard_tools(),
    );
    Some(Memory(servers))
}

orch_ports::tool_server_conformance!(make);

#[tokio::test]
async fn a_server_that_is_down_is_unreachable_until_it_is_up() {
    use orch_ports::{ToolServerClient, ToolServerError};
    let f = make().await.unwrap();
    f.0.set_down(URL, true);
    let err = f.0.list_tools(&f.endpoint()).await.unwrap_err();
    assert!(
        matches!(err, ToolServerError::Unreachable { .. }),
        "{err:?}"
    );
    f.0.set_down(URL, false);
    assert_eq!(f.0.list_tools(&f.endpoint()).await.unwrap().len(), 4);
}

#[tokio::test]
async fn a_refused_request_is_not_in_the_journal() {
    use orch_ports::ToolServerClient;
    let f = make().await.unwrap();
    let wrong = ToolServerEndpoint {
        bearer: None,
        ..f.endpoint()
    };
    f.0.list_tools(&wrong).await.unwrap_err();
    assert!(f.seen().is_empty());
}

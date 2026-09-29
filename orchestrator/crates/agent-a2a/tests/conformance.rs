//! The `AgentClient` conformance testkit against the A2A adapter, over real HTTP, with an
//! in-process A2A 1.0 agent (`orch-testsupport`'s `FakeAgent`) behind it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::AgentId;
use orch_ports::AgentEndpoint;
use orch_ports::testkit::agent_client::AgentFixture;
use orch_testsupport::{FakeAgent, FakeAgentOptions};

struct Fixture {
    client: A2aAgentClient,
    agent: FakeAgent,
    /// A card URL whose port was bound and released: nothing listens there.
    gone: String,
}

impl AgentFixture for Fixture {
    type Client = A2aAgentClient;

    fn client(&self) -> &A2aAgentClient {
        &self.client
    }

    fn endpoint(&self) -> AgentEndpoint {
        self.agent.endpoint("fake", None)
    }

    fn unreachable(&self) -> AgentEndpoint {
        AgentEndpoint::a2a(AgentId::new("gone"), self.gone.clone(), None)
    }

    fn release_gate(&self) {
        self.agent.release_gate();
    }

    // The default script words (`echo`, `ask`, `gate`, `slow`, `fail`) are the fake agent's.
}

async fn make() -> Option<Fixture> {
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    Some(Fixture {
        client: A2aAgentClient::new(A2aConfig {
            use_system_proxy: false,
            ..A2aConfig::default()
        })
        .unwrap(),
        agent: FakeAgent::spawn(FakeAgentOptions::default()).await,
        gone: format!("http://{addr}/.well-known/agent-card.json"),
    })
}

orch_ports::agent_client_conformance!(make);

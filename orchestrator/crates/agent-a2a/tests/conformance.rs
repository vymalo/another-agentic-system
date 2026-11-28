//! The `AgentClient` conformance testkit against the A2A adapter, over real HTTP, with an
//! in-process A2A 1.0 agent (`orch-testsupport`'s `FakeAgent`) behind it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_core::AgentId;
use orch_ports::AgentEndpoint;
use orch_ports::testkit::agent_client::{AgentFixture, ReceivedFile, TestLinks};
use orch_testsupport::{FakeAgent, FakeAgentOptions};

struct Fixture {
    client: A2aAgentClient,
    agent: FakeAgent,
    /// An agent whose card lists `access-tier/v1` with the four tiers.
    tiered: FakeAgent,
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

    fn tiered_endpoint(&self) -> Option<AgentEndpoint> {
        Some(self.tiered.endpoint("tiered", None))
    }

    fn carries_attachments(&self) -> bool {
        true
    }

    fn received_files(&self, message_id: &str) -> Vec<ReceivedFile> {
        let text = |part: &serde_json::Value, key: &str| {
            part.get(key).and_then(|v| v.as_str()).map(str::to_owned)
        };
        self.agent
            .executions()
            .into_iter()
            .filter(|call| call.message_id.as_deref() == Some(message_id))
            .flat_map(|call| call.parts)
            .filter_map(|part| {
                Some(ReceivedFile {
                    url: text(&part, "url")?,
                    media_type: text(&part, "mediaType"),
                    filename: text(&part, "filename"),
                })
            })
            .collect()
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
            attachment_links: Some(std::sync::Arc::new(TestLinks)),
            ..A2aConfig::default()
        })
        .unwrap(),
        agent: FakeAgent::spawn(FakeAgentOptions::default()).await,
        tiered: FakeAgent::spawn(FakeAgentOptions {
            access_tier: Some(serde_json::json!({
                "tiers": ["plan", "edits", "auto", "bypass"],
                "default": "plan"
            })),
            ..FakeAgentOptions::default()
        })
        .await,
        gone: format!("http://{addr}/.well-known/agent-card.json"),
    })
}

orch_ports::agent_client_conformance!(make);

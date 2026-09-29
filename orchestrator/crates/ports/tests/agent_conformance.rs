//! Runs the `AgentClient` conformance testkit against the scripted in-memory agent.
#![allow(missing_docs)]

use orch_core::AgentId;
use orch_ports::AgentEndpoint;
use orch_ports::memory::ScriptedAgent;
use orch_ports::testkit::agent_client::{AgentFixture, Script};

struct Scripted(ScriptedAgent);

impl AgentFixture for Scripted {
    type Client = ScriptedAgent;

    fn client(&self) -> &ScriptedAgent {
        &self.0
    }

    fn endpoint(&self) -> AgentEndpoint {
        AgentEndpoint::a2a(
            AgentId::new("scripted"),
            "https://scripted.example.com/.well-known/agent-card.json",
            None,
        )
    }

    fn unreachable(&self) -> AgentEndpoint {
        AgentEndpoint::a2a(
            AgentId::new("gone"),
            "https://gone.example.com/.well-known/agent-card.json",
            None,
        )
    }

    fn release_gate(&self) {
        self.0.release_gate();
    }

    fn text(&self, script: Script) -> String {
        // Its `fail` word rejects the send; the word that fails the task is `failed`.
        match script {
            Script::Fail => "failed conformance".to_owned(),
            Script::Echo => "echo conformance".to_owned(),
            Script::Ask => "ask conformance".to_owned(),
            Script::Gate => "gate conformance".to_owned(),
            Script::Slow => "slow conformance".to_owned(),
        }
    }
}

async fn make() -> Option<Scripted> {
    let agent = ScriptedAgent::new();
    agent.set_unreachable("gone");
    Some(Scripted(agent))
}

orch_ports::agent_client_conformance!(make);

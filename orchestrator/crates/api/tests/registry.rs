//! The agent list as the registry shapes it (ADR 0022): `source` and `tags` on every agent, the
//! registry's agents listed from the moment the registry lists them, and none of them while the
//! registry cannot be read. The schema itself is checked in `contract.rs`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig};
use orch_core::{AgentId, AgentSource};
use orch_ports::memory::{MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{
    AgentEndpoint, CompositeRegistry, FixedRegistry, NoModel, PortSet, RegistryEntry, SystemClock,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    CompositeRegistry<FixedRegistry, MemoryRegistry>,
>;

const ALICE: &str = "alice@example.com";

struct Api {
    base: String,
    client: reqwest::Client,
    registry: MemoryRegistry,
    server: JoinHandle<()>,
}

impl Drop for Api {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn start() -> Api {
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("coder"),
            "https://coder.example.com/.well-known/agent-card.json",
            None,
        ),
        name: "Coder".to_owned(),
    }]);
    let registry = MemoryRegistry::new();
    let app: Arc<App<Stack>> = Arc::new(
        App::new(
            PortSet {
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: NoModel,
                registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
            },
            directory,
            AppConfig::default(),
        )
        .expect("a valid gate"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = orch_api::router_with_surfaces(app, ApiConfig::default(), Vec::new());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Api {
        base,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        registry,
        server,
    }
}

impl Api {
    async fn agents(&self) -> Value {
        let r = self
            .client
            .get(format!("{}/api/agents", self.base))
            .header("X-Auth-Request-Email", ALICE)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.json().await.unwrap()
    }
}

fn platform_agent(id: &str, name: &str, tags: &[&str]) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: name.to_owned(),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        origin: AgentSource::Registry,
    }
}

#[tokio::test]
async fn an_agent_says_where_it_is_listed_from_and_carries_its_tags() {
    let api = start().await;
    api.registry
        .add(platform_agent("helper", "Helper", &["writing", "docs"]));
    api.registry.add(platform_agent("scribe", "Scribe", &[]));
    let agents = api.agents().await;
    let agents = agents.as_array().unwrap();
    assert_eq!(
        agents
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["coder", "helper", "scribe"],
        "the static agents first, then the registry's in its own order"
    );
    // `source` is always there; `tags` only when the registry kept some (absent, not `[]`).
    assert_eq!(agents[0]["source"], "static");
    assert!(agents[0].get("tags").is_none(), "{}", agents[0]);
    assert_eq!(agents[1]["source"], "registry");
    assert_eq!(agents[1]["tags"], json!(["writing", "docs"]));
    assert_eq!(agents[1]["name"], "Helper");
    assert_eq!(
        agents[1]["cardUrl"],
        "https://helper.agents.example.com/.well-known/agent-card.json"
    );
    assert_eq!(agents[2]["source"], "registry");
    assert!(agents[2].get("tags").is_none(), "{}", agents[2]);
}

#[tokio::test]
async fn the_list_is_read_live_and_a_registry_that_is_down_lists_none_of_its_agents() {
    let api = start().await;
    api.registry.add(platform_agent("helper", "Helper", &[]));
    assert_eq!(api.agents().await.as_array().unwrap().len(), 2);

    api.registry.set_down(true);
    let agents = api.agents().await;
    assert_eq!(
        agents
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["coder"],
        "the static agent stays, the registry's is gone"
    );

    api.registry.set_down(false);
    api.registry.remove(&AgentId::new("helper"));
    assert_eq!(api.agents().await.as_array().unwrap().len(), 1);
}

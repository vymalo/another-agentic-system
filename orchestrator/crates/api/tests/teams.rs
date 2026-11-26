//! `/api/teams` (ADR 0074) over real HTTP against the in-memory stack: a preset is the caller's own
//! (another person's is a 404 for every method), a member's agent must be one the caller may use
//! (an audience that leaves them out is refused like an agent that is not there), the writes are
//! compare-and-swap on a revision, and every answer is never cached and matches
//! `docs/api/chat-api.yaml`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig};
use orch_core::{AgentId, AgentSource};
use orch_ports::memory::{
    MemoryAuth, MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentAudience, AgentEndpoint, CompositeRegistry, FixedRegistry, NoModel, PortSet, Principal,
    RegistryEntry, Role, SystemClock,
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
    MemoryAuth,
>;

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

fn platform(id: &str, audience: AgentAudience) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
        audience,
    }
}

struct Contract(Value);

impl Contract {
    fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        Contract(serde_norway::from_str(&std::fs::read_to_string(path).unwrap()).unwrap())
    }

    fn answer(&self, path: &str, method: &str, status: u16, body: &Value) {
        let responses = &self.0["paths"][path][method.to_lowercase()]["responses"];
        let declared = responses
            .get(status.to_string())
            .unwrap_or_else(|| panic!("the contract documents no {status} for {method} {path}"));
        let declared = match declared.get("$ref").and_then(Value::as_str) {
            Some(r) => self
                .0
                .pointer(r.strip_prefix('#').unwrap())
                .unwrap()
                .clone(),
            None => declared.clone(),
        };
        let Some(content) = declared.get("content").and_then(Value::as_object) else {
            return;
        };
        let (_, media) = content.iter().next().unwrap();
        let mut root = media["schema"].clone();
        let object = root.as_object_mut().unwrap();
        object.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        object.insert("components".to_owned(), self.0["components"].clone());
        let validator = jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .unwrap();
        let errors: Vec<String> = validator
            .iter_errors(body)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{}\n{body}", errors.join("\n"));
    }
}

struct Resp {
    status: u16,
    no_store: bool,
    body: Value,
}

impl Resp {
    fn code(&self) -> Option<&str> {
        self.body["code"].as_str()
    }
}

struct Api {
    base: String,
    client: reqwest::Client,
    contract: Contract,
    server: JoinHandle<()>,
}

impl Drop for Api {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// `alice` and `bob` are users; `holder` is a user who also holds `agent.use:mine`.
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
    registry.add(platform(
        "mine",
        AgentAudience::from_values(["agent.use:mine"]),
    ));
    let auth = MemoryAuth::new();
    auth.allow("alice", principal("alice@example.com", &["user"]));
    auth.allow("bob", principal("bob@example.com", &["user"]));
    auth.allow(
        "holder",
        principal("holder@example.com", &["user", "agent.use:mine"]),
    );
    let app: Arc<App<Stack>> = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: NoModel,
                auth,
                registry: CompositeRegistry::new(directory.fixed_registry(), registry),
                user_tools: orch_ports::NoUserTools::off(),
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
        contract: Contract::load(),
        server,
    }
}

impl Api {
    async fn call(&self, token: &str, method: &str, path: &str, body: Option<&Value>) -> Resp {
        let mut req = self
            .client
            .request(method.parse().unwrap(), format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            req = req.json(body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let no_store = resp
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok())
            == Some("no-store");
        let bytes = resp.bytes().await.unwrap();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        let template = if path.matches('/').count() == 2 {
            "/api/teams"
        } else {
            "/api/teams/{teamId}"
        };
        if status != 401 && path.starts_with("/api/teams") {
            self.contract.answer(template, method, status, &body);
        }
        Resp {
            status,
            no_store,
            body,
        }
    }
}

fn team(name: &str, second_agent: &str) -> Value {
    json!({
        "name": name,
        "title": format!("Team {name}"),
        "lead": "lead",
        "members": [
            { "name": "lead", "target": { "agentId": "coder" }, "role": "plans the work" },
            { "name": "helper", "target": { "agentId": second_agent }, "role": "does the work" }
        ],
        "charter": "Ship it.",
        "maxHandoffs": 10,
        "pushTokens": 24000
    })
}

#[tokio::test]
async fn a_person_saves_reads_lists_replaces_and_deletes_their_own_preset() {
    let api = start().await;
    let made = api
        .call("alice", "POST", "/api/teams", Some(&team("dev", "coder")))
        .await;
    assert_eq!(made.status, 201, "{}", made.body);
    assert!(made.no_store);
    let id = made.body["id"].as_str().unwrap().to_owned();
    let revision = made.body["revision"].as_str().unwrap().to_owned();

    let got = api
        .call("alice", "GET", &format!("/api/teams/{id}"), None)
        .await;
    assert_eq!(got.status, 200);
    assert_eq!(got.body, made.body);

    let list = api.call("alice", "GET", "/api/teams", None).await;
    assert_eq!(list.status, 200);
    assert_eq!(list.body["held"], 1);
    assert_eq!(list.body["items"][0]["memberCount"], 2);

    let mut next = team("dev", "coder");
    next["title"] = json!("Renamed");
    next["revision"] = json!(revision);
    let put = api
        .call("alice", "PUT", &format!("/api/teams/{id}"), Some(&next))
        .await;
    assert_eq!(put.status, 200, "{}", put.body);
    assert_eq!(put.body["title"], "Renamed");

    // the old revision is stale
    let stale = api
        .call("alice", "PUT", &format!("/api/teams/{id}"), Some(&next))
        .await;
    assert_eq!(stale.status, 409);
    assert_eq!(stale.code(), Some("stale_revision"));

    let gone = api
        .call("alice", "DELETE", &format!("/api/teams/{id}"), None)
        .await;
    assert_eq!(gone.status, 204);
    let again = api
        .call("alice", "DELETE", &format!("/api/teams/{id}"), None)
        .await;
    assert_eq!(again.status, 404);
}

#[tokio::test]
async fn team_preset_with_agent_outside_audience_is_refused() {
    let api = start().await;
    // bob may not use `mine`: refused, naming the member and never the agent
    let refused = api
        .call("bob", "POST", "/api/teams", Some(&team("dev", "mine")))
        .await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(refused.code(), Some("member_agent_refused"));
    assert_eq!(refused.body["at"], "members.1.agentId");
    assert!(!refused.body.to_string().contains("mine"));
    // an agent nobody lists is the same answer
    let unknown = api
        .call("bob", "POST", "/api/teams", Some(&team("dev", "nowhere")))
        .await;
    assert_eq!(unknown.status, 422);
    assert_eq!(unknown.body["detail"], refused.body["detail"]);
    // nothing was written
    assert_eq!(
        api.call("bob", "GET", "/api/teams", None).await.body["held"],
        0
    );
    // the holder of the audience may
    let ok = api
        .call("holder", "POST", "/api/teams", Some(&team("dev", "mine")))
        .await;
    assert_eq!(ok.status, 201, "{}", ok.body);
    // and a replace that brings an agent outside the audience is refused too
    let id = ok.body["id"].as_str().unwrap();
    let mut next = team("dev", "coder");
    next["revision"] = ok.body["revision"].clone();
    assert_eq!(
        api.call("holder", "PUT", &format!("/api/teams/{id}"), Some(&next))
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn other_persons_preset_is_404() {
    let api = start().await;
    let made = api
        .call("alice", "POST", "/api/teams", Some(&team("dev", "coder")))
        .await;
    let id = made.body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/teams/{id}");
    let never = api
        .call("bob", "GET", "/api/teams/t-aaaaaaaaaaaa", None)
        .await;
    let get = api.call("bob", "GET", &path, None).await;
    assert_eq!(get.status, 404);
    assert_eq!(
        get.body, never.body,
        "the same answer as for an id that never existed"
    );
    let mut put = team("dev", "coder");
    put["revision"] = made.body["revision"].clone();
    assert_eq!(api.call("bob", "PUT", &path, Some(&put)).await.status, 404);
    assert_eq!(api.call("bob", "DELETE", &path, None).await.status, 404);
    assert_eq!(
        api.call("bob", "GET", "/api/teams", None).await.body["held"],
        0
    );
    // still there for its owner, unchanged
    assert_eq!(api.call("alice", "GET", &path, None).await.status, 200);
}

#[tokio::test]
async fn the_rules_of_the_page_and_the_names_are_refused() {
    let api = start().await;
    let mut bad = team("dev", "coder");
    bad["lead"] = json!("nobody");
    let r = api.call("alice", "POST", "/api/teams", Some(&bad)).await;
    assert_eq!(r.status, 400);
    assert_eq!(r.code(), Some("bad_lead"));
    assert_eq!(r.body["at"], "lead");
    assert_eq!(
        api.call("alice", "POST", "/api/teams", Some(&team("dev", "coder")))
            .await
            .status,
        201
    );
    let dup = api
        .call("alice", "POST", "/api/teams", Some(&team("dev", "coder")))
        .await;
    assert_eq!(dup.status, 409);
    assert_eq!(dup.code(), Some("name_taken"));
    assert_eq!(
        api.call("alice", "GET", "/api/teams/not-an-id", None)
            .await
            .status,
        400
    );
    let config = api.call("alice", "GET", "/api/config", None).await;
    assert_eq!(config.body["teams"]["limit"], 50);
}

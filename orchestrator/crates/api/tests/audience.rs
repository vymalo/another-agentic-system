//! The audience of a platform agent over real HTTP (ADR 0045, decision 1): the agent list holds the
//! agents the person may read, a thread whose agent they may no longer use is read-only and not
//! hidden, an agent the audience shuts them out of is asked for like one that is not there, and
//! `GET /api/me` still says what the roles say.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, NewThread, ToolServerInfo};
use orch_core::{AgentId, AgentSource, AgentTarget, ThreadId};
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

const ALICE: &str = "alice@example.com";

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

struct Api {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    registry: MemoryRegistry,
    server: JoinHandle<()>,
}

impl Drop for Api {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct Resp {
    status: u16,
    body: Value,
}

impl Resp {
    fn detail(&self) -> &str {
        self.body["detail"].as_str().unwrap_or_default()
    }
}

/// `alice` is a user, `holder` a user who holds `agent.use:mine`, `root` an administrator.
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
    registry.add(platform("open", AgentAudience::Everyone));
    registry.add(platform(
        "mine",
        AgentAudience::from_values(["agent.use:mine"]),
    ));
    registry.add(platform("fresh", AgentAudience::AdminsOnly));
    let auth = MemoryAuth::new();
    auth.allow("alice", principal(ALICE, &["user"]));
    auth.allow(
        "holder",
        principal("holder@example.com", &["user", "agent.use:mine"]),
    );
    auth.allow("root", principal("root@example.com", &["admin"]));
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
                registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            AppConfig {
                tool_servers: vec![
                    ToolServerInfo::new("docs", "Documentation"),
                    ToolServerInfo::new("wiki", "Wiki"),
                ],
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), Vec::new());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Api {
        base,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        app,
        registry,
        server,
    }
}

impl Api {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Resp {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await.unwrap();
        let status = response.status().as_u16();
        let bytes = response.bytes().await.unwrap();
        Resp {
            status,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn get(&self, path: &str, token: &str) -> Resp {
        self.call(reqwest::Method::GET, path, token, None).await
    }

    /// `PUT /api/threads/{id}/tools` as `alice`.
    async fn put_tools(&self, thread: ThreadId, servers: Value) -> Resp {
        self.call(
            reqwest::Method::PUT,
            &format!("/api/threads/{thread}/tools"),
            "alice",
            Some(json!({"servers": servers})),
        )
        .await
    }

    /// `POST /api/threads/{id}/fork` as `alice`, onto `agent`.
    async fn fork_onto(&self, thread: ThreadId, agent: &str) -> Resp {
        self.call(
            reqwest::Method::POST,
            &format!("/api/threads/{thread}/fork"),
            "alice",
            Some(json!({"after": 1, "target": {"agentId": agent}})),
        )
        .await
    }

    async fn agents(&self, token: &str) -> Vec<String> {
        let r = self.get("/api/agents", token).await;
        assert_eq!(r.status, 200);
        r.body
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap().to_owned())
            .collect()
    }

    /// A thread of `alice` on the registry agent `open`, made while it is for everybody.
    async fn thread_on_open(&self) -> ThreadId {
        self.app
            .create_thread(
                &principal(ALICE, &["user"]),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("open"),
                        release: None,
                    },
                    text: "hello".to_owned(),
                },
            )
            .await
            .unwrap()
            .id
    }
}

#[tokio::test]
async fn the_agent_list_is_what_the_audience_lets_the_person_see() {
    let api = start().await;
    assert_eq!(api.agents("alice").await, ["coder", "open"]);
    assert_eq!(api.agents("holder").await, ["coder", "open", "mine"]);
    assert_eq!(api.agents("root").await, ["coder", "open", "mine", "fresh"]);
}

#[tokio::test]
async fn me_still_says_the_roles_patterns_because_an_audience_cannot_be_said_as_one() {
    let api = start().await;
    for token in ["alice", "root"] {
        let me = api.get("/api/me", token).await;
        assert_eq!(me.status, 200);
        assert_eq!(
            me.body["agents"],
            json!({"read": ["*"], "invoke": ["*"]}),
            "{token}"
        );
    }
    // while the list the web draws its picker from is filtered
    assert!(!api.agents("alice").await.contains(&"fresh".to_owned()));
}

#[tokio::test]
async fn a_thread_whose_agent_the_person_may_no_longer_use_is_read_only_and_not_hidden() {
    let api = start().await;
    let id = api.thread_on_open().await;
    let thread = format!("/api/threads/{id}");
    assert_eq!(api.put_tools(id, json!(["docs"])).await.status, 200);

    // the platform takes the person out of its audience
    api.registry.add(platform(
        "open",
        AgentAudience::from_values(["agent.use:somebody-else"]),
    ));

    // it is still theirs: in their list, readable, and so are its events
    let r = api.get(&thread, "alice").await;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["target"]["agentId"], "open");
    let list = api.get("/api/threads", "alice").await;
    assert!(
        list.body
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == id.to_string()),
        "{}",
        list.body
    );
    let export = api.get(&format!("{thread}/export"), "alice").await;
    assert_eq!(export.status, 200);

    // attaching a server is for the agent that will use it: 403, as when `agent.invoke` is lost
    let attach = api.put_tools(id, json!(["docs", "wiki"])).await;
    assert_eq!(attach.status, 403, "{}", attach.body);
    assert_eq!(attach.body["code"], "forbidden");
    assert_eq!(
        attach.detail(),
        "your roles do not grant agent.invoke for the agent open"
    );
    // what is attached already is theirs to keep or to take off
    let same = api.put_tools(id, json!(["docs"])).await;
    assert_eq!(
        (same.status, &same.body["servers"]),
        (200, &json!(["docs"]))
    );
    let off = api.put_tools(id, json!([])).await;
    assert_eq!((off.status, &off.body["servers"]), (200, &json!([])));
}

#[tokio::test]
async fn forking_onto_an_agent_the_audience_hides_is_the_answer_for_an_agent_that_is_not_there() {
    let api = start().await;
    let id = api.thread_on_open().await;
    let hidden = api.fork_onto(id, "fresh").await;
    let nowhere = api.fork_onto(id, "nowhere").await;
    assert_eq!(hidden.status, 400, "{}", hidden.body);
    assert_eq!(hidden.status, nowhere.status);
    assert_eq!(hidden.detail(), "unknown agent 'fresh'");
    assert_eq!(nowhere.detail(), "unknown agent 'nowhere'");
    // the same words, the same shape: nothing tells the one from the other but the id they sent
    assert_eq!(
        hidden.detail().replace("fresh", "_"),
        nowhere.detail().replace("nowhere", "_")
    );
    assert_eq!(hidden.body["title"], nowhere.body["title"]);
    assert_eq!(hidden.body["type"], nowhere.body["type"]);
    assert_eq!(hidden.body["code"], nowhere.body["code"]);
}

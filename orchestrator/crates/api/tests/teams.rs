//! `/api/teams` (ADR 0074 decision 13) over real HTTP against the in-memory stack: a preset is the
//! person's own (another person's is the answer for an id that never existed), every member's agent is
//! one the person may use (an agent whose audience leaves them out is not there, ADR 0045), a change is
//! conditional on the revision, and `GET /api/config` says the bounds.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, ToolServerInfo};
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
    server: JoinHandle<()>,
}

impl Drop for Api {
    fn drop(&mut self) {
        self.server.abort();
    }
}

struct Resp {
    status: u16,
    etag: Option<String>,
    body: Value,
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
    auth.allow("bob", principal("bob@example.com", &["user"]));
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
        server,
    }
}

impl Api {
    async fn call_with(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Value>,
        if_match: Option<&str>,
    ) -> Resp {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(tag) = if_match {
            req = req.header("If-Match", tag);
        }
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req.send().await.unwrap();
        let status = response.status().as_u16();
        let etag = response
            .headers()
            .get("etag")
            .map(|v| v.to_str().unwrap().to_owned());
        let bytes = response.bytes().await.unwrap();
        Resp {
            status,
            etag,
            body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        }
    }

    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> Resp {
        self.call_with(method, path, token, body, None).await
    }

    async fn get(&self, path: &str, token: &str) -> Resp {
        self.call(reqwest::Method::GET, path, token, None).await
    }
}

fn team(agent_a: &str, agent_b: &str) -> Value {
    json!({
        "name": "duo",
        "title": "A pair",
        "lead": "planner",
        "members": [
            {"name": "planner", "target": {"agentId": agent_a}, "role": "plans the work"},
            {"name": "builder", "target": {"agentId": agent_b}, "role": "builds it"}
        ],
        "charter": "Ship small changes.",
        "maxHandoffs": 6,
        "pushTokens": 24000
    })
}

fn post() -> reqwest::Method {
    reqwest::Method::POST
}

#[tokio::test]
async fn team_preset_with_agent_outside_audience_is_refused() {
    let api = start().await;
    // `mine` is for the holders of `agent.use:mine`; `alice` is not one
    let refused = api
        .call(post(), "/api/teams", "alice", Some(team("open", "mine")))
        .await;
    assert_eq!(refused.status, 422, "{}", refused.body);
    assert_eq!(refused.body["code"], "unknown_agent");
    assert_eq!(refused.body["at"], "members.1.agent");
    // an agent nobody lists is the same answer
    let nowhere = api
        .call(post(), "/api/teams", "alice", Some(team("open", "nowhere")))
        .await;
    assert_eq!(nowhere.status, 422);
    assert_eq!(nowhere.body["code"], "unknown_agent");
    // nothing was written
    let list = api.get("/api/teams", "alice").await;
    assert_eq!(list.body["held"], 0);
    // the holder may
    let ok = api
        .call(post(), "/api/teams", "holder", Some(team("open", "mine")))
        .await;
    assert_eq!(ok.status, 201, "{}", ok.body);
}

#[tokio::test]
async fn other_persons_preset_is_404() {
    let api = start().await;
    let made = api
        .call(post(), "/api/teams", "alice", Some(team("open", "coder")))
        .await;
    assert_eq!(made.status, 201, "{}", made.body);
    let id = made.body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/teams/{id}");
    let absent = "/api/teams/t-000000000000";
    let rev = made.etag.clone().unwrap();
    for who in ["bob", "root"] {
        let read = api.get(&path, who).await;
        let never = api.get(absent, who).await;
        assert_eq!(read.status, 404, "{who}");
        assert_eq!(
            read.body, never.body,
            "{who}: the answer for an id that never was"
        );
        let put = api
            .call_with(
                reqwest::Method::PUT,
                &path,
                who,
                Some(team("open", "coder")),
                Some(&rev),
            )
            .await;
        assert_eq!(put.status, 404, "{who}");
        let del = api
            .call_with(reqwest::Method::DELETE, &path, who, None, None)
            .await;
        assert_eq!(del.status, 404, "{who}");
        assert_eq!(api.get("/api/teams", who).await.body["held"], 0, "{who}");
    }
    // still there for the owner
    assert_eq!(api.get(&path, "alice").await.status, 200);
}

#[tokio::test]
async fn a_preset_is_replaced_and_deleted_on_its_revision() {
    let api = start().await;
    let made = api
        .call(post(), "/api/teams", "alice", Some(team("open", "coder")))
        .await;
    assert_eq!(made.status, 201);
    let id = made.body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/teams/{id}");
    let rev = made.etag.clone().unwrap();
    assert_eq!(
        made.body["revision"].as_str().unwrap(),
        rev.trim_matches('"')
    );

    // a name taken
    let mut other = team("open", "coder");
    other["name"] = json!("other");
    assert_eq!(
        api.call(post(), "/api/teams", "alice", Some(other))
            .await
            .status,
        201
    );
    assert_eq!(
        api.call(post(), "/api/teams", "alice", Some(team("open", "coder")))
            .await
            .body["code"],
        "name_taken"
    );

    // no If-Match
    let bare = api
        .call(
            reqwest::Method::PUT,
            &path,
            "alice",
            Some(team("open", "coder")),
        )
        .await;
    assert_eq!(bare.status, 400);

    let mut changed = team("open", "coder");
    changed["title"] = json!("A better pair");
    let put = api
        .call_with(
            reqwest::Method::PUT,
            &path,
            "alice",
            Some(changed.clone()),
            Some(&rev),
        )
        .await;
    assert_eq!(put.status, 200, "{}", put.body);
    assert_eq!(put.body["title"], "A better pair");
    assert_ne!(put.etag.as_deref(), Some(rev.as_str()));

    // the old revision is stale
    let stale = api
        .call_with(
            reqwest::Method::PUT,
            &path,
            "alice",
            Some(changed),
            Some(&rev),
        )
        .await;
    assert_eq!(stale.status, 409);
    assert_eq!(stale.body["code"], "stale_revision");
    let stale_del = api
        .call_with(reqwest::Method::DELETE, &path, "alice", None, Some(&rev))
        .await;
    assert_eq!(stale_del.status, 409);

    let del = api
        .call_with(
            reqwest::Method::DELETE,
            &path,
            "alice",
            None,
            put.etag.as_deref(),
        )
        .await;
    assert_eq!(del.status, 204);
    assert_eq!(api.get(&path, "alice").await.status, 404);
}

#[tokio::test]
async fn a_rule_of_the_page_is_a_400_that_says_where() {
    let api = start().await;
    let mut one = team("open", "coder");
    one["members"] = json!([one["members"][0].clone()]);
    let r = api.call(post(), "/api/teams", "alice", Some(one)).await;
    assert_eq!(r.status, 400, "{}", r.body);
    assert_eq!(r.body["at"], "members");
    let r = api
        .call(post(), "/api/teams", "alice", Some(json!({"name": 1})))
        .await;
    assert_eq!(r.status, 400);
    assert_eq!(r.body["code"], "bad_request");
}

#[tokio::test]
async fn config_says_the_bounds() {
    let api = start().await;
    let r = api.get("/api/config", "alice").await;
    assert_eq!(r.status, 200);
    assert_eq!(r.body["teams"]["limit"], 50);
    assert_eq!(r.body["teams"]["maxMembers"], 8);
    assert_eq!(r.body["teams"]["defaultMaxHandoffs"], 6);
    assert_eq!(r.body["teams"]["defaultPushTokens"], 24000);
}

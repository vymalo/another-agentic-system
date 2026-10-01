//! The platform's agent registry end to end (ADR 0022): the orchestrator over the real
//! `PlatformRegistry`, reading a linkset from an in-process server over HTTP, delegating to
//! in-process fake A2A agents over real HTTP, through the resource API and the AG-UI routes.
//!
//! The registry says which agents exist; each agent's own card says which releases it offers.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use orch_agent_a2a::{A2aAgentClient, A2aConfig};
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, Dispatcher};
use orch_ports::memory::{MemoryStore, MemoryWakeup};
use orch_ports::{
    CompositeRegistry, FixedRegistry, NoModel, OutboxStatus, PortSet, Ports as _, SystemClock,
    ThreadStore, UuidV7Ids,
};
use orch_registry_platform::{PROFILE, PlatformConfig, PlatformRegistry};
use orch_testsupport::{
    Chat, FakeAgent, FakeAgentOptions, FakeReleases, TestInstance, eventually, fast_dispatcher,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    A2aAgentClient,
    SystemClock,
    UuidV7Ids,
    NoModel,
    CompositeRegistry<FixedRegistry, PlatformRegistry>,
>;

const ALICE: &str = "alice@example.com";

/// A platform: serves the linkset of `agent-registry/v1`, which the test changes.
struct Platform {
    items: Arc<Mutex<Vec<Value>>>,
    down: Arc<AtomicBool>,
    url: String,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Platform {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[derive(Clone)]
struct PlatformState {
    items: Arc<Mutex<Vec<Value>>>,
    down: Arc<AtomicBool>,
}

async fn linkset(State(state): State<PlatformState>) -> axum::response::Response {
    if state.down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let body = json!({"linkset": [{
        "anchor": "https://platform.example.com/registry/v1/agents",
        "profile": [{"href": PROFILE}],
        "item": *state.items.lock().unwrap(),
    }]});
    (
        [
            (header::CONTENT_TYPE, "application/linkset+json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body.to_string(),
    )
        .into_response()
}

impl Platform {
    async fn start() -> Platform {
        let state = PlatformState {
            items: Arc::new(Mutex::new(Vec::new())),
            down: Arc::new(AtomicBool::new(false)),
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "http://{}/registry/v1/agents",
            listener.local_addr().unwrap()
        );
        let router = Router::new()
            .route("/registry/v1/agents", get(linkset))
            .with_state(state.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Platform {
            items: state.items,
            down: state.down,
            url,
            server,
        }
    }

    /// The platform lists `agent` (served at `card_url`).
    fn list(&self, service: &str, title: &str, card_url: &str, tags: &[&str]) {
        let mut item = json!({"href": card_url, "type": "application/json",
                              "title": title, "service": [service]});
        if !tags.is_empty() {
            item["tags"] = json!(tags);
        }
        self.items.lock().unwrap().push(item);
    }

    fn unlist(&self, service: &str) {
        self.items
            .lock()
            .unwrap()
            .retain(|item| item["service"][0] != service);
    }

    fn set_down(&self, down: bool) {
        self.down.store(down, Ordering::SeqCst);
    }
}

/// The orchestrator: one static agent (`plain`), the platform's registry beside it.
struct World {
    app: Arc<App<Stack>>,
    platform: Platform,
    plain: FakeAgent,
    /// The agent the platform lists as `platform-coder`; it offers release channels.
    coder: FakeAgent,
}

async fn world() -> World {
    let platform = Platform::start().await;
    let plain = FakeAgent::spawn(FakeAgentOptions::default()).await;
    let coder = FakeAgent::spawn(FakeAgentOptions {
        releases: Some(FakeReleases::sample()),
        ..FakeAgentOptions::default()
    })
    .await;
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: plain.endpoint("plain", None),
        name: "Plain".to_owned(),
    }]);
    let registry = PlatformRegistry::new(
        PlatformConfig::new(platform.url.clone())
            .without_system_proxy()
            .with_timeout(Duration::from_secs(2)),
    )
    .unwrap();
    let app = Arc::new(
        App::new(
            PortSet {
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: A2aAgentClient::new(A2aConfig {
                    use_system_proxy: false,
                    ..A2aConfig::default()
                })
                .unwrap(),
                clock: SystemClock,
                ids: UuidV7Ids,
                model: NoModel,
                registry: CompositeRegistry::new(directory.fixed_registry(), registry),
            },
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    );
    World {
        app,
        platform,
        plain,
        coder,
    }
}

fn ids(agents: &Value) -> Vec<&str> {
    agents
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect()
}

#[tokio::test]
async fn an_agent_the_platform_adds_is_listed_with_its_cards_releases_and_runs_without_a_restart() {
    let w = world().await;
    let instance = TestInstance::spawn(
        Arc::clone(&w.app),
        ApiConfig::default(),
        fast_dispatcher(),
        "e2e",
    )
    .await;
    let chat = Chat::new(&instance.base_url, ALICE);

    let (status, agents) = chat.get("/api/agents").await;
    assert_eq!(status, 200);
    assert_eq!(ids(&agents), ["plain"]);
    assert_eq!(agents[0]["source"], "static");

    // The platform provisions an agent: it is in the picker on the next request, no restart.
    w.platform.list(
        "platform-coder",
        "Platform coder",
        &w.coder.card_url(),
        &["coding", "git"],
    );
    let (_, agents) = chat.get("/api/agents").await;
    assert_eq!(ids(&agents), ["plain", "platform-coder"]);
    let coder = &agents[1];
    assert_eq!(coder["name"], "Platform coder");
    assert_eq!(coder["source"], "registry");
    assert_eq!(coder["tags"], json!(["coding", "git"]));
    assert_eq!(coder["cardUrl"], w.coder.card_url());
    // The registry lists no releases: they come from the agent's own card, read live.
    assert_eq!(coder["releases"]["defaultChannel"], "production");
    assert_eq!(coder["releases"]["channels"]["staging"], "coder-r51");
    assert!(agents[0].get("releases").is_none());

    // It is described and can be run, on a release its card offers.
    let (status, capabilities) = chat.agui_capabilities("platform-coder").await;
    assert_eq!(status, 200);
    assert_eq!(capabilities["identity"]["name"], "Platform coder");
    let thread = chat
        .create_thread("platform-coder", "echo hello", Some("staging"))
        .await;
    chat.wait_state(&thread, "done").await;
    let executed = w.coder.executions();
    assert_eq!(executed.len(), 1);
    assert!(w.plain.executions().is_empty());

    // The registry's agents are a source that answered.
    let (_, registry) = chat.get("/api/registry").await;
    assert_eq!(
        registry,
        json!({"sources": [
            {"name": "static", "status": "ok"},
            {"name": "platform", "status": "ok"}
        ]})
    );

    // The platform takes the agent away: it is gone from the list and a run says so.
    w.platform.unlist("platform-coder");
    let (_, agents) = chat.get("/api/agents").await;
    assert_eq!(ids(&agents), ["plain"]);
    let (status, _) = chat
        .try_create_thread("platform-coder", "echo again", None)
        .await;
    assert_eq!(status, 404);
    instance.shutdown().await;
}

#[tokio::test]
async fn a_registry_that_is_down_leaves_the_static_agents_and_the_ui_can_say_so() {
    let w = world().await;
    w.platform
        .list("platform-coder", "Platform coder", &w.coder.card_url(), &[]);
    let instance = TestInstance::spawn(
        Arc::clone(&w.app),
        ApiConfig::default(),
        fast_dispatcher(),
        "e2e",
    )
    .await;
    let chat = Chat::new(&instance.base_url, ALICE);
    assert_eq!(
        ids(&chat.get("/api/agents").await.1),
        ["plain", "platform-coder"]
    );

    w.platform.set_down(true);
    let (status, agents) = chat.get("/api/agents").await;
    assert_eq!(status, 200, "the list still answers");
    assert_eq!(ids(&agents), ["plain"], "none of the registry's agents");
    let (_, registry) = chat.get("/api/registry").await;
    assert_eq!(
        registry,
        json!({"sources": [
            {"name": "static", "status": "ok"},
            {"name": "platform", "status": "unavailable",
             "detail": "the registry could not be reached"}
        ]})
    );

    // A registry agent cannot be said to exist, or not: 503, never a 404, on every route.
    let (status, problem) = chat
        .try_create_thread("platform-coder", "echo hello", None)
        .await;
    assert_eq!(status, 503, "{problem}");
    assert_eq!(problem["detail"], "the agent registry is unreachable");
    let response = chat
        .agui_post(
            "nobody",
            &Chat::agui_input(
                &uuid::Uuid::now_v7().to_string(),
                "run-1",
                &[("msg-1", "echo hello")],
                json!({}),
            ),
        )
        .await;
    assert_eq!(response.status().as_u16(), 503);
    assert!(response.headers().contains_key("retry-after"));
    let (status, _) = chat.agui_capabilities("platform-coder").await;
    assert_eq!(status, 503);

    // The static agent works as if nothing happened.
    let thread = chat.create_thread("plain", "echo hello", None).await;
    chat.wait_state(&thread, "done").await;

    // And the registry is read again as soon as it answers.
    w.platform.set_down(false);
    assert_eq!(
        ids(&chat.get("/api/agents").await.1),
        ["plain", "platform-coder"]
    );
    let thread = chat
        .create_thread("platform-coder", "echo hello", None)
        .await;
    chat.wait_state(&thread, "done").await;
    instance.shutdown().await;
}

#[tokio::test]
async fn a_delegation_waits_for_a_registry_that_is_down_and_is_served_when_it_is_back() {
    let w = world().await;
    w.platform
        .list("platform-coder", "Platform coder", &w.coder.card_url(), &[]);
    // The API only: no dispatcher yet, so the message is accepted and waits in the outbox.
    let instance =
        TestInstance::spawn_with(Arc::clone(&w.app), ApiConfig::default(), None, "e2e-api").await;
    let chat = Chat::new(&instance.base_url, ALICE);
    let thread = chat
        .create_thread("platform-coder", "echo wait", None)
        .await;
    let id: orch_core::ThreadId = thread.parse().unwrap();
    let store = w.app.ports().store();
    let row_id = store.list_open_outbox(id).await.unwrap()[0].id;

    // The registry goes down, and a dispatcher starts: the row is tried again and is never dead.
    w.platform.set_down(true);
    let token = CancellationToken::new();
    let dispatcher = tokio::spawn(
        Dispatcher::new(Arc::clone(&w.app), fast_dispatcher(), "e2e-dispatcher").run(token.clone()),
    );
    let row = eventually("the row has been tried again", || async {
        let row = store.get_outbox(row_id).await.unwrap().unwrap();
        (row.attempts >= 3).then_some(row)
    })
    .await;
    assert_ne!(row.status, OutboxStatus::Dead);
    assert_eq!(chat.state(&thread).await, "queued");
    assert!(w.coder.executions().is_empty(), "nothing reached the agent");

    // The platform answers again, and the message goes through.
    w.platform.set_down(false);
    chat.wait_state(&thread, "done").await;
    assert_eq!(w.coder.executions().len(), 1);
    token.cancel();
    dispatcher.await.unwrap();
    instance.shutdown().await;
}

#[tokio::test]
async fn a_delegation_to_an_agent_the_platform_no_longer_lists_is_dead_with_the_reason() {
    let w = world().await;
    w.platform
        .list("platform-coder", "Platform coder", &w.coder.card_url(), &[]);
    let instance =
        TestInstance::spawn_with(Arc::clone(&w.app), ApiConfig::default(), None, "e2e-api").await;
    let chat = instance.chat(ALICE);
    let thread = chat
        .create_thread("platform-coder", "echo gone", None)
        .await;

    // The platform removes the agent before the message is delivered; it answers, without it.
    w.platform.unlist("platform-coder");
    let token = CancellationToken::new();
    let dispatcher = tokio::spawn(
        Dispatcher::new(Arc::clone(&w.app), fast_dispatcher(), "e2e-dispatcher").run(token.clone()),
    );
    chat.wait_state(&thread, "failed").await;
    let events = chat.events(&thread).await;
    assert!(
        events
            .iter()
            .any(|e| e["data"]["message"] == "agent 'platform-coder' is no longer listed"),
        "{events:?}"
    );
    assert!(w.coder.executions().is_empty());
    token.cancel();
    dispatcher.await.unwrap();
    instance.shutdown().await;
}

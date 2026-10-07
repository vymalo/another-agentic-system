//! The HTTP edge without any interaction surface, and with test surfaces mounted: the resource
//! API and health are always there, a surface's routes exist only when mounted, and everything
//! mounted sits behind the identity layer.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::Router;
use axum::routing::get;
use orch_api::{ApiConfig, SurfaceRoutes};
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, NewThread};
use orch_auth_header::HeaderAuth;
use orch_core::{AgentId, AgentTarget, ThreadId, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, PortSet, Principal, SystemClock};
use tokio::task::JoinHandle;

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    orch_ports::NoModel,
    orch_ports::FixedRegistry,
    orch_auth_header::HeaderAuth,
>;

const ALICE: &str = "alice@example.com";

struct Edge {
    base: String,
    client: reqwest::Client,
    app: Arc<App<Stack>>,
    server: JoinHandle<()>,
}

impl Drop for Edge {
    fn drop(&mut self) {
        self.server.abort();
    }
}

/// The agent's bearer token: a secret the orchestrator holds, which no response may repeat.
const AGENT_BEARER: &str = "agent-bearer-0123456789-not-for-export";

fn new_app() -> Arc<App<Stack>> {
    new_app_with(HeaderAuth::new())
}

fn new_app_with(auth: HeaderAuth) -> Arc<App<Stack>> {
    let entry = AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("plain"),
            "https://plain.example.com/.well-known/agent-card.json".to_owned(),
            Some(AGENT_BEARER.to_owned()),
        ),
        name: "Plain".to_owned(),
    };
    let directory = AgentDirectory::new(vec![entry]);
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth,
                registry: directory.fixed_registry(),
            },
            directory,
            AppConfig::default(),
        )
        .expect("a valid gate"),
    )
}

async fn edge(cfg: ApiConfig, surfaces: Vec<SurfaceRoutes>) -> Edge {
    edge_with(cfg, surfaces, HeaderAuth::new()).await
}

/// The edge over an application whose authenticator is `auth` (a development user, say).
async fn edge_with(cfg: ApiConfig, surfaces: Vec<SurfaceRoutes>, auth: HeaderAuth) -> Edge {
    let app = new_app_with(auth);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = orch_api::router_with_surfaces(Arc::clone(&app), cfg, surfaces);
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Edge {
        base: format!("http://{addr}"),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        app,
        server,
    }
}

impl Edge {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        user: Option<&str>,
    ) -> reqwest::Response {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(user) = user {
            req = req.header("X-Auth-Request-Email", user);
        }
        req.send().await.unwrap()
    }

    async fn thread(&self) -> String {
        self.thread_as(ALICE).await
    }

    /// A thread of `user`, created through the application (the resource API cannot create one).
    async fn thread_as(&self, user: &str) -> String {
        let record = self
            .app
            .create_thread(
                &UserId::new(user),
                NewThread {
                    title: None,
                    target: AgentTarget {
                        agent_id: AgentId::new("plain"),
                        release: None,
                    },
                    text: "hello".to_owned(),
                },
            )
            .await
            .unwrap();
        record.id.to_string()
    }
}

/// A surface with plain routes (`/x/whoami` replies with the identity, `/x/slow-plain` after
/// `delay`) and one streaming route (`/x/slow-stream`, after `delay`).
fn test_surface(delay: Duration) -> SurfaceRoutes {
    async fn whoami(Extension(principal): Extension<Principal>) -> String {
        principal.user.as_str().to_owned()
    }
    let slow = move || async move {
        tokio::time::sleep(delay).await;
        "slow"
    };
    SurfaceRoutes::new()
        .plain(
            Router::new()
                .route("/x/whoami", get(whoami))
                .route("/x/slow-plain", get(slow)),
        )
        .streaming(Router::new().route("/x/slow-stream", get(slow)))
}

#[tokio::test]
async fn health_needs_no_identity_and_everything_else_does() {
    let e = edge(ApiConfig::default(), vec![]).await;
    for path in ["/healthz", "/readyz"] {
        assert_eq!(e.call(reqwest::Method::GET, path, None).await.status(), 200);
    }
    for path in ["/api/agents", "/api/threads", "/api/nothing"] {
        let r = e.call(reqwest::Method::GET, path, None).await;
        assert_eq!(r.status(), 401, "{path}");
        assert_eq!(
            r.headers()["content-type"],
            "application/problem+json",
            "{path}"
        );
    }
}

const METRICS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

#[tokio::test]
async fn metrics_report_the_outbox_and_need_no_identity() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let empty = e.call(reqwest::Method::GET, "/metrics", None).await;
    assert_eq!(empty.status(), 200);
    assert_eq!(empty.headers()["content-type"], METRICS_CONTENT_TYPE);
    let body = empty.text().await.unwrap();
    assert!(body.contains("# TYPE orch_outbox_rows gauge"), "{body}");
    assert!(body.contains("orch_outbox_rows{state=\"due\"} 0"), "{body}");

    // A new thread queues one delegation, due at once.
    e.thread().await;
    let body = e
        .call(reqwest::Method::GET, "/metrics", None)
        .await
        .text()
        .await
        .unwrap();
    assert!(body.contains("orch_outbox_rows{state=\"due\"} 1"), "{body}");
    assert!(
        body.contains("orch_outbox_rows{state=\"waiting\"} 0"),
        "{body}"
    );
    assert!(
        body.contains("orch_outbox_rows{state=\"leased\"} 0"),
        "{body}"
    );
    assert!(
        body.contains("# TYPE orch_outbox_oldest_due_age_seconds gauge"),
        "{body}"
    );
}

#[tokio::test]
async fn the_health_router_serves_health_only_and_needs_no_identity() {
    let app = new_app();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = orch_api::health_router(Arc::clone(&app));
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let get = |path: &'static str| {
        let client = client.clone();
        async move {
            client
                .get(format!("http://{addr}{path}"))
                .send()
                .await
                .unwrap()
        }
    };

    // No identity header, and it answers like the full router does.
    let healthz = get("/healthz").await;
    assert_eq!(healthz.status(), 200);
    assert!(healthz.headers().contains_key("x-request-id"));
    assert_eq!(healthz.text().await.unwrap(), "ok");
    let readyz = get("/readyz").await;
    assert_eq!(readyz.status(), 200);
    assert_eq!(readyz.text().await.unwrap(), "ready");

    // The queue metrics are served too, like on the full router.
    let metrics = get("/metrics").await;
    assert_eq!(metrics.status(), 200);
    assert_eq!(metrics.headers()["content-type"], METRICS_CONTENT_TYPE);
    assert!(
        metrics
            .text()
            .await
            .unwrap()
            .contains("# TYPE orch_outbox_rows gauge")
    );

    // Nothing else is served: no resource API, and no identity check to answer 401 with.
    for path in ["/api/agents", "/api/threads", "/api/nothing"] {
        assert_eq!(get(path).await.status(), 404, "{path}");
    }

    // The same state machine: not ready, then shutting down.
    app.set_ready(false);
    assert_eq!(get("/readyz").await.status(), 503);
    app.set_shutting_down();
    assert_eq!(get("/healthz").await.status(), 503);
    server.abort();
}

#[tokio::test]
async fn the_resource_api_is_served_without_any_surface() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread().await;

    let agents = e
        .call(reqwest::Method::GET, "/api/agents", Some(ALICE))
        .await;
    assert_eq!(agents.status(), 200);
    let agents: serde_json::Value = agents.json().await.unwrap();
    assert_eq!(agents[0]["id"], "plain");

    let list = e
        .call(reqwest::Method::GET, "/api/threads", Some(ALICE))
        .await;
    assert_eq!(list.status(), 200);
    let list: serde_json::Value = list.json().await.unwrap();
    assert_eq!(list[0]["id"], id.as_str());

    let one = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(one.status(), 200);
    // Someone else's thread does not exist.
    let other = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}"),
            Some("bob@example.com"),
        )
        .await;
    assert_eq!(other.status(), 404);

    let cancel = e
        .call(
            reqwest::Method::POST,
            &format!("/api/threads/{id}/cancel"),
            Some(ALICE),
        )
        .await;
    assert_eq!(cancel.status(), 202);
}

/// The four operations of the legacy chat API (`createThread`, `postMessage`, `listEvents`,
/// `streamEvents`) were removed on 2026-09-30 (ADR 0012). The resource API does not serve them,
/// and only a surface can add interaction routes, so with no surface, or with one that mounts
/// something else, they match nothing.
#[tokio::test]
async fn the_legacy_interaction_routes_are_gone() {
    for surfaces in [vec![], vec![test_surface(Duration::ZERO)]] {
        let mounted = surfaces.len();
        let e = edge(ApiConfig::default(), surfaces).await;
        let id = e.thread().await;
        for (method, path) in [
            (reqwest::Method::GET, format!("/api/threads/{id}/events")),
            (reqwest::Method::GET, format!("/api/threads/{id}/stream")),
            (reqwest::Method::POST, format!("/api/threads/{id}/messages")),
        ] {
            let r = e.call(method, &path, Some(ALICE)).await;
            assert_eq!(r.status(), 404, "{path} ({mounted} surfaces)");
            assert!(r.headers().get("deprecation").is_none(), "{path}");
            let body: serde_json::Value = r.json().await.unwrap();
            assert_eq!(
                body["detail"], "no such route",
                "{path} ({mounted} surfaces)"
            );
        }
        // `POST /api/threads` shares its path with the served `GET /api/threads`, so the router
        // can only say the method is not allowed.
        let r = e
            .call(reqwest::Method::POST, "/api/threads", Some(ALICE))
            .await;
        assert_eq!(r.status(), 405, "POST /api/threads ({mounted} surfaces)");
        // The resource API beside them is served.
        let r = e
            .call(
                reqwest::Method::GET,
                &format!("/api/threads/{id}"),
                Some(ALICE),
            )
            .await;
        assert_eq!(r.status(), 200);
        let r = e
            .call(reqwest::Method::GET, "/api/threads", Some(ALICE))
            .await;
        assert_eq!(r.status(), 200);
    }
}

#[tokio::test]
async fn a_mounted_surface_sits_behind_the_identity_layer() {
    let e = edge(ApiConfig::default(), vec![test_surface(Duration::ZERO)]).await;
    for path in ["/x/whoami", "/x/slow-plain", "/x/slow-stream"] {
        let r = e.call(reqwest::Method::GET, path, None).await;
        assert_eq!(r.status(), 401, "{path} must fail closed");
    }
    let r = e.call(reqwest::Method::GET, "/x/whoami", Some(ALICE)).await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.text().await.unwrap(),
        ALICE,
        "the handler sees the identity"
    );

    // A surface does not weaken the malformed-header rule, even with a dev user configured.
    let e = edge_with(
        ApiConfig::default(),
        vec![test_surface(Duration::ZERO)],
        HeaderAuth::new().with_dev_user(UserId::new("dev@example.com")),
    )
    .await;
    assert_eq!(
        e.call(reqwest::Method::GET, "/x/whoami", None)
            .await
            .text()
            .await
            .unwrap(),
        "dev@example.com"
    );
    let r = e
        .call(reqwest::Method::GET, "/x/whoami", Some("no-at-sign"))
        .await;
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn streaming_routes_are_exempt_from_the_request_timeout() {
    let cfg = ApiConfig {
        request_timeout: Duration::from_millis(50),
        ..ApiConfig::default()
    };
    let e = edge(cfg, vec![test_surface(Duration::from_millis(300))]).await;
    let plain = e
        .call(reqwest::Method::GET, "/x/slow-plain", Some(ALICE))
        .await;
    assert_eq!(plain.status(), 503, "plain routes time out");
    let stream = e
        .call(reqwest::Method::GET, "/x/slow-stream", Some(ALICE))
        .await;
    assert_eq!(stream.status(), 200, "streaming routes do not");
}

#[tokio::test]
async fn several_surfaces_merge_into_one_router() {
    let second =
        SurfaceRoutes::new().plain(Router::new().route("/y/ping", get(|| async { "pong" })));
    let e = edge(
        ApiConfig::default(),
        vec![test_surface(Duration::ZERO), second],
    )
    .await;
    assert_eq!(
        e.call(reqwest::Method::GET, "/x/whoami", Some(ALICE))
            .await
            .status(),
        200
    );
    assert_eq!(
        e.call(reqwest::Method::GET, "/y/ping", Some(ALICE))
            .await
            .text()
            .await
            .unwrap(),
        "pong"
    );
}

// ---- identity and isolation of the resource API -------------------------------------------

#[tokio::test]
async fn no_identity_is_401_everywhere_except_the_probes() {
    let e = edge(ApiConfig::default(), vec![test_surface(Duration::ZERO)]).await;
    let id = e.thread().await;
    for (method, path) in [
        (reqwest::Method::GET, "/api/agents".to_owned()),
        (reqwest::Method::GET, "/api/threads".to_owned()),
        (reqwest::Method::GET, format!("/api/threads/{id}")),
        (reqwest::Method::POST, format!("/api/threads/{id}/cancel")),
        (reqwest::Method::GET, format!("/api/threads/{id}/export")),
        // Unknown paths, wrong methods and a surface's paths are refused the same way: nothing is
        // reachable anonymously. (The removed legacy routes are among the unknown ones.)
        (reqwest::Method::POST, "/api/threads".to_owned()),
        (reqwest::Method::GET, format!("/api/threads/{id}/events")),
        (reqwest::Method::GET, "/api/unknown".to_owned()),
        (reqwest::Method::GET, "/x/whoami".to_owned()),
        (reqwest::Method::GET, "/".to_owned()),
        (reqwest::Method::DELETE, format!("/api/threads/{id}")),
        (reqwest::Method::PUT, "/api/threads".to_owned()),
    ] {
        let r = e.call(method.clone(), &path, None).await;
        assert_eq!(r.status(), 401, "{method} {path}");
        assert_eq!(
            r.headers()["content-type"],
            "application/problem+json",
            "{method} {path}"
        );
        let p: serde_json::Value = r.json().await.unwrap();
        assert_eq!(p["status"], 401);
        assert_eq!(p["title"], "Unauthorized");
        // The web shows this detail: it names the header the proxy did not send.
        assert_eq!(
            p["detail"], "missing X-Auth-Request-Email",
            "{method} {path}"
        );
    }
    // Nothing was changed by the anonymous requests: the thread is still queued.
    let record = e
        .app
        .get_thread(&UserId::new(ALICE), id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(record.state.as_str(), "queued");
}

#[tokio::test]
async fn blank_or_malformed_identity_is_401() {
    let e = edge(ApiConfig::default(), vec![]).await;
    for value in ["", "   ", "not-an-email"] {
        let r = e
            .call(reqwest::Method::GET, "/api/agents", Some(value))
            .await;
        assert_eq!(r.status(), 401, "{value:?}");
    }
}

#[tokio::test]
async fn a_user_cannot_see_another_users_thread() {
    const BOB: &str = "bob@example.com";
    const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread().await;
    for (method, path) in [
        (reqwest::Method::GET, format!("/api/threads/{id}")),
        (reqwest::Method::GET, format!("/api/threads/{id}/export")),
        (reqwest::Method::POST, format!("/api/threads/{id}/cancel")),
    ] {
        let r = e.call(method.clone(), &path, Some(BOB)).await;
        assert_eq!(r.status(), 404, "{method} {path}");
        assert!(
            r.headers()["content-type"]
                .to_str()
                .unwrap()
                .starts_with("application/problem+json")
        );
    }
    // The same answer as for a thread that does not exist.
    let ghost = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{RANDOM}"),
            Some(BOB),
        )
        .await;
    let foreign = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}"),
            Some(BOB),
        )
        .await;
    assert_eq!(ghost.status(), foreign.status());
    let (ghost, foreign): (serde_json::Value, serde_json::Value) =
        (ghost.json().await.unwrap(), foreign.json().await.unwrap());
    assert_eq!(ghost["title"], foreign["title"]);
    assert_eq!(ghost["detail"], foreign["detail"]);
    // Bob's list excludes it, and his own threads are his.
    let list = |user: &'static str| {
        let e = &e;
        async move {
            let r = e
                .call(reqwest::Method::GET, "/api/threads", Some(user))
                .await;
            r.json::<serde_json::Value>().await.unwrap()
        }
    };
    assert!(list(BOB).await.as_array().unwrap().is_empty());
    let bobs = e.thread_as(BOB).await;
    let bob_list = list(BOB).await;
    assert_eq!(bob_list.as_array().unwrap().len(), 1);
    assert_eq!(bob_list[0]["id"], bobs.as_str());
    // Alice still has hers untouched (Bob's cancel attempt did nothing).
    let record = e
        .app
        .get_thread(&UserId::new(ALICE), id.parse().unwrap())
        .await
        .unwrap();
    assert_eq!(record.state.as_str(), "queued");
    assert_eq!(list(ALICE).await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn identity_is_case_and_space_insensitive() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread_as("Alice@Example.COM").await;
    let r = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}"),
            Some("  alice@example.com "),
        )
        .await;
    assert_eq!(r.status(), 200);
    let events = e
        .app
        .list_events(
            &UserId::new("ALICE@example.com"),
            id.parse().unwrap(),
            0,
            10,
        )
        .await
        .unwrap();
    assert_eq!(events[0].actor.name, "alice@example.com");
}

#[tokio::test]
async fn dev_user_applies_only_when_configured() {
    let with = edge_with(
        ApiConfig::default(),
        vec![],
        HeaderAuth::new().with_dev_user(UserId::new("dev@example.com")),
    )
    .await;
    let r = with.call(reqwest::Method::GET, "/api/agents", None).await;
    assert_eq!(r.status(), 200);
    // The dev user owns what was created as the dev user and lists it without any header.
    let id = with.thread_as("dev@example.com").await;
    let r = with.call(reqwest::Method::GET, "/api/threads", None).await;
    let list: serde_json::Value = r.json().await.unwrap();
    assert_eq!(list[0]["id"], id.as_str());
    // A real identity still wins, and a malformed header is not papered over by the dev user.
    let r = with
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 404);
    let r = with
        .call(reqwest::Method::GET, "/api/agents", Some("garbage"))
        .await;
    assert_eq!(r.status(), 401);

    let without = edge(ApiConfig::default(), vec![]).await;
    let r = without
        .call(reqwest::Method::GET, "/api/agents", None)
        .await;
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn probes_report_readiness_and_shutdown_while_the_api_keeps_answering() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let status = |path: &'static str, user: Option<&'static str>| {
        let e = &e;
        async move { e.call(reqwest::Method::GET, path, user).await.status() }
    };
    assert_eq!(status("/readyz", None).await, 200);
    e.app.set_ready(false);
    assert_eq!(status("/readyz", None).await, 503);
    e.app.set_ready(true);
    assert_eq!(status("/readyz", None).await, 200);
    assert_eq!(status("/healthz", None).await, 200);
    e.app.set_shutting_down();
    assert_eq!(status("/healthz", None).await, 503);
    // The API keeps answering while draining.
    assert_eq!(status("/api/agents", Some(ALICE)).await, 200);
}

/// A machine route (ADR 0016, ADR 0019) is outside the identity layer and the request timeout,
/// and is guarded by the layer its surface hands over: there is no way to add one without.
#[tokio::test]
async fn a_machine_route_needs_its_own_guard_and_not_the_identity() {
    use axum::extract::Request;
    use axum::http::StatusCode;
    use axum::middleware::{Next, from_fn};
    use axum::response::{IntoResponse, Response};

    async fn guard(request: Request, next: Next) -> Response {
        match request.headers().get("x-machine-key") {
            Some(v) if v == "secret" => next.run(request).await,
            _ => StatusCode::FORBIDDEN.into_response(),
        }
    }
    async fn who(request: Request) -> String {
        // The identity layer did not run: there is no `Principal`, and the route never reads the header.
        format!(
            "machine, identity extension: {}",
            request.extensions().get::<Principal>().is_some()
        )
    }
    let machine =
        SurfaceRoutes::new().machine(Router::new().route("/m/who", get(who)), from_fn(guard));
    let cfg = ApiConfig {
        // The plain timeout would cut a slow call; a machine route is not subject to it.
        request_timeout: Duration::from_millis(50),
        ..ApiConfig::default()
    };
    let e = edge_with(
        cfg,
        vec![test_surface(Duration::ZERO), machine],
        HeaderAuth::new().with_dev_user(UserId::new("dev@example.com")),
    )
    .await;
    let call = |key: Option<&'static str>, user: Option<&'static str>| {
        let mut req = e.client.get(format!("{}/m/who", e.base));
        if let Some(key) = key {
            req = req.header("x-machine-key", key);
        }
        if let Some(user) = user {
            req = req.header("X-Auth-Request-Email", user);
        }
        req.send()
    };
    // The guard decides: with the key, no identity is needed; without it, an identity does not help.
    let ok = call(Some("secret"), None).await.unwrap();
    assert_eq!(ok.status(), 200);
    assert_eq!(
        ok.text().await.unwrap(),
        "machine, identity extension: false"
    );
    for (key, user) in [
        (None, None),
        (Some("wrong"), Some(ALICE)),
        (None, Some(ALICE)),
    ] {
        assert_eq!(
            call(key, user).await.unwrap().status(),
            403,
            "{key:?} {user:?}"
        );
    }
    // Everything else is still behind the identity layer (here the dev user answers for it).
    let r = e.call(reqwest::Method::GET, "/x/whoami", None).await;
    assert_eq!(r.text().await.unwrap(), "dev@example.com");
    let r = e.call(reqwest::Method::GET, "/api/agents", None).await;
    assert_eq!(r.status(), 200);
}

// ---- the thread export --------------------------------------------------------------------

const BOB: &str = "bob@example.com";
const RANDOM: &str = "0190aaaa-0000-7000-8000-000000000123";

#[tokio::test]
async fn the_export_is_one_versioned_attachment_with_the_thread_its_job_and_the_log() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread().await;
    let r = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}/export"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 200);
    let h = r.headers().clone();
    assert_eq!(h["content-type"], "application/json");
    assert_eq!(
        h["content-disposition"],
        format!("attachment; filename=\"thread-{id}.json\"").as_str()
    );
    assert_eq!(h["cache-control"], "no-store");
    let body = r.text().await.unwrap();
    let doc: serde_json::Value = serde_json::from_str(&body).unwrap();

    assert_eq!(doc["format"], "another-agentic-system/thread-export");
    assert_eq!(doc["version"], 1);
    assert_eq!(doc["format"], orch_api::EXPORT_FORMAT);
    assert_eq!(doc["version"], orch_api::EXPORT_VERSION);
    let at: jiff::Timestamp = doc["exportedAt"].as_str().unwrap().parse().unwrap();
    assert!(at.as_second() > 0);
    // `thread` is what GET /api/threads/{id} says.
    let got = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}"),
            Some(ALICE),
        )
        .await;
    assert_eq!(
        doc["thread"],
        got.json::<serde_json::Value>().await.unwrap()
    );
    // The whole ledger is there even without a gate, which `thread` hides.
    assert!(doc["thread"].get("job").is_none());
    assert_eq!(doc["job"]["attempt"], 1);
    assert!(doc["job"]["gate"]["require"].as_array().unwrap().is_empty());
    assert_eq!(doc["binding"]["agentId"], "plain");
    // the agent assigns the context with its first answer (ADR 0055): nothing was sent yet
    assert!(doc["binding"]["contextId"].is_null(), "{doc}");
    assert_eq!(doc["eventsTruncated"], false);
    let events = doc["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(events[0]["threadId"], id.as_str());
    assert_eq!(events[0]["kind"], "user_message");
    assert_eq!(events[0]["data"]["text"], "hello");
    // It is meant to be opened by a person: indented.
    assert!(body.contains("\n  \"events\""), "{body}");
}

#[tokio::test]
async fn the_export_holds_the_log_in_order_and_no_credential_of_the_orchestrator() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread().await;
    let thread: ThreadId = id.parse().unwrap();
    for i in 0..600 {
        e.app
            .post_message(&UserId::new(ALICE), thread, format!("message {i}"))
            .await
            .unwrap();
    }
    let r = e
        .call(
            reqwest::Method::GET,
            &format!("/api/threads/{id}/export"),
            Some(ALICE),
        )
        .await;
    let body = r.text().await.unwrap();
    // The agent's bearer token is configured on the agent this thread targets; it must not be
    // anywhere in the file, nor the identity header's name.
    assert!(
        !body.contains(AGENT_BEARER),
        "the agent's bearer token leaked"
    );
    let doc: serde_json::Value = serde_json::from_str(&body).unwrap();
    let seqs: Vec<i64> = doc["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|ev| ev["seq"].as_i64().unwrap())
        .collect();
    assert_eq!(seqs, (1..=601).collect::<Vec<i64>>());
    assert_eq!(doc["thread"]["lastSeq"], 601);
    // The owner's address is in the log, as the actor of their messages, and is the thread's
    // `owner`, as in `getThread` (which an administrator reading others' threads needs).
    assert_eq!(doc["events"][0]["actor"]["name"], ALICE);
    assert_eq!(doc["thread"]["owner"], ALICE);
}

#[tokio::test]
async fn the_export_is_the_owners_alone_and_unknown_threads_are_404() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread().await;
    let get = |path: String, user: Option<&'static str>| {
        let e = &e;
        async move { e.call(reqwest::Method::GET, &path, user).await }
    };
    let foreign = get(format!("/api/threads/{id}/export"), Some(BOB)).await;
    let ghost = get(format!("/api/threads/{RANDOM}/export"), Some(ALICE)).await;
    let junk = get("/api/threads/not-a-uuid/export".to_owned(), Some(ALICE)).await;
    for r in [&foreign, &ghost, &junk] {
        assert_eq!(r.status(), 404);
        assert_eq!(r.headers()["content-type"], "application/problem+json");
        assert!(r.headers().get("content-disposition").is_none());
    }
    // Someone else's thread reads exactly like one that does not exist.
    let (foreign, ghost): (serde_json::Value, serde_json::Value) =
        (foreign.json().await.unwrap(), ghost.json().await.unwrap());
    assert_eq!(foreign, ghost);
    // No identity, and a malformed one, are refused before anything is read.
    let anonymous = get(format!("/api/threads/{id}/export"), None).await;
    assert_eq!(anonymous.status(), 401);
    let malformed = get(format!("/api/threads/{id}/export"), Some("not-an-email")).await;
    assert_eq!(malformed.status(), 401);
    // Only GET is served.
    let r = e
        .call(
            reqwest::Method::POST,
            &format!("/api/threads/{id}/export"),
            Some(ALICE),
        )
        .await;
    assert_eq!(r.status(), 405);
}

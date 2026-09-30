//! The HTTP edge without any interaction surface, and with test surfaces mounted: the resource
//! API and health are always there, a surface's routes exist only when mounted, and everything
//! mounted sits behind the identity layer.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::Router;
use axum::routing::get;
use orch_api::{ApiConfig, AuthConfig, SurfaceRoutes};
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, NewThread};
use orch_core::{AgentId, AgentTarget, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use tokio::task::JoinHandle;

type Stack = PortSet<MemoryStore, MemoryWakeup, ScriptedAgent, SystemClock, SeqIds>;

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

fn new_app() -> Arc<App<Stack>> {
    let entry = AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("plain"),
            "https://plain.example.com/.well-known/agent-card.json".to_owned(),
            None,
        ),
        name: "Plain".to_owned(),
    };
    Arc::new(App::new(
        PortSet {
            store: MemoryStore::new(),
            wakeup: MemoryWakeup::new(),
            agents: ScriptedAgent::new(),
            clock: SystemClock,
            ids: SeqIds::default(),
        },
        AgentDirectory::new(vec![entry]),
        AppConfig::default(),
    ))
}

async fn edge(cfg: ApiConfig, surfaces: Vec<SurfaceRoutes>) -> Edge {
    let app = new_app();
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
    async fn whoami(Extension(user): Extension<UserId>) -> String {
        user.as_str().to_owned()
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
    let cfg = ApiConfig {
        auth: AuthConfig {
            dev_user: Some(UserId::new("dev@example.com")),
        },
        ..ApiConfig::default()
    };
    let e = edge(cfg, vec![test_surface(Duration::ZERO)]).await;
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
    let cfg = ApiConfig {
        auth: AuthConfig {
            dev_user: Some(UserId::new("dev@example.com")),
        },
        ..ApiConfig::default()
    };
    let with = edge(cfg, vec![]).await;
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

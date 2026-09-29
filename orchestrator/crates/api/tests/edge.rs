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
        endpoint: AgentEndpoint {
            id: AgentId::new("plain"),
            card_url: "https://plain.example.com/.well-known/agent-card.json".to_owned(),
            bearer: None,
        },
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
        let record = self
            .app
            .create_thread(
                &UserId::new(ALICE),
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

#[tokio::test]
async fn interaction_routes_exist_only_when_a_surface_mounts_them() {
    let e = edge(ApiConfig::default(), vec![]).await;
    let id = e.thread().await;
    for (method, path) in [
        (reqwest::Method::GET, format!("/api/threads/{id}/events")),
        (reqwest::Method::GET, format!("/api/threads/{id}/stream")),
        (reqwest::Method::POST, format!("/api/threads/{id}/messages")),
    ] {
        let r = e.call(method, &path, Some(ALICE)).await;
        assert_eq!(r.status(), 404, "{path}");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["detail"], "no such route", "{path}");
    }
    // `POST /api/threads` shares its path with the served `GET /api/threads`, so the router
    // can only say the method is not allowed.
    let r = e
        .call(reqwest::Method::POST, "/api/threads", Some(ALICE))
        .await;
    assert_eq!(r.status(), 405);
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

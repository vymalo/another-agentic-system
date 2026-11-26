//! CBOR beside JSON on the resource API (ADR 0100): the same request in both encodings answers
//! equal values, and the refusals are the documented ones.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig, NewThread};
use orch_auth_header::HeaderAuth;
use orch_core::{AgentId, AgentTarget, UserId};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, PortSet, SystemClock};
use std::sync::Arc;
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
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            AppConfig::default(),
        )
        .expect("a valid gate"),
    )
}

async fn edge() -> Edge {
    let app = new_app();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), vec![]);
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
    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .header("X-Auth-Request-Email", ALICE)
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

fn cbor_of(value: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).unwrap();
    out
}

async fn as_value(r: reqwest::Response) -> (u16, String, String, serde_json::Value) {
    let status = r.status().as_u16();
    let content_type = r.headers()["content-type"].to_str().unwrap().to_owned();
    let vary = r
        .headers()
        .get_all("vary")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect::<Vec<_>>()
        .join(",");
    let bytes = r.bytes().await.unwrap();
    let value = if content_type.contains("cbor") {
        ciborium::from_reader(bytes.as_ref()).unwrap()
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, content_type, vary, value)
}

/// Drops what legitimately differs between two answers (ids and instants of separate threads).
fn same_shape(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    use serde_json::Value::{Array, Object};
    match (a, b) {
        (Object(x), Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_shape(v, w)))
        }
        (Array(x), Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same_shape(v, w))
        }
        (serde_json::Value::String(_), serde_json::Value::String(_)) => true,
        _ => a == b,
    }
}

#[tokio::test]
async fn a_read_answers_equal_values_in_both_encodings() {
    let e = edge().await;
    let id = e.thread().await;
    for path in [
        "/api/agents".to_owned(),
        "/api/threads".to_owned(),
        format!("/api/threads/{id}"),
    ] {
        let (s1, t1, v1, json) =
            as_value(e.req(reqwest::Method::GET, &path).send().await.unwrap()).await;
        let (s2, t2, v2, cbor) = as_value(
            e.req(reqwest::Method::GET, &path)
                .header("Accept", "application/cbor")
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!((s1, s2), (200, 200), "{path}");
        assert_eq!(t1, "application/json", "{path}");
        assert_eq!(t2, "application/cbor", "{path}");
        assert!(v1.contains("Accept") && v2.contains("Accept"), "{path}");
        assert_eq!(json, cbor, "{path}");
    }
}

#[tokio::test]
async fn an_error_is_the_same_problem_in_both_encodings() {
    let e = edge().await;
    let path = "/api/threads/00000000-0000-0000-0000-000000000000";
    let (s1, t1, _, json) = as_value(e.req(reqwest::Method::GET, path).send().await.unwrap()).await;
    let (s2, t2, _, cbor) = as_value(
        e.req(reqwest::Method::GET, path)
            .header("Accept", "application/cbor")
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(s1, 404);
    assert_eq!(s1, s2);
    assert_eq!(t1, "application/problem+json");
    assert_eq!(t2, "application/problem+cbor");
    assert_eq!(json, cbor);
}

#[tokio::test]
async fn a_write_in_cbor_makes_the_same_change_as_in_json() {
    let e = edge().await;
    let (a, b) = (e.thread().await, e.thread().await);
    let body = serde_json::json!({ "title": "A title" });
    let r = e
        .req(reqwest::Method::PATCH, &format!("/api/threads/{a}"))
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let (sj, _, _, json) = as_value(r).await;
    let r = e
        .req(reqwest::Method::PATCH, &format!("/api/threads/{b}"))
        .header("Content-Type", "application/cbor")
        .header("Accept", "application/cbor")
        .body(cbor_of(&body))
        .send()
        .await
        .unwrap();
    let (sc, tc, _, cbor) = as_value(r).await;
    assert_eq!(sj, sc, "{json} / {cbor}");
    assert_eq!(tc, "application/cbor");
    assert!(same_shape(&json, &cbor), "{json} / {cbor}");
    // the thread holds the same title for both
    let ta = as_value(
        e.req(reqwest::Method::GET, &format!("/api/threads/{a}"))
            .send()
            .await
            .unwrap(),
    )
    .await
    .3;
    let tb = as_value(
        e.req(reqwest::Method::GET, &format!("/api/threads/{b}"))
            .send()
            .await
            .unwrap(),
    )
    .await
    .3;
    assert_eq!(ta["title"], "A title");
    assert_eq!(ta["title"], tb["title"]);
}

#[tokio::test]
async fn the_refusals_are_the_documented_ones() {
    let e = edge().await;
    let id = e.thread().await;
    let path = format!("/api/threads/{id}");
    // 406: an Accept that lists neither
    let r = e
        .req(reqwest::Method::GET, "/api/agents")
        .header("Accept", "text/html")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 406);
    assert_eq!(r.headers()["content-type"], "application/problem+json");
    // 406 on a write happens before the write
    let r = e
        .req(reqwest::Method::PATCH, &path)
        .header("Accept", "text/html")
        .header("Content-Type", "application/json")
        .body(r#"{"title":"x"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 406);
    // malformed CBOR, an indefinite-length item and a body too deep are 400
    for body in [vec![0xa1u8, 0x61], vec![0x9f, 0x01, 0xff], vec![0x81; 200]] {
        let r = e
            .req(reqwest::Method::PATCH, &path)
            .header("Content-Type", "application/cbor")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 400);
        assert_eq!(r.headers()["content-type"], "application/problem+json");
    }
    // a body over the limit is 413
    let r = e
        .req(reqwest::Method::PATCH, &path)
        .header("Content-Type", "application/cbor")
        .body(vec![0u8; 2 * 1024 * 1024])
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 413);
    // an unsupported type is refused by the route, never read as CBOR or JSON
    let r = e
        .req(reqwest::Method::PATCH, &path)
        .header("Content-Type", "text/plain")
        .body("x")
        .send()
        .await
        .unwrap();
    assert!(matches!(r.status().as_u16(), 400 | 415), "{}", r.status());
}

#[tokio::test]
async fn an_event_stream_is_never_converted() {
    use axum::Router;
    use axum::routing::get;
    let app = new_app();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let surface = orch_api::SurfaceRoutes::new().streaming(Router::new().route(
        "/x/sse",
        get(|| async { ([("content-type", "text/event-stream")], "data: {}\n\n") }),
    ));
    let router = orch_api::router_with_surfaces(app, ApiConfig::default(), vec![surface]);
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let r = reqwest::Client::new()
        .get(format!("http://{addr}/x/sse"))
        .header("X-Auth-Request-Email", ALICE)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "text/event-stream");
    assert_eq!(r.text().await.unwrap(), "data: {}\n\n");
    server.abort();
}

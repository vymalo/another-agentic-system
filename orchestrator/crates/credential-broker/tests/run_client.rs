//! The orchestrator's client of the broker's run routes (ADR 0084 decisions 1 and 6): what it sends,
//! and what each answer becomes.

#![allow(clippy::expect_used, reason = "a test that cannot set up should panic")]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, post};
use orch_core::{AgentId, UserId};
use orch_credential_broker::{BuildError, RunBindingClient, deadline_in};
use orch_ports::{RunBinding, RunBindingError, RunBindings, RunId, ToolSecret};

const TOKEN: &str = "orchestrator-token";

/// What the stub saw.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

#[derive(Clone, Default)]
struct Stub {
    calls: Arc<Mutex<Vec<Call>>>,
    status: Arc<Mutex<u16>>,
}

impl Stub {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("a lock").clone()
    }

    fn answer(&self, status: u16) {
        *self.status.lock().expect("a lock") = status;
    }

    fn status(&self) -> u16 {
        *self.status.lock().expect("a lock")
    }
}

async fn record(stub: &Stub, method: &str, path: String, headers: HeaderMap, body: Bytes) {
    let authorization = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    stub.calls.lock().expect("a lock").push(Call {
        method: method.to_owned(),
        path,
        authorization,
        body: String::from_utf8_lossy(&body).into_owned(),
    });
}

async fn bind_route(State(stub): State<Stub>, headers: HeaderMap, body: Bytes) -> StatusCode {
    record(&stub, "POST", "/internal/runs".to_owned(), headers, body).await;
    StatusCode::from_u16(stub.status()).expect("a status")
}

async fn unbind_route(
    State(stub): State<Stub>,
    uri: axum::http::Uri,
    headers: HeaderMap,
) -> StatusCode {
    // The raw path, so the encoding the client chose is what the test sees.
    record(
        &stub,
        "DELETE",
        uri.path().to_owned(),
        headers,
        Bytes::new(),
    )
    .await;
    StatusCode::from_u16(stub.status()).expect("a status")
}

/// A stub broker on a loopback port, and the client that talks to it.
async fn started() -> (Stub, RunBindingClient) {
    let stub = Stub::default();
    stub.answer(204);
    let app = Router::new()
        .route("/internal/runs", post(bind_route))
        .route("/internal/runs/{run}", delete(unbind_route))
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let client = RunBindingClient::new(&format!("http://{address}"), ToolSecret::new(TOKEN))
        .expect("a client");
    (stub, client)
}

fn binding() -> RunBinding {
    RunBinding {
        run: RunId::new("run-1"),
        agent: AgentId("coder".to_owned()),
        person: UserId::new("person@example.com"),
        model: Some("model-x".to_owned()),
        expires_at: deadline_in(600),
    }
}

#[tokio::test]
async fn binding_sends_the_run_the_agent_the_person_and_the_deadline() {
    let (stub, client) = started().await;
    let binding = binding();
    client.bind(binding.clone()).await.expect("bind");
    let calls = stub.calls();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.method, "POST");
    assert_eq!(call.path, "/internal/runs");
    assert_eq!(
        call.authorization.as_deref(),
        Some("Bearer orchestrator-token")
    );
    let body: serde_json::Value = serde_json::from_str(&call.body).expect("JSON");
    assert_eq!(body["run"], "run-1");
    assert_eq!(body["agent"], "coder");
    assert_eq!(body["person"], "person@example.com");
    assert_eq!(body["model"], "model-x");
    assert_eq!(
        body["expiresAt"],
        binding.expires_at.to_string(),
        "the deadline is RFC 3339"
    );
}

#[tokio::test]
async fn a_binding_with_no_model_sends_no_model_key() {
    let (stub, client) = started().await;
    let binding = RunBinding {
        model: None,
        ..binding()
    };
    client.bind(binding).await.expect("bind");
    let body: serde_json::Value = serde_json::from_str(&stub.calls()[0].body).expect("JSON");
    assert!(body.get("model").is_none(), "{body}");
}

#[tokio::test]
async fn unbinding_names_the_run_in_the_path_and_encodes_it() {
    let (stub, client) = started().await;
    client.unbind(&RunId::new("run-1")).await.expect("unbind");
    client
        .unbind(&RunId::new("run/a b"))
        .await
        .expect("unbind an awkward id");
    let calls = stub.calls();
    assert_eq!(calls[0].method, "DELETE");
    assert_eq!(calls[0].path, "/internal/runs/run-1");
    assert_eq!(
        calls[0].authorization.as_deref(),
        Some("Bearer orchestrator-token")
    );
    assert_eq!(
        calls[1].path, "/internal/runs/run%2Fa%20b",
        "an id with a slash cannot address another route"
    );
}

#[tokio::test]
async fn a_conflict_is_its_own_answer_and_anything_else_is_a_refusal() {
    let (stub, client) = started().await;
    stub.answer(409);
    assert_eq!(client.bind(binding()).await, Err(RunBindingError::Conflict));
    stub.answer(500);
    assert_eq!(
        client.bind(binding()).await,
        Err(RunBindingError::Refused(500))
    );
    assert_eq!(
        client.unbind(&RunId::new("run-1")).await,
        Err(RunBindingError::Refused(500))
    );
}

#[tokio::test]
async fn an_unreachable_broker_is_unreachable() {
    // A port nothing listens on: the listener is dropped before the client knocks.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("an address");
    drop(listener);
    let client = RunBindingClient::new(&format!("http://{address}"), ToolSecret::new(TOKEN))
        .expect("a client");
    assert_eq!(
        client.bind(binding()).await,
        Err(RunBindingError::Unreachable)
    );
    assert_eq!(
        client.unbind(&RunId::new("run-1")).await,
        Err(RunBindingError::Unreachable)
    );
}

#[test]
fn the_address_must_be_an_http_url_and_the_token_never_prints() {
    for bad in [
        "not a url",
        "ftp://broker",
        "http://user:pw@broker",
        "http://broker?x=1",
    ] {
        assert!(
            matches!(
                RunBindingClient::new(bad, ToolSecret::new(TOKEN)),
                Err(BuildError::BrokerUrl)
            ),
            "{bad}"
        );
    }
    let client = RunBindingClient::new("http://broker.internal:8080", ToolSecret::new(TOKEN))
        .expect("a client");
    let printed = format!("{client:?}");
    assert!(!printed.contains(TOKEN), "{printed}");
    assert!(!format!("{:?}", RunBindingError::Conflict).contains(TOKEN));
}

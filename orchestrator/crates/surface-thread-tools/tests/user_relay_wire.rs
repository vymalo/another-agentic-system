//! The relay of a person's own servers over the wire (ADR 0046, decisions 4 and 5, slice 5a): the
//! real credential broker (the credential is sealed in its store, put there by the API route that
//! adds the server), the real MCP client with the egress guard at the connection, and real MCP
//! servers on the loopback.
//!
//! What only this can show: the upstream receives exactly the credential the person typed (a bearer
//! or a header) and nothing else of the orchestrator; the real client's 401 marks the server; a
//! server whose name resolves to a private address is **never connected to** (a tripwire counts the
//! connections) and the credential never leaves, however the relay arrived at it; and a server the
//! operator named exactly is.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use orch_core::StepState;
use orch_ports::{EgressPolicy, HostPattern};
use orch_testsupport::{FakeToolServer, FakeToolServerOptions};
use orch_tools_mcp::{HostLookup, LookupFuture, McpToolClient};
use serde_json::{Value, json};
use support::wire::WireRig;
use support::{call, tool_names};

const BEARER: &str = "wire-bearer-3d7a91c0-not-a-real-credential";
const KEY: &str = "wire-api-key-b81f45e2-not-a-real-credential";

fn pattern(text: &str) -> HostPattern {
    text.parse().unwrap()
}

/// The policy of a deployment that admits plain `http` and the loopback address (its servers here
/// are on it).
fn loopback_allowed() -> EgressPolicy {
    EgressPolicy::new()
        .allowing_http()
        .with_allow(pattern("127.0.0.1"))
}

/// A lookup that answers what the test says, whatever the name: DNS rebinding, played.
struct Answers(Vec<IpAddr>);

impl HostLookup for Answers {
    fn lookup<'a>(&'a self, _host: &'a str) -> LookupFuture<'a> {
        let answer = self.0.clone();
        Box::pin(async move { Ok::<_, io::Error>(answer) })
    }
}

/// A listener that counts the connections made to it and closes each at once.
struct Tripwire {
    port: u16,
    connections: Arc<AtomicUsize>,
}

impl Tripwire {
    async fn set() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&connections);
        tokio::spawn(async move {
            // a connection is counted and closed: a client that gets one fails at once
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                drop(socket);
            }
        });
        Tripwire { port, connections }
    }

    fn tripped(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

fn bearer_body(name: &str, url: &str, secret: &str) -> Value {
    json!({"name": name, "url": url, "auth": {"kind": "bearer"}, "secret": secret})
}

#[tokio::test]
async fn the_server_receives_exactly_the_credential_the_person_typed() {
    let bearer_server =
        FakeToolServer::spawn(FakeToolServerOptions::default().bearer(BEARER)).await;
    let key_server =
        FakeToolServer::spawn(FakeToolServerOptions::default().require_header("X-Api-Key", KEY))
            .await;
    let rig = WireRig::start(
        loopback_allowed(),
        McpToolClient::guarded(loopback_allowed()).unwrap(),
    )
    .await;
    let tracker = rig
        .add(bearer_body("Tracker", &bearer_server.url(), BEARER))
        .await;
    let search = rig
        .add(json!({"name": "Search", "url": key_server.url(),
                    "auth": {"kind": "api_key", "header": "X-Api-Key"}, "secret": KEY}))
        .await;
    let thread = rig.working_thread(&[&tracker, &search]).await;
    let client = rig.client(thread).await;

    let names = tool_names(&client).await;
    assert!(names.contains(&format!("{tracker}__echo")), "{names:?}");
    assert!(names.contains(&format!("{search}__echo")), "{names:?}");
    let one = call(&client, &format!("{tracker}__echo"), json!({"text": "a"})).await;
    let two = call(&client, &format!("{search}__echo"), json!({"text": "b"})).await;
    assert!(!one.is_error && !two.is_error, "{one:?} {two:?}");
    assert_eq!(one.value, json!({"text": "a"}));

    // the bearer went as `Authorization: Bearer`, the key in the header the person named, and
    // neither carries the other, nor the thread token, nor anything else of the orchestrator
    let seen = bearer_server.seen();
    assert!(
        seen.iter().all(|s| s.bearer.as_deref() == Some(BEARER)),
        "{seen:?}"
    );
    assert_eq!(seen.iter().filter(|s| s.method == "tools/call").count(), 1);
    let seen = key_server.seen();
    assert!(!seen.is_empty());
    for request in &seen {
        assert_eq!(request.bearer, None);
        assert!(
            request
                .headers
                .iter()
                .any(|(name, value)| name == "x-api-key" && value == KEY),
            "{:?}",
            request.headers
        );
    }
}

#[tokio::test]
async fn a_401_through_the_real_client_marks_the_server_with_or_without_a_challenge() {
    for challenge in [true, false] {
        // the server wants another token than the one the person typed
        let options = FakeToolServerOptions::default().bearer("the-token-it-wants");
        let server = FakeToolServer::spawn(if challenge {
            options
        } else {
            options.without_challenge()
        })
        .await;
        let rig = WireRig::start(
            loopback_allowed(),
            McpToolClient::guarded(loopback_allowed()).unwrap(),
        )
        .await;
        let id = rig
            .add(bearer_body("Tracker", &server.url(), "the-token-typed"))
            .await;
        let thread = rig.working_thread(&[&id]).await;
        let client = rig.client(thread).await;

        let out = call(&client, &format!("{id}__echo"), json!({})).await;
        assert!(out.is_error, "{out:?}");
        assert_eq!(out.text, orch_surface_thread_tools::NEEDS_SIGN_IN);
        let (_, mine) = rig
            .api(reqwest::Method::GET, "/api/user-tool-servers", None)
            .await;
        let mine: Value = serde_json::from_str(&mine).unwrap();
        let row = &mine["servers"][0];
        assert_eq!(
            (
                row["state"].as_str(),
                row["status"].as_str(),
                row["reason"].as_str()
            ),
            (
                Some("needs_reauth"),
                Some("needs_auth"),
                Some("reauthorize")
            ),
            "challenge: {challenge}"
        );
        // the server refused the request before it spoke MCP: it served nothing
        assert!(server.seen().is_empty());
        let steps = rig
            .app
            .list_events(&WireRig::alice(), thread, 0, 100)
            .await
            .unwrap();
        let ended: Vec<_> = steps
            .into_iter()
            .filter_map(|e| match e.body {
                orch_core::EventBody::AgentStep(data) if data.state != StepState::Running => {
                    Some(data)
                }
                _ => None,
            })
            .collect();
        assert_eq!(ended.len(), 1);
        assert_eq!(ended[0].state, StepState::Failed);
    }
}

/// A call to a server that is added under a public name which `policy` admits and the lookup of the
/// client answers with the loopback: how many connections reached the tripwire, and what the agent
/// read. The list is read first.
async fn through_a_name_that_resolves_to_the_loopback(policy: EgressPolicy) -> (usize, String) {
    let tripwire = Tripwire::set().await;
    let rebinding = McpToolClient::guarded_with_lookup(
        policy.clone(),
        Answers(vec!["127.0.0.1".parse().unwrap()]),
    )
    .unwrap();
    let rig = WireRig::start(policy, rebinding).await;
    let url = format!("http://mcp.example.test:{}/mcp", tripwire.port);
    let id = rig.add(bearer_body("Rebinder", &url, BEARER)).await;
    let thread = rig.working_thread(&[&id]).await;
    let client = rig.client(thread).await;
    // left out of the list
    assert!(
        !tool_names(&client)
            .await
            .iter()
            .any(|n| n.starts_with(&format!("{id}__")))
    );
    let out = call(&client, &format!("{id}__echo"), json!({})).await;
    assert!(out.is_error);
    // never marked as needing a key: the credential was not refused by anybody
    let (_, mine) = rig
        .api(reqwest::Method::GET, "/api/user-tool-servers", None)
        .await;
    let mine: Value = serde_json::from_str(&mine).unwrap();
    assert_eq!(mine["servers"][0]["state"], "connected");
    (tripwire.tripped(), out.text)
}

#[tokio::test]
async fn a_server_whose_name_resolves_to_a_private_address_is_never_connected_to() {
    // The name passes every check of the URL, as any public name does; what it resolves to, at the
    // moment of the connection, is the loopback (DNS rebinding). The credential is leased, and it
    // goes nowhere: the connection is refused on the address it would have been made to.
    let (connections, said) =
        through_a_name_that_resolves_to_the_loopback(EgressPolicy::new().allowing_http()).await;
    assert_eq!(connections, 0, "a connection was made to a private address");
    assert_eq!(
        said,
        "the MCP server 'Rebinder' is not reachable from here; the call did not run"
    );
    assert!(
        !said.contains("127.0.0.1") && !said.contains(BEARER),
        "{said}"
    );

    // the control: the same stack, the same name and the same answer of the lookup connect when the
    // operator names the host exactly (it is then theirs to say it may be private), so the test above
    // is not one that nothing could reach
    let (connections, said) = through_a_name_that_resolves_to_the_loopback(
        EgressPolicy::new()
            .allowing_http()
            .with_allow(pattern("mcp.example.test")),
    )
    .await;
    assert!(connections >= 1, "the admitted name was not connected to");
    assert_eq!(
        said, "the MCP server 'Rebinder' could not be reached; the call did not run",
        "the tripwire closes the connection: unreachable, and not refused"
    );
}

#[tokio::test]
async fn a_server_the_operator_named_exactly_is_called_where_the_default_policy_would_refuse_it() {
    // control for the test above: the same stack reaches the loopback when the deployment names it
    let server = FakeToolServer::spawn(FakeToolServerOptions::default().bearer(BEARER)).await;
    let strict = EgressPolicy::new().allowing_http();
    let rig = WireRig::start(strict.clone(), McpToolClient::guarded(strict).unwrap()).await;
    // not admitted at all: the loopback is private
    let (status, body) = rig
        .api(
            reqwest::Method::POST,
            "/api/user-tool-servers",
            Some(bearer_body("Local", &server.url(), BEARER)),
        )
        .await;
    assert_eq!(status, 400, "{body}");
    assert!(server.seen().is_empty());

    let named = WireRig::start(
        loopback_allowed(),
        McpToolClient::guarded(loopback_allowed()).unwrap(),
    )
    .await;
    let id = named.add(bearer_body("Local", &server.url(), BEARER)).await;
    let thread = named.working_thread(&[&id]).await;
    let out = call(
        &named.client(thread).await,
        &format!("{id}__echo"),
        json!({"text": "ok"}),
    )
    .await;
    assert!(!out.is_error, "{out:?}");
    assert!(!server.seen().is_empty());
}

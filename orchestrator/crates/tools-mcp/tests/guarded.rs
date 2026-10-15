//! The MCP client for servers people name (`McpToolClient::guarded`, ADR 0046, decision 5): the
//! egress guard runs again at every connection, on the address the connection is made to.
//!
//! What is proven with a tripwire, a listener that counts the connections it is made: a server the
//! policy refuses is **never connected to** (no handshake, no bearer sent), whether it is written as
//! an address or reached through a name that resolves to a private one (DNS rebinding, with the
//! lookup of the test standing in for the resolver), and a redirect is not followed. The suite of
//! the port also runs against a guarded client, so its behaviour on an admitted server is the
//! unguarded one's.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::io;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::routing::any;
use orch_core::{Classify, ErrorClass};
use orch_ports::testkit::tool_server::{SeenRequest, ToolServerFixture};
use orch_ports::{
    EgressPolicy, EgressRefusal, HostPattern, ToolCall, ToolSecret, ToolServerClient,
    ToolServerEndpoint, ToolServerError,
};
use orch_testsupport::{FakeToolServer, FakeToolServerOptions};
use orch_tools_mcp::{HostLookup, LookupFuture, McpToolClient};

const BEARER: &str = "guarded-bearer-0123456789-very-secret";
const HEADER: (&str, &str) = ("X-Guarded-Key", "guarded-header-0123456789-very-secret");

fn pattern(text: &str) -> HostPattern {
    text.parse().unwrap()
}

/// Plain `http` on the loopback is what a test can serve; the policy that admits it names it.
fn admitting_loopback() -> EgressPolicy {
    EgressPolicy::new()
        .allowing_http()
        .with_allow(pattern("127.0.0.1"))
}

fn endpoint(url: impl Into<String>) -> ToolServerEndpoint {
    ToolServerEndpoint::new("guarded", url, Duration::from_secs(5))
        .with_bearer(ToolSecret::new(BEARER))
        .with_header(HEADER.0, ToolSecret::new(HEADER.1))
}

/// A listener that counts the connections made to it and answers none of them: the tripwire.
struct Tripwire {
    port: u16,
    connections: Arc<AtomicUsize>,
}

impl Tripwire {
    async fn set() -> Self {
        Self::set_on("127.0.0.1").await
    }

    /// On another address of the loopback block, which Linux binds as it does `127.0.0.1`.
    async fn set_on(address: &str) -> Self {
        let listener = tokio::net::TcpListener::bind((address, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&connections);
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                counted.fetch_add(1, Ordering::SeqCst);
                held.push(socket);
            }
        });
        Tripwire { port, connections }
    }

    fn url(&self, host: &str) -> String {
        format!("http://{host}:{}/mcp", self.port)
    }

    fn tripped(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

/// A lookup that answers what the test says, whatever the name.
struct Answers(Vec<IpAddr>);

impl HostLookup for Answers {
    fn lookup<'a>(&'a self, _host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

/// A lookup that fails.
struct Broken;

impl HostLookup for Broken {
    fn lookup<'a>(&'a self, _host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move { Err(io::Error::other("no resolver")) })
    }
}

fn refused(error: &ToolServerError) -> Option<EgressRefusal> {
    match error {
        ToolServerError::EgressRefused(rule) => Some(*rule),
        _ => None,
    }
}

// ---- the suite of the port, against a guarded client ------------------------------------------

struct Fixture {
    client: McpToolClient,
    server: FakeToolServer,
    unreachable: String,
}

impl ToolServerFixture for Fixture {
    type Client = McpToolClient;

    fn client(&self) -> &McpToolClient {
        &self.client
    }

    fn bearer(&self) -> &str {
        BEARER
    }

    fn header(&self) -> (&str, &str) {
        HEADER
    }

    fn endpoint(&self) -> ToolServerEndpoint {
        endpoint(self.server.url())
    }

    fn unreachable_endpoint(&self) -> ToolServerEndpoint {
        endpoint(self.unreachable.clone())
    }

    fn seen(&self) -> Vec<SeenRequest> {
        self.server.seen()
    }
}

mod an_admitted_server {
    use super::*;

    async fn make() -> Option<Fixture> {
        Some(Fixture {
            client: McpToolClient::guarded(admitting_loopback()).unwrap(),
            server: FakeToolServer::spawn(
                FakeToolServerOptions::default()
                    .bearer(BEARER)
                    .require_header(HEADER.0, HEADER.1),
            )
            .await,
            unreachable: FakeToolServer::closed_url().await,
        })
    }

    orch_ports::tool_server_conformance!(make);
}

// ---- an address the policy refuses is never connected to ---------------------------------------

#[tokio::test]
async fn loopback_is_never_connected_to_under_the_default_policy() {
    let tripwire = Tripwire::set().await;
    // `http` is switched on, as outside production; the loopback is still private
    let client = McpToolClient::guarded(EgressPolicy::new().allowing_http()).unwrap();
    let server = endpoint(tripwire.url("127.0.0.1"));

    let listed = client.list_tools(&server).await.unwrap_err();
    assert_eq!(
        refused(&listed),
        Some(EgressRefusal::PrivateAddress),
        "{listed:?}"
    );
    let called = client
        .call_tool(&server, &ToolCall::new("echo"))
        .await
        .unwrap_err();
    assert_eq!(
        refused(&called),
        Some(EgressRefusal::PrivateAddress),
        "{called:?}"
    );

    assert_eq!(
        tripwire.tripped(),
        0,
        "a connection was made to a refused address"
    );
}

#[tokio::test]
async fn every_spelling_of_a_private_address_is_refused_before_a_connection() {
    let tripwire = Tripwire::set().await;
    let client = McpToolClient::guarded(EgressPolicy::new().allowing_http()).unwrap();
    for host in [
        "127.0.0.1",
        "2130706433",
        "0x7f.0.0.1",
        "017700000001",
        "127.1",
        "[::1]",
        "[::ffff:7f00:1]",
        "localhost",
        "LOCALHOST.",
        "sub.localhost",
    ] {
        let err = client
            .list_tools(&endpoint(tripwire.url(host)))
            .await
            .unwrap_err();
        assert!(refused(&err).is_some(), "{host}: {err:?}");
    }
    assert_eq!(tripwire.tripped(), 0);
}

#[tokio::test]
async fn a_name_that_resolves_to_a_private_address_is_refused_on_the_address_connected_to() {
    // The name passes the check of the URL; what it resolves to does not (DNS rebinding).
    let tripwire = Tripwire::set().await;
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let client = McpToolClient::guarded_with_lookup(
        EgressPolicy::new().allowing_http(),
        Answers(vec![loopback]),
    )
    .unwrap();
    let err = client
        .list_tools(&endpoint(tripwire.url("mcp.example.test")))
        .await
        .unwrap_err();
    assert_eq!(
        refused(&err),
        Some(EgressRefusal::PrivateAddress),
        "{err:?}"
    );
    assert_eq!(tripwire.tripped(), 0);

    // an answer that mixes a public address with a private one is refused whole
    let mixed = McpToolClient::guarded_with_lookup(
        EgressPolicy::new().allowing_http(),
        Answers(vec!["93.184.216.34".parse().unwrap(), loopback]),
    )
    .unwrap();
    let err = mixed
        .call_tool(
            &endpoint(tripwire.url("mcp.example.test")),
            &ToolCall::new("echo"),
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused(&err),
        Some(EgressRefusal::PrivateAddress),
        "{err:?}"
    );
    assert_eq!(tripwire.tripped(), 0);
}

#[tokio::test]
async fn a_name_the_operator_named_exactly_may_resolve_to_a_private_address() {
    // The name is connected to only through the lookup (the system does not know it). The listener
    // answers nothing, so the handshake ends at the endpoint's timeout: the error is "nobody
    // answered", and the tripwire shows that the connection was made.
    let tripwire = Tripwire::set().await;
    let policy = EgressPolicy::new()
        .allowing_http()
        .with_allow(pattern("intranet.example.test"));
    let client =
        McpToolClient::guarded_with_lookup(policy, Answers(vec!["127.0.0.1".parse().unwrap()]))
            .unwrap();
    let err = client
        .list_tools(
            &endpoint(tripwire.url("intranet.example.test"))
                .with_timeout(Duration::from_millis(300)),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ToolServerError::Unreachable { .. }),
        "{err:?}"
    );
    assert_eq!(
        tripwire.tripped(),
        1,
        "the admitted name was not connected to"
    );

    // a suffix entry admits the name and does not lift the ban: no second connection
    let suffix = McpToolClient::guarded_with_lookup(
        EgressPolicy::new()
            .allowing_http()
            .with_allow(pattern("*.example.test")),
        Answers(vec!["127.0.0.1".parse().unwrap()]),
    )
    .unwrap();
    let err = suffix
        .list_tools(&endpoint(tripwire.url("intranet.example.test")))
        .await
        .unwrap_err();
    assert_eq!(
        refused(&err),
        Some(EgressRefusal::PrivateAddress),
        "{err:?}"
    );
    assert_eq!(tripwire.tripped(), 1);
}

#[tokio::test]
async fn the_lists_of_the_policy_apply_to_the_url_at_every_connection() {
    let server = FakeToolServer::spawn(FakeToolServerOptions::default()).await;
    let url = server.url();
    // a deny entry stops a host that was admitted when the server was added
    let denied =
        McpToolClient::guarded(admitting_loopback().with_deny(pattern("127.0.0.1"))).unwrap();
    let err = denied.list_tools(&endpoint(url.clone())).await.unwrap_err();
    assert_eq!(refused(&err), Some(EgressRefusal::Denied), "{err:?}");
    // an allow list that does not name the host stops it
    let elsewhere = McpToolClient::guarded(
        EgressPolicy::new()
            .allowing_http()
            .with_allow(pattern("mcp.example.com")),
    )
    .unwrap();
    let err = elsewhere.list_tools(&endpoint(url)).await.unwrap_err();
    assert_eq!(refused(&err), Some(EgressRefusal::NotAllowed), "{err:?}");
    assert!(server.seen().is_empty(), "the server was reached");
}

#[tokio::test]
async fn plain_http_is_refused_unless_the_policy_allows_it() {
    let server = FakeToolServer::spawn(FakeToolServerOptions::default()).await;
    let client =
        McpToolClient::guarded(EgressPolicy::new().with_allow(pattern("127.0.0.1"))).unwrap();
    let err = client
        .list_tools(&endpoint(server.url()))
        .await
        .unwrap_err();
    assert_eq!(refused(&err), Some(EgressRefusal::Scheme), "{err:?}");
    assert!(server.seen().is_empty());
}

#[tokio::test]
async fn a_url_the_guard_does_not_accept_is_refused_whatever_the_client_would_make_of_it() {
    let client = McpToolClient::guarded(admitting_loopback()).unwrap();
    for (url, rule) in [
        ("http://user:pass@127.0.0.1:9/mcp", EgressRefusal::UserInfo),
        ("http://127.0.0.1:9/mcp?key=1", EgressRefusal::Query),
        ("http://127.0.0.1:9/mcp#frag", EgressRefusal::Fragment),
        ("http://127.0.0.1:0/mcp", EgressRefusal::Port),
    ] {
        let err = client.list_tools(&endpoint(url)).await.unwrap_err();
        assert_eq!(refused(&err), Some(rule), "{url}: {err:?}");
    }
}

#[tokio::test]
async fn a_name_that_does_not_resolve_is_unreachable_and_a_refusal_is_not() {
    let client =
        McpToolClient::guarded_with_lookup(EgressPolicy::new().allowing_http(), Broken).unwrap();
    let err = client
        .list_tools(&endpoint("http://nowhere.example.test:9/mcp"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ToolServerError::Unreachable { .. }),
        "{err:?}"
    );
    assert!(refused(&err).is_none());
}

#[tokio::test]
async fn a_refusal_names_the_rule_and_no_address_url_or_secret() {
    let tripwire = Tripwire::set().await;
    let client = McpToolClient::guarded_with_lookup(
        EgressPolicy::new().allowing_http(),
        Answers(vec!["169.254.169.254".parse().unwrap()]),
    )
    .unwrap();
    let err = client
        .list_tools(&endpoint(tripwire.url("mcp.example.test")))
        .await
        .unwrap_err();
    let shown = format!(
        "{err} {err:?} {} {}",
        err.public_detail(),
        source_chain(&err)
    );
    for secret in [BEARER, HEADER.1, "169.254", "mcp.example.test", "/mcp"] {
        assert!(!shown.contains(secret), "{secret} is in {shown}");
    }
    assert_eq!(err.class(), EgressRefusal::PrivateAddress.class());
    assert_eq!(err.class(), ErrorClass::Invalid, "nobody retries a refusal");
    assert_eq!(tripwire.tripped(), 0);
}

fn source_chain(error: &dyn std::error::Error) -> String {
    let mut text = String::new();
    let mut source = error.source();
    while let Some(e) = source {
        text.push_str(&format!("{e} {e:?} "));
        source = e.source();
    }
    text
}

// ---- no redirect is followed ---------------------------------------------------------------------

// `127.0.0.2` is bound as the loopback only on Linux (the rest of 127/8 is not an address of the
// loopback interface on macOS, for one).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_redirect_to_a_refused_address_is_not_followed() {
    // The origin is admitted; it answers a redirect to an address nobody admitted. Neither the
    // target nor the bearer is reached.
    let target = Tripwire::set_on("127.0.0.2").await;
    let to = target.url("127.0.0.2");
    let origin = Router::new().route(
        "/mcp",
        any(move || {
            let to = to.clone();
            async move { (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, to)]) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, origin).await.unwrap() });

    let client = McpToolClient::guarded(admitting_loopback()).unwrap();
    let err = client
        .list_tools(&endpoint(format!("http://{addr}/mcp")))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ToolServerError::Protocol { .. } | ToolServerError::Unreachable { .. }
        ),
        "{err:?}"
    );
    assert_eq!(target.tripped(), 0, "the redirect was followed");
}

// ---- the URL the client connects to is the guard's normal form -----------------------------------

#[tokio::test]
async fn the_client_connects_to_the_guards_normal_form_of_the_url_not_the_text_it_was_given() {
    // The guard reads this text as `http://127.0.0.1:<port>/mcp` (leading spaces and the case of
    // the scheme are not part of a URL), and that is what is judged and connected to. The text
    // itself is not a URL the transport takes: a client that used it would fail on it.
    let server = FakeToolServer::spawn(FakeToolServerOptions::default()).await;
    let normal = server.url();
    let written = format!("  HTTP://{}", normal.strip_prefix("http://").unwrap());
    let tools = McpToolClient::guarded(admitting_loopback())
        .unwrap()
        .list_tools(&endpoint(written.clone()))
        .await
        .expect("the normal form is what is used");
    assert_eq!(tools.len(), 4);
    // the control: the unguarded client takes the text as it is, and cannot use it
    let error = McpToolClient::new()
        .unwrap()
        .list_tools(&endpoint(written))
        .await
        .unwrap_err();
    assert!(
        matches!(error, ToolServerError::Misconfigured(_)),
        "{error:?}"
    );
}

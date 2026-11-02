//! A tool server that speaks only MCP `2026-07-28` and takes signed requests
//! (`docs/api/signed-tool-requests-v1.md`, ADR 0067), against the MCP client: what it opens with, what every request
//! carries, what the receiving side's checks say of it, and what the client refuses to send.
//!
//! The server is `FakeToolServer` with `modern_only` and `signed`: it verifies each request with the reference verifier
//! (checks 1 to 5), refuses a `cti` it has seen (check 6), and journals the verdict. The same bytes are held to the real
//! `cratestack_cose::open_claims` by `crates/signed-request/interop`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;
use std::time::Duration;

use base64::Engine as _;
use orch_core::{ThreadId, UserId};
use orch_ports::{
    ToolCall, ToolProtocol, ToolSecret, ToolServerClient, ToolServerEndpoint, ToolServerError,
    ToolSigning,
};
use orch_signed_request::verify::{Incoming, Mismatch, Object, Verifier, VerifyError};
use orch_signed_request::{HEADER, SigningKey};
use orch_testsupport::{FakeToolServer, FakeToolServerOptions, SignedRequest};
use orch_tools_mcp::McpToolClient;
use serde_json::{Value, json};

const AUDIENCE: &str = "https://tools.example/mcp";
const SEED: &[u8; 32] = b"throwaway-ed25519-seed-do-not-us";

/// A PKCS#8 version-1 PEM for an Ed25519 seed, as `openssl genpkey -algorithm ed25519` writes it.
fn pem_of(seed: &[u8; 32]) -> String {
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend_from_slice(seed);
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    format!(
        "-----BEGIN PRIVATE {0}-----\n{body}\n-----END PRIVATE {0}-----\n",
        "KEY"
    )
}

fn thread() -> ThreadId {
    "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c33".parse().unwrap()
}

fn public_key() -> [u8; 32] {
    SigningKey::from_seed(SEED).unwrap().public_key()
}

/// A server that speaks only `2026-07-28` and takes the requests signed by `SEED`.
async fn modern_signed_server() -> FakeToolServer {
    FakeToolServer::spawn(
        FakeToolServerOptions::default()
            .modern_only()
            .signed(AUDIENCE, &[public_key()]),
    )
    .await
}

fn signing(seed: &[u8; 32], audience: &str) -> ToolSigning {
    ToolSigning {
        key: ToolSecret::new(pem_of(seed)),
        audience: audience.to_owned(),
        ttl_secs: 30,
    }
}

/// The endpoint the relay hands the client for a request: protocol, signing, and for whom.
fn endpoint(server: &FakeToolServer) -> ToolServerEndpoint {
    ToolServerEndpoint::new("vaam", server.url(), Duration::from_secs(10))
        .with_protocol(ToolProtocol::V2026_07_28)
        .with_signing(signing(SEED, AUDIENCE))
        .with_acting_for(thread(), UserId::new("  Amina@Example.com "))
}

fn client() -> McpToolClient {
    McpToolClient::new().unwrap()
}

/// Checks 1 to 5 of the contract on a request as it was captured.
fn check(
    verifier: &Verifier,
    header: &str,
    method: &str,
    path: &str,
    body: &[u8],
    now: u64,
) -> Result<orch_signed_request::verify::Verified, VerifyError> {
    verifier.verify(
        &[header],
        &Incoming {
            method,
            path,
            query: None,
            body,
            content_encoding: None,
        },
        now,
    )
}

fn json_body(request: &SignedRequest) -> Value {
    serde_json::from_slice(&request.body).unwrap()
}

#[tokio::test]
async fn a_session_opens_with_server_discover_and_every_request_is_signed_for_the_thread_and_its_owner()
 {
    let server = modern_signed_server().await;
    let client = client();
    let endpoint = endpoint(&server);

    let tools = client.list_tools(&endpoint).await.unwrap();
    assert!(tools.iter().any(|t| t.name == "echo"), "{tools:?}");
    let call = ToolCall::new("echo")
        .with_arguments(json!({"text": "hello"}).as_object().cloned().unwrap());
    let output = client.call_tool(&endpoint, &call).await.unwrap();
    assert!(
        !output.is_error && output.text_of().contains("hello"),
        "{output:?}"
    );

    let requests = server.signed_requests();
    assert!(
        requests.len() >= 3,
        "discover, list and call: {}",
        requests.len()
    );
    // the handshake is `server/discover` and `initialize` is never sent
    let methods: Vec<String> = requests
        .iter()
        .filter_map(|r| json_body(r)["method"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(methods[0], "server/discover", "{methods:?}");
    assert!(!methods.iter().any(|m| m == "initialize"), "{methods:?}");
    assert!(methods.iter().any(|m| m == "tools/list"));
    assert!(methods.iter().any(|m| m == "tools/call"));

    let mut ctis = BTreeSet::new();
    for request in &requests {
        // every request was let through: the signature, the audience, the time, and method, path and the digest of the
        // very bytes the server received (checks 1 to 6)
        assert!(request.accepted(), "{request:?}");
        assert_eq!(
            (request.method.as_str(), request.path.as_str()),
            ("POST", "/mcp")
        );
        assert_eq!(request.claims_headers.len(), 1, "one header, once");
        assert!(
            !request.had_authorization,
            "a signed server is sent no bearer"
        );
        let verified = request.verdict.as_ref().unwrap();
        // for the thread, and the thread's owner as the orchestrator holds them (trimmed and lower-cased)
        assert_eq!(verified.thread, thread().0);
        assert_eq!(verified.owner, "amina@example.com");
        assert_eq!(verified.exp - verified.iat, 30);
        // a cti of its own for each HTTP request
        assert!(ctis.insert(verified.cti), "a cti was used twice");
    }
    // the MCP handler saw no `Authorization`, and saw the header the claims travelled in
    let seen = server.seen();
    assert!(!seen.is_empty());
    for request in &seen {
        assert!(request.header("authorization").is_none() && request.bearer.is_none());
        assert!(request.header(HEADER).is_some());
    }
}

#[tokio::test]
async fn a_tools_call_binds_the_bytes_that_were_sent_and_tampering_with_any_of_them_fails() {
    let server = modern_signed_server().await;
    let endpoint = endpoint(&server);
    let call = ToolCall::new("echo").with_arguments(
        json!({"text": "héllo  wörld"})
            .as_object()
            .cloned()
            .unwrap(),
    );
    client().call_tool(&endpoint, &call).await.unwrap();

    // what crossed the wire for the call, taken from the server's journal
    let captured = server
        .signed_requests()
        .into_iter()
        .find(|r| json_body(r)["method"] == "tools/call")
        .expect("the call");
    let header = captured.claims_headers[0].as_str();
    let verifier = Verifier::new(AUDIENCE, 30).pin(public_key()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let received = |method: &str, path: &str, body: &[u8]| -> Result<_, VerifyError> {
        check(&verifier, header, method, path, body, now)
    };
    // untouched: the same request the server accepted verifies
    let body = captured.body.as_slice();
    let ok = received("POST", "/mcp", body).unwrap();
    assert_eq!(ok.owner, "amina@example.com");
    // the method, the path and the body: each fails in its own check
    assert_eq!(
        received("GET", "/mcp", body).unwrap_err(),
        VerifyError::Request(Mismatch::Method)
    );
    assert_eq!(
        received("POST", "/mcp/x", body).unwrap_err(),
        VerifyError::Request(Mismatch::Path)
    );
    let mut changed = captured.body.clone();
    let at = changed.iter().position(|b| *b == b'w').unwrap();
    changed[at] = b'W';
    assert_eq!(
        received("POST", "/mcp", &changed).unwrap_err(),
        VerifyError::Request(Mismatch::Body)
    );
    // the thread and the owner are in the signed payload: change one and the object is not the one that was signed
    for (from, to) in [
        (
            "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c33",
            "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c34",
        ),
        ("amina@example.com", "bella@example.com"),
    ] {
        let mut bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(header)
            .unwrap();
        let at = bytes
            .windows(from.len())
            .position(|w| w == from.as_bytes())
            .unwrap();
        bytes[at..at + to.len()].copy_from_slice(to.as_bytes());
        let tampered = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        assert_eq!(
            check(&verifier, &tampered, "POST", "/mcp", body, now).unwrap_err(),
            VerifyError::Object(Object::Unverified),
            "{from} -> {to}"
        );
    }

    // and a replay: the same bytes, sent again, are refused by the server (check 6 is the server's job)
    let replay = reqwest::Client::new()
        .post(server.url())
        .header(HEADER, header)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body(captured.body.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(replay.status(), 401);
    let last = server.signed_requests().pop().unwrap();
    assert!(last.replayed && !last.accepted(), "{last:?}");
}

#[tokio::test]
async fn a_server_that_speaks_only_the_new_revision_refuses_the_old_handshake() {
    // the server is not signed here: this is about how the session opens
    let server = FakeToolServer::spawn(FakeToolServerOptions::default().modern_only()).await;
    let old = ToolServerEndpoint::new("modern", server.url(), Duration::from_secs(10));
    assert_eq!(old.protocol, ToolProtocol::Legacy);
    let refused = client().list_tools(&old).await.unwrap_err();
    assert!(
        matches!(
            refused,
            ToolServerError::Remote { .. } | ToolServerError::Protocol { .. }
        ),
        "{refused:?}"
    );
    // and the same server, told it is one, is listed and called
    let modern = old.clone().with_protocol(ToolProtocol::V2026_07_28);
    let tools = client().list_tools(&modern).await.unwrap();
    assert_eq!(tools.len(), 4);
    let output = client()
        .call_tool(
            &modern,
            &ToolCall::new("echo")
                .with_arguments(json!({"text": "x"}).as_object().cloned().unwrap()),
        )
        .await
        .unwrap();
    assert!(output.text_of().contains('x'));
}

#[tokio::test]
async fn a_server_that_does_not_speak_the_new_revision_is_an_error_not_a_fallback() {
    // a server of before 2026-07-28: it answers `server/discover` with 404, as a server that has no such method does
    let router = axum::Router::new().route(
        "/mcp",
        axum::routing::post(|| async { (axum::http::StatusCode::NOT_FOUND, "no such method") }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await });
    let endpoint =
        ToolServerEndpoint::new("old", format!("http://{addr}/mcp"), Duration::from_secs(5))
            .with_protocol(ToolProtocol::V2026_07_28);
    let error = client().list_tools(&endpoint).await.unwrap_err();
    assert!(
        !matches!(error, ToolServerError::Misconfigured(_)),
        "a server that is not one is not a mistake in the endpoint: {error:?}"
    );
}

#[tokio::test]
async fn a_signed_request_that_cannot_be_signed_is_never_sent_and_never_sent_unsigned() {
    let server = modern_signed_server().await;
    let client = client();
    let good = endpoint(&server);
    let mut cases: Vec<(&str, ToolServerEndpoint)> = Vec::new();
    // for no thread and no owner
    let mut nobody = good.clone();
    nobody.acting_for = None;
    cases.push(("no thread", nobody));
    // a bearer beside the signing
    cases.push((
        "a bearer",
        good.clone().with_bearer(ToolSecret::new("tok-0123456789")),
    ));
    // a header of the claims set by hand
    cases.push((
        "the header set by hand",
        good.clone().with_header(HEADER, ToolSecret::new("x")),
    ));
    // a key that is not a key
    let mut bad_key = good.clone();
    bad_key.signing = Some(ToolSigning {
        key: ToolSecret::new("not a pem, MC4CAQAwBQYDK2VwBCIE"),
        ..signing(SEED, AUDIENCE)
    });
    cases.push(("a bad key", bad_key));
    // an audience and a lifetime out of bounds
    let mut audience = good.clone();
    audience.signing = Some(signing(SEED, ""));
    cases.push(("no audience", audience));
    let mut ttl = good.clone();
    ttl.signing = Some(ToolSigning {
        ttl_secs: 61,
        ..signing(SEED, AUDIENCE)
    });
    cases.push(("a long life", ttl));
    // an owner the contract cannot carry
    cases.push((
        "a long owner",
        ToolServerEndpoint {
            acting_for: None,
            ..good.clone()
        }
        .with_acting_for(thread(), UserId::new(&"a".repeat(321))),
    ));
    // a URL with a query
    let mut query = good.clone();
    query.url = format!("{}?x=1", good.url);
    cases.push(("a query", query));

    for (what, endpoint) in cases {
        let error = client.list_tools(&endpoint).await.unwrap_err();
        assert!(
            matches!(error, ToolServerError::Misconfigured(_)),
            "{what}: {error:?}"
        );
        let shown = format!("{error} {error:?}");
        for secret in ["MC4CAQAw", "tok-0123456789", "not a pem"] {
            assert!(!shown.contains(secret), "{what}: {shown}");
        }
    }
    assert!(server.signed_requests().is_empty(), "something was sent");
    // while the good endpoint, unchanged, works
    client.list_tools(&good).await.unwrap();
}

#[tokio::test]
async fn a_request_signed_for_another_audience_or_by_another_key_is_refused_by_the_server() {
    let server = modern_signed_server().await;
    let client = client();
    let mut other_audience = endpoint(&server);
    other_audience.signing = Some(signing(SEED, "https://other.example/mcp"));
    let mut other_key = endpoint(&server);
    other_key.signing = Some(signing(&[9; 32], AUDIENCE));
    for (what, endpoint) in [("audience", other_audience), ("key", other_key)] {
        let error = client.list_tools(&endpoint).await.unwrap_err();
        assert!(
            matches!(error, ToolServerError::Unauthenticated),
            "{what}: {error:?}"
        );
    }
    let requests = server.signed_requests();
    assert_eq!(
        requests.len(),
        2,
        "the handshake, once each, and nothing after it"
    );
    assert!(matches!(requests[0].verdict, Err(VerifyError::Audience)));
    assert!(matches!(
        requests[1].verdict,
        Err(VerifyError::Object(Object::Unverified))
    ));
    // the server's refusal quoted nothing: the error holds no claim and no key
    let error = client
        .list_tools(&endpoint_with(&server, signing(&[9; 32], AUDIENCE)))
        .await
        .unwrap_err();
    let shown = format!("{error} {error:?}");
    for request in server.signed_requests() {
        for header in &request.claims_headers {
            assert!(!shown.contains(header.as_str()));
        }
    }
}

fn endpoint_with(server: &FakeToolServer, signing: ToolSigning) -> ToolServerEndpoint {
    let mut endpoint = endpoint(server);
    endpoint.signing = Some(signing);
    endpoint
}

#[tokio::test]
async fn the_thread_and_the_owner_of_each_request_are_those_of_that_request() {
    // one client, two threads: a header made for one is never used for the other
    let server = modern_signed_server().await;
    let client = client();
    let other_thread: ThreadId = "0194a6c2-6b1e-7c3a-9f40-5d2e8b7a1c99".parse().unwrap();
    let a = endpoint(&server);
    let b = ToolServerEndpoint {
        acting_for: None,
        ..endpoint(&server)
    }
    .with_acting_for(other_thread, UserId::new("bella@example.com"));
    client.list_tools(&a).await.unwrap();
    client.list_tools(&b).await.unwrap();
    client.list_tools(&a).await.unwrap();
    let owners: Vec<(uuid::Uuid, String)> = server
        .signed_requests()
        .iter()
        .map(|r| {
            let v = r.verdict.as_ref().unwrap();
            (v.thread, v.owner.clone())
        })
        .collect();
    let for_a = (thread().0, "amina@example.com".to_owned());
    let for_b = (other_thread.0, "bella@example.com".to_owned());
    assert!(
        owners.contains(&for_a) && owners.contains(&for_b),
        "{owners:?}"
    );
    // in the order of the calls: a's requests, then b's, then a's
    let first_b = owners.iter().position(|o| *o == for_b).unwrap();
    let last_b = owners.iter().rposition(|o| *o == for_b).unwrap();
    assert!(owners[..first_b].iter().all(|o| *o == for_a));
    assert!(owners[first_b..=last_b].iter().all(|o| *o == for_b));
    assert!(owners[last_b + 1..].iter().all(|o| *o == for_a));
}

#[tokio::test]
async fn the_legacy_path_is_unchanged_a_bearer_and_initialize() {
    // a server that is not configured for any of it: what it always was
    let server =
        FakeToolServer::spawn(FakeToolServerOptions::default().bearer("tok-0123456789-abc")).await;
    let endpoint = ToolServerEndpoint::new("plain", server.url(), Duration::from_secs(10))
        .with_bearer(ToolSecret::new("tok-0123456789-abc"))
        // for whom is only ever used by a server that signs
        .with_acting_for(thread(), UserId::new("amina@example.com"));
    let tools = client().list_tools(&endpoint).await.unwrap();
    assert_eq!(tools.len(), 4);
    let seen = server.seen();
    assert!(!seen.is_empty());
    for request in &seen {
        assert_eq!(request.bearer.as_deref(), Some("tok-0123456789-abc"));
        assert!(
            request.header(HEADER).is_none(),
            "an unsigned server is sent no claims"
        );
    }
    assert!(server.signed_requests().is_empty());
}

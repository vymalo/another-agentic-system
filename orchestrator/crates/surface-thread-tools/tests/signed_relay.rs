//! The relay and a tool server that takes signed requests (ADR 0067, `docs/api/signed-tool-requests-v1.md`), all of it real
//! but the agent: the real relay, the real MCP client over HTTP, and a `FakeToolServer` that speaks only MCP `2026-07-28` and
//! verifies every request. What only the three together show: the request is signed **for the thread's owner and the thread
//! the agent is working on**, by whichever agent calls; a server that is not configured for signing is sent a bearer and
//! learns neither.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::time::Duration;

use base64::Engine as _;
use orch_app::{AppConfig, ToolServerInfo};
use orch_core::{StepState, UserId};
use orch_ports::{ToolProtocol, ToolSecret, ToolServerEndpoint, ToolSigning};
use orch_signed_request::SigningKey;
use orch_surface_thread_tools::RelayTools;
use orch_testsupport::{FakeToolServer, FakeToolServerOptions};
use orch_tools_mcp::McpToolClient;
use serde_json::{Value, json};
use support::*;

const AUDIENCE: &str = "https://api.vaam.example/mcp";
const SEED: &[u8; 32] = b"throwaway-ed25519-seed-do-not-us";
const BEARER: &str = "plain-bearer-0123456789-not-real";

fn pem() -> String {
    let mut der = vec![
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20,
    ];
    der.extend_from_slice(SEED);
    let body = base64::engine::general_purpose::STANDARD.encode(der);
    format!(
        "-----BEGIN PRIVATE {0}-----\n{body}\n-----END PRIVATE {0}-----\n",
        "KEY"
    )
}

struct Rig {
    h: Harness,
    signed: FakeToolServer,
    plain: FakeToolServer,
}

async fn rig() -> Rig {
    let signed = FakeToolServer::spawn(FakeToolServerOptions::default().modern_only().signed(
        AUDIENCE,
        &[SigningKey::from_seed(SEED).unwrap().public_key()],
    ))
    .await;
    let plain = FakeToolServer::spawn(FakeToolServerOptions::default().bearer(BEARER)).await;
    let timeout = Duration::from_secs(10);
    let servers = vec![
        (
            ToolServerInfo::new("vaam", "Vaam"),
            ToolServerEndpoint::new("vaam", signed.url(), timeout)
                .with_protocol(ToolProtocol::V2026_07_28)
                .with_signing(ToolSigning {
                    key: ToolSecret::new(pem()),
                    audience: AUDIENCE.to_owned(),
                    ttl_secs: 30,
                }),
        ),
        (
            ToolServerInfo::new("plain", "Plain"),
            ToolServerEndpoint::new("plain", plain.url(), timeout)
                .with_bearer(ToolSecret::new(BEARER)),
        ),
    ];
    let cfg = AppConfig {
        tool_servers: servers.iter().map(|(info, _)| info.clone()).collect(),
        ..AppConfig::default()
    };
    let h = Harness::start_full(keys(), cfg, |app, config| {
        config.with_provider(
            RelayTools::new(app.clone(), McpToolClient::new().unwrap(), servers).unwrap(),
        )
    })
    .await;
    Rig { h, signed, plain }
}

#[tokio::test]
async fn the_relay_signs_each_request_for_the_threads_owner_and_a_plain_server_learns_neither() {
    let r = rig().await;
    let thread = r.h.working_thread("plain").await;
    r.h.app
        .set_tools(
            &UserId::new(ALICE),
            thread,
            vec!["vaam".to_owned(), "plain".to_owned()],
        )
        .await
        .unwrap();
    let client = connect(&r.h.url(thread), &grant_token(thread, "plain")).await;

    // the listing: both servers' tools, each listed with a request of its own
    let names = tool_names(&client).await;
    assert!(names.iter().any(|n| n == "vaam__echo"), "{names:?}");
    assert!(names.iter().any(|n| n == "plain__echo"), "{names:?}");

    // a call of each
    let signed = call(&client, "vaam__echo", json!({"text": "for alice"})).await;
    assert!(
        !signed.is_error && signed.text.contains("for alice"),
        "{signed:?}"
    );
    let plain = call(&client, "plain__echo", json!({"text": "plain"})).await;
    assert!(!plain.is_error && plain.text.contains("plain"), "{plain:?}");

    // the signed server: every request was let through, for the thread and its owner, with no bearer
    let requests = r.signed.signed_requests();
    assert!(
        requests.len() >= 3,
        "discover, list and call: {}",
        requests.len()
    );
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(first["method"], "server/discover", "no initialize");
    let mut ctis = std::collections::BTreeSet::new();
    for request in &requests {
        assert!(request.accepted(), "{request:?}");
        assert!(!request.had_authorization);
        let verified = request.verdict.as_ref().unwrap();
        assert_eq!(verified.thread, thread.0);
        assert_eq!(verified.owner, ALICE);
        assert!(ctis.insert(verified.cti));
    }
    let called: Vec<Value> = requests
        .iter()
        .map(|r| serde_json::from_slice::<Value>(&r.body).unwrap())
        .filter(|body| body["method"] == "tools/call")
        .collect();
    assert_eq!(called.len(), 1);
    assert_eq!(called[0]["params"]["name"], "echo");

    // the plain server: the bearer it always got, no claims, and nothing of the thread or the person anywhere in a header
    let seen = r.plain.seen();
    assert!(!seen.is_empty());
    for request in &seen {
        assert_eq!(request.bearer.as_deref(), Some(BEARER));
        assert!(request.header("tool-request-claims").is_none());
        for (name, value) in &request.headers {
            assert!(
                !value.contains(ALICE) && !value.contains(&thread.to_string()),
                "{name}: {value}"
            );
        }
    }
    assert!(r.plain.signed_requests().is_empty());

    // and the call was one step of the thread, completed, whose record holds no key and no claim
    let steps = r.h.events(thread).await;
    let text = format!("{steps:?}");
    assert!(
        !text.contains("BEGIN PRIVATE") && !text.contains("MC4CAQAw"),
        "a key reached the log"
    );
    for request in &requests {
        for header in &request.claims_headers {
            assert!(!text.contains(header.as_str()), "a claim reached the log");
        }
    }
    let completed = steps
        .iter()
        .filter_map(|e| match &e.body {
            orch_core::EventBody::AgentStep(data) => Some(data),
            _ => None,
        })
        .any(|step| step.state == StepState::Completed && step.label.starts_with("Vaam"));
    assert!(completed, "the call is a completed step");
}

#[tokio::test]
async fn a_signed_server_that_refuses_the_signature_is_told_to_the_agent_as_refused_credentials() {
    // the server pins another key: every request is refused, and the agent is told what it is told of any server
    // that refuses the orchestrator's credentials, with no claim and no key in the words
    let other = SigningKey::from_seed(&[9; 32]).unwrap().public_key();
    let signed = FakeToolServer::spawn(
        FakeToolServerOptions::default()
            .modern_only()
            .signed(AUDIENCE, &[other]),
    )
    .await;
    let servers = vec![(
        ToolServerInfo::new("vaam", "Vaam"),
        ToolServerEndpoint::new("vaam", signed.url(), Duration::from_secs(10))
            .with_protocol(ToolProtocol::V2026_07_28)
            .with_signing(ToolSigning {
                key: ToolSecret::new(pem()),
                audience: AUDIENCE.to_owned(),
                ttl_secs: 30,
            }),
    )];
    let cfg = AppConfig {
        tool_servers: servers.iter().map(|(info, _)| info.clone()).collect(),
        ..AppConfig::default()
    };
    let h = Harness::start_full(keys(), cfg, |app, config| {
        config.with_provider(
            RelayTools::new(app.clone(), McpToolClient::new().unwrap(), servers).unwrap(),
        )
    })
    .await;
    let thread = h.working_thread("plain").await;
    h.app
        .set_tools(&UserId::new(ALICE), thread, vec!["vaam".to_owned()])
        .await
        .unwrap();
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;
    // nothing can be listed, so nothing is offered
    assert_eq!(tool_names(&client).await, ["get_ui_catalog", "turn_output"]);
    assert!(!signed.signed_requests().is_empty());
    assert!(signed.signed_requests().iter().all(|r| !r.accepted()));
}

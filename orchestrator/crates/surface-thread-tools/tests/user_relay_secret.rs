//! The credential of a person's own server, planted as a sentinel, is in the request to that server
//! and **nowhere else** (ADR 0046, decision 3: "a test that plants a sentinel secret and greps every
//! output is the acceptance of the slice"; slice 4b did it for the API, this is the relay's half).
//!
//! A test of its own, in its own process: the logs are captured with a subscriber set for the test
//! (every level, under the filter the binary builds for `RUST_LOG=trace`: `rmcp`, the MCP library, is
//! off unless an operator names it, and `user_relay_logs_*.rs` is what proves that) and read by it, and a process that runs other tests at the same time would give the callsites
//! another interest. The runtime is the current-thread one, so every task of the stack (the router,
//! the dispatcher, the relay, the broker) logs through the subscriber of the test.
//!
//! Three servers are used: one that wants a bearer, one that wants a key in a header, one that
//! refuses what it is given (a 401). The agent lists their tools, calls them, calls a tool that does
//! not exist, calls one that fails, calls the server that refuses, and calls with the secret store
//! down; then the person's views are read (the pickers, the thread, its export, the AG-UI stream, the
//! log of events). Everything the agent, the person and the operator can read is searched for each
//! secret. A canary shows that the capture would find a secret if one were logged, and the sealed
//! store shows that the secrets are really held (a search of the outputs that finds nothing proves
//! little unless the secrets exist).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_ports::{EgressPolicy, HostPattern, SecretServer, SecretStore, UserToolServerId};
use orch_testsupport::{Chat, FakeToolServer, FakeToolServerOptions};
use orch_tools_mcp::McpToolClient;
use serde_json::{Value, json};
use support::wire::WireRig;
use support::{ALICE, call, try_call};
use tracing_subscriber::fmt::format::FmtSpan;

const BEARER: &str = "SENTINEL-bearer-5c0e91d7a2";
const KEY: &str = "SENTINEL-apikey-93b4f10c68";
/// Typed for the server that refuses it.
const REFUSED: &str = "SENTINEL-refused-e72a5d1b04";
/// In the path of the URL of a server, as a webhook-style URL has one: the person sees their own URL
/// and nothing else may show it.
const IN_PATH: &str = "SENTINEL-in-the-path-4be7c20a1d";
/// Typed for the server that says it back.
const LEAKED: &str = "SENTINEL-leaked-1b9d03e6f2";
const SECRETS: [&str; 4] = [BEARER, KEY, REFUSED, LEAKED];

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn pattern(text: &str) -> HostPattern {
    text.parse().unwrap()
}

#[tokio::test]
async fn a_secret_lent_for_a_call_is_in_the_request_to_the_server_and_nowhere_else() {
    let captured = Captured::default();
    // Every level of every crate of ours, with the filter the binary builds for `RUST_LOG=trace`
    // (`orch_log_filter`): the MCP library stays off unless an operator names it, so there is no
    // `rmcp::` line here to exempt. What the library says is `user_relay_logs_*.rs`'s.
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(orch_log_filter::env_filter(Some("trace")))
        .with_span_events(FmtSpan::FULL)
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    // a canary: the capture finds a secret if one is logged
    tracing::info!(canary = BEARER, "the canary");

    let bearer_server = FakeToolServer::spawn(
        FakeToolServerOptions::default()
            .bearer(BEARER)
            .at_path(format!("/{IN_PATH}/mcp")),
    )
    .await;
    let key_server =
        FakeToolServer::spawn(FakeToolServerOptions::default().require_header("X-Api-Key", KEY))
            .await;
    let refusing =
        FakeToolServer::spawn(FakeToolServerOptions::default().bearer("another-token")).await;
    let leaky =
        FakeToolServer::spawn(FakeToolServerOptions::default().bearer(LEAKED).leaky()).await;
    let policy = EgressPolicy::new()
        .allowing_http()
        .with_allow(pattern("127.0.0.1"));
    let rig = WireRig::start(policy.clone(), McpToolClient::guarded(policy).unwrap()).await;

    // everything the person's client and the agent's client saw
    let seen = Mutex::new(String::new());
    let note =
        |what: &str, text: &str| seen.lock().unwrap().push_str(&format!("[{what}] {text}\n"));
    // what the person is shown of their own servers, URLs included: a person's URL is theirs
    let own_views = Mutex::new(String::new());
    let own = |what: &str, text: &str| {
        own_views
            .lock()
            .unwrap()
            .push_str(&format!("[{what}] {text}\n"));
    };

    // the person adds the servers, with the secrets in the request bodies, as the web does
    let mut ids = Vec::new();
    for body in [
        json!({"name": "Tracker", "url": bearer_server.url(), "auth": {"kind": "bearer"}, "secret": BEARER}),
        json!({"name": "Search", "url": key_server.url(),
               "auth": {"kind": "api_key", "header": "X-Api-Key"}, "secret": KEY}),
        json!({"name": "Locked", "url": refusing.url(), "auth": {"kind": "bearer"}, "secret": REFUSED}),
        json!({"name": "Leaky", "url": leaky.url(), "auth": {"kind": "bearer"}, "secret": LEAKED}),
    ] {
        let (status, text) = rig
            .api(reqwest::Method::POST, "/api/user-tool-servers", Some(body))
            .await;
        assert_eq!(status, 201, "{text}");
        own("add", &text);
        ids.push(
            serde_json::from_str::<Value>(&text).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let [tracker, search, locked, leaking] = [&ids[0], &ids[1], &ids[2], &ids[3]];

    // the control: the broker does hold the secrets, sealed in its store, so a search that finds
    // nothing elsewhere is a search that could have found something
    for (id, secret) in [
        (tracker, BEARER),
        (search, KEY),
        (locked, REFUSED),
        (leaking, LEAKED),
    ] {
        let id = UserToolServerId::parse(id).unwrap();
        let (_, material) = rig
            .secrets
            .get(&WireRig::alice(), &SecretServer::credential(&id))
            .await
            .unwrap()
            .expect("a credential is kept");
        assert!(String::from_utf8_lossy(material.expose()).contains(secret));
    }

    let thread = rig
        .working_thread(&[tracker, search, locked, leaking])
        .await;
    let client = rig.client(thread).await;

    // the agent: the list, the calls that work, a tool that fails, a tool that does not exist
    let tools = client.list_all_tools().await.unwrap();
    note("agent list", &serde_json::to_string(&tools).unwrap());
    for (id, arguments) in [
        (tracker, json!({"text": "one"})),
        (search, json!({"text": "two"})),
    ] {
        let out = call(&client, &format!("{id}__echo"), arguments).await;
        assert!(!out.is_error, "{out:?}");
        note("agent call", &format!("{out:?}"));
    }
    let failing = call(&client, &format!("{tracker}__fail"), json!({})).await;
    assert!(failing.is_error);
    note("agent call", &format!("{failing:?}"));
    let unknown = call(&client, &format!("{tracker}__nothing"), json!({})).await;
    assert!(unknown.is_error);
    note("agent call", &format!("{unknown:?}"));
    // a name that is not on the endpoint at all is a protocol error
    let error = try_call(&client, "no-such-server__echo", json!({}))
        .await
        .unwrap_err();
    note("agent error", &format!("{error} {error:?}"));

    // the server that says the credential back, in a result, in a failed tool and in an error
    for tool in ["echo", "fail", "nothing"] {
        let out = call(
            &client,
            &format!("{leaking}__{tool}"),
            json!({"text": "hi"}),
        )
        .await;
        assert!(out.text.contains("[redacted]"), "{out:?}");
        note("agent call", &format!("{out:?}"));
    }
    // the server that refuses: the person is told to sign in again; the secret is not repeated
    let refused = call(&client, &format!("{locked}__echo"), json!({})).await;
    assert!(refused.is_error);
    assert_eq!(refused.text, orch_surface_thread_tools::NEEDS_SIGN_IN);
    note("agent call", &format!("{refused:?}"));
    // and with the secret store down, a call to a server that is fine fails without it
    rig.secrets.set_down(true);
    let down = call(&client, &format!("{tracker}__echo"), json!({})).await;
    assert!(down.is_error);
    assert!(down.text.contains("could not be had now"), "{}", down.text);
    note("agent call", &format!("{down:?}"));
    let down_list = client.list_all_tools().await.unwrap();
    note("agent list", &serde_json::to_string(&down_list).unwrap());
    rig.secrets.set_down(false);

    // the views of the person, while the thread still runs
    for path in [
        "/api/tool-servers".to_owned(),
        "/api/user-tool-servers".to_owned(),
        format!("/api/threads/{thread}"),
    ] {
        let (status, text) = rig.api(reqwest::Method::GET, &path, None).await;
        assert_eq!(status, 200, "{path}: {text}");
        if path == "/api/user-tool-servers" {
            own(&path, &text);
        } else {
            note(&path, &text);
        }
    }
    // an attach that is refused, and one that works, by the route
    let other = rig.working_thread(&[]).await;
    for ids in [vec![locked.clone()], vec![tracker.clone()]] {
        let (_, text) = rig
            .api(
                reqwest::Method::PUT,
                &format!("/api/threads/{other}/tools"),
                Some(json!({"servers": ids})),
            )
            .await;
        note("attach", &text);
    }

    // end the thread, and read the rest: the export, the AG-UI stream, the log
    rig.app.cancel(&WireRig::alice(), thread).await.unwrap();
    orch_testsupport::eventually("the thread to end", || async {
        let record = rig.app.get_thread(&WireRig::alice(), thread).await.unwrap();
        record.state.is_terminal().then_some(())
    })
    .await;
    let chat = Chat::new(&rig.base, ALICE);
    let (status, export) = chat.get(&format!("/api/threads/{thread}/export")).await;
    assert_eq!(status, 200);
    note("export", &export.to_string());
    let frames = chat
        .agui_connect(&thread.to_string(), None, true)
        .await
        .collect_frames(Duration::from_secs(20))
        .await;
    assert!(!frames.is_empty());
    note(
        "AG-UI frames",
        &serde_json::to_string(&frames.iter().map(|f| &f.event).collect::<Vec<_>>()).unwrap(),
    );
    let events = rig
        .app
        .list_events(&WireRig::alice(), thread, 0, 1000)
        .await
        .unwrap();
    note("events", &serde_json::to_string(&events).unwrap());
    let steps = events
        .iter()
        .filter(|e| matches!(e.body, orch_core::EventBody::AgentStep(_)))
        .count();
    assert!(steps >= 10, "the calls were steps of the thread: {steps}");

    // what was said to the servers: the secrets, exactly where they were meant to go
    let tracker_seen = bearer_server.seen();
    assert!(
        tracker_seen
            .iter()
            .all(|r| r.bearer.as_deref() == Some(BEARER)),
        "{tracker_seen:?}"
    );
    assert!(tracker_seen.iter().any(|r| r.method == "tools/call"));
    assert!(
        key_server
            .seen()
            .iter()
            .all(|r| r.headers.iter().any(|(n, v)| n == "x-api-key" && v == KEY))
    );
    // the server that refuses served nothing, and never saw the typed secret as anything but a
    // refused bearer (it answers before speaking MCP)
    assert!(refusing.seen().is_empty());

    // nothing else holds a secret
    let logs = captured.text();
    assert!(
        logs.contains(r#"canary=\"SENTINEL-bearer-5c0e91d7a2\""#)
            || logs.contains("SENTINEL-bearer-5c0e91d7a2"),
        "the capture does not see what is logged"
    );
    assert!(
        logs.contains("a server of a person's") && logs.contains("needs_reauth"),
        "the relay logged nothing about the server that refused:\n{logs}"
    );
    let outputs = seen.lock().unwrap().clone();
    // the person is shown their own URL, path included (the control), and nothing else shows it
    assert!(own_views.lock().unwrap().contains(IN_PATH));
    assert!(
        !outputs.contains(IN_PATH),
        "the path of a URL is in what the client and the agent saw"
    );
    assert_eq!(
        logs.matches(IN_PATH).count(),
        0,
        "the path of a URL is in the logs"
    );
    for secret in SECRETS {
        assert!(
            !outputs.contains(secret),
            "{secret} is in what the client and the agent saw"
        );
        // the logs hold the canary and nothing else of the bearer
        let in_logs = logs.matches(secret).count();
        let expected = usize::from(secret == BEARER); // the canary line
        assert_eq!(in_logs, expected, "{secret} is in the logs {in_logs} times");
    }
}

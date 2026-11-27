//! The window tools through the endpoint (ADR 0092, `docs/api/windows-v1.md`): over real HTTP
//! with the token the adapter mints, on the fake adapter of `orch-window` (no network). They are
//! listed to the addressed agent only; open, look, act and close answer what the contract says;
//! every refusal is a tool error that starts with the contract's code; and a call by an asked
//! agent is refused **before any adapter call**, which the counting adapter below shows.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use orch_app::AskCall;
use orch_core::{AgentId, Caller, ThreadId};
use orch_surface_thread_tools::{ThreadToolsConfig, WindowTools};
use orch_window::testkit::FakeWindow;
use orch_window::{Action, Frame, Opened, Window, WindowError, WindowId};
use serde_json::{Value, json};
use support::{Harness, call, connect, grant_token, keys, tool_names};

/// The fake adapter, with a count of the calls that reached it: a refusal that happens before the
/// adapter leaves the count where it was.
struct Counting {
    inner: FakeWindow,
    calls: AtomicUsize,
}

impl Counting {
    fn new() -> Arc<Self> {
        Arc::new(Counting {
            inner: FakeWindow::new(),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Window for Counting {
    async fn open(&self, thread: ThreadId, url: &str) -> Result<Opened, WindowError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.open(thread, url).await
    }

    async fn look(&self, thread: ThreadId, id: &WindowId) -> Result<Option<Frame>, WindowError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.look(thread, id).await
    }

    async fn act(
        &self,
        thread: ThreadId,
        id: &WindowId,
        actions: &[Action],
    ) -> Result<Frame, WindowError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.act(thread, id, actions).await
    }

    async fn close(&self, thread: ThreadId, id: &WindowId) -> Result<bool, WindowError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.close(thread, id).await
    }
}

const PAGE: &str = "http://93.184.216.34/";

async fn harness(window: &Arc<Counting>) -> Harness {
    let window = Arc::clone(window);
    Harness::start_with(keys(), move |config: ThreadToolsConfig| {
        config.with_provider(WindowTools::new(window))
    })
    .await
}

#[tokio::test]
async fn the_four_tools_are_listed_to_the_addressed_agent_after_the_built_ins() {
    let window = Counting::new();
    let h = harness(&window).await;
    let thread = h.thread("plain").await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;

    let names = tool_names(&client).await;
    assert_eq!(
        names,
        [
            "get_ui_catalog",
            "turn_output",
            "window_open",
            "window_look",
            "window_act",
            "window_close"
        ]
    );
    for tool in client.list_all_tools().await.unwrap() {
        if tool.name.starts_with("window_") {
            assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
            assert!(tool.output_schema.is_some(), "{}", tool.name);
        }
    }
    assert_eq!(window.calls(), 0, "listing never reaches the adapter");
}

#[tokio::test]
async fn a_window_is_opened_looked_at_acted_on_and_closed() {
    let window = Counting::new();
    let h = harness(&window).await;
    let thread = h.thread("plain").await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;

    let opened = call(
        &client,
        "window_open",
        json!({"url": PAGE, "label": "the page"}),
    )
    .await;
    assert!(!opened.is_error, "{}", opened.text);
    let id = opened.value["window"].as_str().unwrap().to_owned();
    assert!(!id.is_empty());
    let first = opened.value["frame"].clone();
    assert_eq!(first["mediaType"], "image/png");
    // a frame is by reference, never the bytes
    assert!(first["url"].as_str().unwrap().starts_with("artifact:"));
    assert_eq!(window.calls(), 1);

    let looked = call(&client, "window_look", json!({"window": id})).await;
    assert!(!looked.is_error, "{}", looked.text);
    assert_eq!(looked.value["frame"], first, "a look changes nothing");

    let acted = call(
        &client,
        "window_act",
        json!({"window": id, "actions": [
            {"click": {"x": 12, "y": 34}},
            {"type": {"text": "hello"}},
            {"scroll": {"dy": -5}},
            {"key": {"name": "Enter"}}
        ]}),
    )
    .await;
    assert!(!acted.is_error, "{}", acted.text);
    assert_ne!(acted.value["frame"], first, "the frame after the actions");
    assert_eq!(window.calls(), 3);

    let closed = call(&client, "window_close", json!({"window": id})).await;
    assert!(!closed.is_error, "{}", closed.text);
    assert_eq!(closed.value, json!({"closed": true}));
    // idempotent: a second close is a success that says nothing was open
    let again = call(&client, "window_close", json!({"window": id})).await;
    assert!(!again.is_error, "{}", again.text);
    assert_eq!(again.value, json!({"closed": false}));
    // a look at a closed window is an answer, not an error
    let gone = call(&client, "window_look", json!({"window": id})).await;
    assert!(!gone.is_error, "{}", gone.text);
    assert_eq!(gone.value, json!({"frame": null, "state": "closed"}));
    assert_eq!(window.calls(), 6);
}

#[tokio::test]
async fn a_refusal_is_a_tool_error_that_starts_with_the_contracts_code() {
    let window = Counting::new();
    let h = harness(&window).await;
    let thread = h.thread("plain").await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;

    // the adapter's guard: loopback is refused, and the adapter was asked
    let loopback = call(
        &client,
        "window_open",
        json!({"url": "http://127.0.0.1:5173/"}),
    )
    .await;
    assert!(loopback.is_error);
    assert!(
        loopback.text.starts_with("url_refused: "),
        "{}",
        loopback.text
    );
    assert_eq!(window.calls(), 1);

    // a window this thread has not got
    let unknown = call(
        &client,
        "window_act",
        json!({"window": "w-nope", "actions": [{"key": {"name": "Enter"}}]}),
    )
    .await;
    assert!(unknown.is_error);
    assert!(
        unknown.text.starts_with("window_unknown: "),
        "{}",
        unknown.text
    );

    // the cap of four windows
    let calls_before = window.calls();
    for _ in 0..4 {
        let opened = call(&client, "window_open", json!({"url": PAGE})).await;
        assert!(!opened.is_error, "{}", opened.text);
    }
    let fifth = call(&client, "window_open", json!({"url": PAGE})).await;
    assert!(fifth.is_error);
    assert!(
        fifth.text.starts_with("too_many_windows: "),
        "{}",
        fifth.text
    );
    assert_eq!(window.calls(), calls_before + 5);
}

#[tokio::test]
async fn what_the_contract_calls_invalid_never_reaches_the_adapter() {
    let window = Counting::new();
    let h = harness(&window).await;
    let thread = h.thread("plain").await;
    let client = connect(&h.url(thread), &grant_token(thread, "plain")).await;

    let opened = call(&client, "window_open", json!({"url": PAGE})).await;
    let id = opened.value["window"].as_str().unwrap().to_owned();
    let before = window.calls();

    let nine: Vec<Value> = (0..9).map(|n| json!({"scroll": {"dy": n}})).collect();
    let too_many = call(
        &client,
        "window_act",
        json!({"window": id, "actions": nine}),
    )
    .await;
    assert!(too_many.is_error);
    assert!(too_many.text.starts_with("invalid: "), "{}", too_many.text);
    // exactly at the bound is accepted
    let eight: Vec<Value> = (0..8).map(|n| json!({"scroll": {"dy": n}})).collect();
    let at_bound = call(
        &client,
        "window_act",
        json!({"window": id, "actions": eight}),
    )
    .await;
    assert!(!at_bound.is_error, "{}", at_bound.text);

    for (tool, args) in [
        (
            "window_act",
            json!({"window": id, "actions": [{"swipe": {}}]}),
        ),
        ("window_act", json!({"window": id})),
        ("window_open", json!({"url": 7})),
        ("window_open", json!({"url": PAGE, "extra": true})),
        ("window_look", json!({})),
        ("window_close", json!({"window": id, "extra": 1})),
    ] {
        let outcome = call(&client, tool, args.clone()).await;
        assert!(outcome.is_error, "{tool} {args}");
        assert!(
            outcome.text.starts_with("invalid: "),
            "{tool} {args}: {}",
            outcome.text
        );
    }
    let long = format!("http://93.184.216.34/{}", "a".repeat(3000));
    let over = call(&client, "window_open", json!({"url": long})).await;
    assert!(over.is_error);
    assert!(over.text.starts_with("url_refused: "), "{}", over.text);

    // only the one accepted act (at the bound) reached the adapter
    assert_eq!(window.calls(), before + 1);
}

/// The decisive test of the gate: an asked agent is not the thread's main caller, so it is not
/// offered the tools, and a call by name is refused with `not_main_caller` **before any adapter
/// call**. Delete the `is_main` check in `WindowTools::call` and this fails: the call reaches the
/// adapter (the count moves) and answers a window instead of an error.
#[tokio::test]
async fn an_asked_agent_is_refused_before_any_adapter_call() {
    let window = Counting::new();
    let h = harness(&window).await;
    let thread = h.asking_thread("plain", "slow", &["coder"]).await;
    h.app
        .ask(AskCall {
            thread,
            job: 1,
            caller: Caller::Main,
            asker: AgentId::new("plain"),
            agent: AgentId::new("coder"),
            text: "slow work".to_owned(),
            call_id: Some("c1".to_owned()),
            parent_step: None,
            timeout: None,
        })
        .await
        .unwrap();
    let coder = connect(&h.url(thread), &support::ask_token(thread, 1, "coder", 1)).await;

    // none of the four is offered
    let names = tool_names(&coder).await;
    assert!(names.iter().all(|n| !n.starts_with("window_")), "{names:?}");

    // a call by name is answered with the reason, as a tool error, and nothing is asked of the adapter
    for (tool, args) in [
        ("window_open", json!({"url": PAGE})),
        ("window_look", json!({"window": "w-1"})),
        (
            "window_act",
            json!({"window": "w-1", "actions": [{"key": {"name": "Enter"}}]}),
        ),
        ("window_close", json!({"window": "w-1"})),
        // not even the arguments are read: a malformed call is refused for the caller first
        ("window_open", json!({})),
    ] {
        let outcome = call(&coder, tool, args).await;
        assert!(outcome.is_error, "{tool}: {}", outcome.text);
        assert!(
            outcome.text.starts_with("not_main_caller: "),
            "{tool}: {}",
            outcome.text
        );
    }
    assert_eq!(window.calls(), 0, "the adapter was never asked");
}

//! What a person searched for is in no log line (ADR 0068). In a test binary of its own, because it captures
//! the logs of the process: another test's callsite caches would otherwise decide what is recorded.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use orch_api::ApiConfig;
use orch_app::{AgentDirectory, AgentEntry, App, AppConfig};
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ThreadId, ThreadState, UserId, UserMessageData,
};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    AgentEndpoint, Commit, FixedRegistry, NewEvent, NewThreadRecord, NoModel, PortSet, Principal,
    Role, SystemClock, ThreadStore,
};
use serde_json::{Value, json};

type Stack = PortSet<
    MemoryStore,
    MemoryWakeup,
    ScriptedAgent,
    SystemClock,
    SeqIds,
    NoModel,
    FixedRegistry,
    MemoryAuth,
    MemoryArtifacts,
>;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

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

fn commit(events: Vec<NewEvent>) -> Commit {
    Commit {
        new_state: ThreadState::Done,
        job: None,
        events,
        outbox: Vec::new(),
        binding: None,
        now: Timestamp::UNIX_EPOCH,
        lease: None,
        watches: Vec::new(),
        timers: Vec::new(),
        inbox: None,
        finishes_outbox: None,
        title: None,
        description: None,
        sharing: None,
        skip_unsent_delegates: false,
    }
}

/// Searches that are answered, refused and not understood: no word of any of them is in the log, at any
/// level, and the answer to a body that does not parse does not quote it.
#[tokio::test]
async fn nothing_of_a_query_is_in_any_log_line() {
    let captured = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish(),
    )
    .unwrap();
    let alice = "alice@example.com";
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("plain"),
            "https://plain.example.com/.well-known/agent-card.json".to_owned(),
            None,
        ),
        name: "plain".to_owned(),
    }]);
    let auth = MemoryAuth::new();
    auth.allow(
        "alice",
        Principal {
            roles: [Role::new("user")].into_iter().collect(),
            ..Principal::of(UserId::new(alice))
        },
    );
    let stack: Stack = PortSet {
        artifacts: MemoryArtifacts::new(),
        store: MemoryStore::new(),
        wakeup: MemoryWakeup::new(),
        agents: ScriptedAgent::new(),
        clock: SystemClock,
        ids: SeqIds::default(),
        model: NoModel,
        auth,
        registry: directory.fixed_registry(),
        user_tools: orch_ports::NoUserTools::off(),
    };
    let app = Arc::new(
        App::new(
            stack,
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .unwrap(),
    );
    app.ports()
        .store
        .create_thread(
            NewThreadRecord {
                id: ThreadId(uuid::Uuid::from_u128(
                    0x0190_aaaa_0000_7000_8000_0000_0000_0001,
                )),
                owner: UserId::new(alice),
                title: "Zyxwvut notes".to_owned(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                },
                context_id: None,
                rail_parent: None,
                now: Timestamp::UNIX_EPOCH,
            },
            commit(vec![NewEvent {
                at: Timestamp::UNIX_EPOCH,
                actor: Actor::user(&UserId::new(alice)),
                body: EventBody::UserMessage(UserMessageData::new("the plugh word")),
                idempotency_key: None,
            }]),
        )
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), Vec::new());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let post = |body: Vec<u8>| {
        client
            .post(format!("{base}/api/search/threads"))
            .bearer_auth("alice")
            .header("Content-Type", "application/json")
            .body(body)
            .send()
    };

    for q in [
        "zyxwvut Plugh Xyzzyquux",
        "zyxwvut plugh",
        "' OR 1=1 -- Hunter2secret",
        "!!",
    ] {
        let answered = post(serde_json::to_vec(&json!({ "q": q })).unwrap())
            .await
            .unwrap();
        assert_eq!(answered.status(), 200);
        let page: Value = answered.json().await.unwrap();
        // the title and the message are two documents: no one holds both words
        assert_eq!(page["results"].as_array().unwrap().len(), 0, "{q}");
    }
    let bad_cursor =
        post(serde_json::to_vec(&json!({"q": "Hunter2secret", "cursor": "Cursorsecret"})).unwrap())
            .await
            .unwrap();
    assert_eq!(bad_cursor.status(), 400);
    let bad_body = post(b"{\"q\": \"Badbodysecret\", \"owner\": 1}".to_vec())
        .await
        .unwrap();
    assert_eq!(bad_body.status(), 400);
    assert!(
        !bad_body.text().await.unwrap().contains("Badbodysecret"),
        "the answer does not quote the body"
    );
    // the store cannot answer: 503, and still nothing of what was asked in the log
    app.ports().store.set_search_down(true);
    let down = post(serde_json::to_vec(&json!({"q": "Downsecretword zyxwvut"})).unwrap())
        .await
        .unwrap();
    assert_eq!(down.status(), 503);
    assert_eq!(down.headers()["retry-after"], "5");
    let down: Value = down.json().await.unwrap();
    assert_eq!(down["code"], "search_unavailable");
    server.abort();

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("/api/search/threads"),
        "the request is logged, without its body:\n{log}"
    );
    assert!(
        log.contains("a search could not be answered"),
        "the 503 is logged, with its cause and not the query:\n{log}"
    );
    assert!(
        log.contains("threads searched"),
        "the route says how many:\n{log}"
    );
    for word in [
        "zyxwvut",
        "Zyxwvut",
        "plugh",
        "Plugh",
        "xyzzyquux",
        "Xyzzyquux",
        "Hunter2secret",
        "hunter2secret",
        "Cursorsecret",
        "Badbodysecret",
        "Downsecretword",
        "downsecretword",
        "DROP",
        "1=1",
    ] {
        assert!(!log.contains(word), "{word} is in the log:\n{log}");
    }
}

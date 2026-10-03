//! The token of a share link stays out of the request span (ADR 0040, section 11).
//!
//! A test of its own, in its own process: a tracing subscriber that is set for one test and read
//! by it must not share the process with tests that run at the same time on other threads (the
//! callsites cache what they are told is of interest).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, NewThread, PublicView, ShareKeys, SharingMode,
    SharingSettings,
};
use orch_core::{AgentId, AgentTarget, ShareLevel, UserId};
use orch_ports::memory::{MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{AgentEndpoint, NoModel, PortSet, Principal, Role, SystemClock};
use secrecy::SecretString;

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

#[tokio::test]
async fn the_token_is_not_in_the_request_span() {
    let directory = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("plain"),
            "https://plain.example.com/.well-known/agent-card.json".to_owned(),
            None,
        ),
        name: "plain".to_owned(),
    }]);
    let auth = MemoryAuth::new();
    let alice = Principal {
        roles: [Role::new("user")].into_iter().collect(),
        ..Principal::of(UserId::new("alice@example.com"))
    };
    auth.allow("alice", alice.clone());
    let keys = ShareKeys::new(
        SecretString::from("0123456789abcdef0123456789abcdef".to_owned()),
        None,
    )
    .unwrap();
    let app = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: NoModel,
                auth,
                registry: directory.fixed_registry(),
            },
            directory,
            AppConfig {
                stream_poll: Duration::from_millis(100),
                sharing: SharingSettings::new(
                    SharingMode::Public,
                    Some(keys),
                    PublicView::default(),
                )
                .unwrap(),
                ..AppConfig::default()
            },
        )
        .unwrap(),
    );
    let thread = app
        .create_thread(
            &alice,
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
    let view = app
        .share_thread(&alice, thread.id, ShareLevel::Public)
        .await
        .unwrap();
    let token = view.url.unwrap().trim_start_matches("/s/").to_owned();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), Vec::new());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let guard = tracing::subscriber::set_default(subscriber);
    let guess = "B".repeat(43);
    for path in [
        format!("/api/public/shared/{token}"),
        format!("/api/shared/{token}"),
        format!("/api/public/shared/{token}/artifacts/{}", "ab".repeat(32)),
        format!("/api/shared/{guess}"),
        format!("/s/{token}"),
    ] {
        client
            .get(format!("{base}{path}"))
            .bearer_auth("alice")
            .send()
            .await
            .unwrap();
    }
    // a request of the owner's own thread is logged as it always was
    client
        .get(format!("{base}/api/threads/{}", thread.id))
        .bearer_auth("alice")
        .send()
        .await
        .unwrap();
    drop(guard);
    server.abort();

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        log.contains("uri=/api/public/shared/…") && log.contains("uri=/api/shared/…"),
        "the paths are in the log, redacted:\n{log}"
    );
    assert!(log.contains("uri=/s/…"), "{log}");
    assert!(!log.contains(&token), "the token is in the log:\n{log}");
    assert!(!log.contains(&guess), "a guessed token is in the log");
    assert!(
        log.contains(&format!("uri=/api/threads/{}", thread.id)),
        "{log}"
    );
}

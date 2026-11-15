//! What a prompt holds is in no log line (ADR 0070). In a test binary of its own, because it captures the logs
//! of the process: another test's callsite caches would otherwise decide what is recorded.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{
    AgentDirectory, AgentEntry, AgentScope, App, AppConfig, Permission, Policy, RoleGrant,
};
use orch_core::{AgentId, UserId};
use orch_ports::memory::{
    MemoryArtifacts, MemoryAuth, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{AgentEndpoint, FixedRegistry, NoModel, PortSet, Principal, Role, SystemClock};
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

/// Saved, shared, rendered, refused and not understood: no word of a name, a title, a description, a body, an
/// answer or a role is in the log, at any level, and the answer to a body that does not parse does not quote it.
#[tokio::test]
async fn nothing_a_prompt_holds_is_in_any_log_line() {
    let captured = Captured::default();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(captured.clone())
            .with_ansi(false)
            .finish(),
    )
    .unwrap();
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
            roles: [Role::new("sharer")].into_iter().collect(),
            ..Principal::of(UserId::new("alice@example.com"))
        },
    );
    auth.allow(
        "carol",
        Principal {
            roles: [Role::new("user"), Role::new("zqrolezq")]
                .into_iter()
                .collect(),
            ..Principal::of(UserId::new("carol@example.com"))
        },
    );
    let all: Vec<Permission> = Permission::ALL.to_vec();
    let grant = |permissions: &[Permission]| RoleGrant {
        permissions: permissions.iter().copied().collect(),
        agents: AgentScope::from_patterns(["*"]),
        max_tier: None,
    };
    let policy = Policy::new(
        [
            (Role::new("sharer"), grant(&all)),
            (Role::new("user"), grant(&all)),
        ]
        .into_iter()
        .collect(),
        None,
    )
    .unwrap();
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
                policy,
                ..AppConfig::default()
            },
        )
        .unwrap(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = orch_api::router_with_surfaces(app, ApiConfig::default(), Vec::new());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let send = |token: &'static str, method: &'static str, path: String, body: Option<Value>| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let mut req = client
                .request(method.parse().unwrap(), format!("{base}{path}"))
                .bearer_auth(token);
            if let Some(body) = body {
                req = req
                    .header("Content-Type", "application/json")
                    .body(serde_json::to_vec(&body).unwrap());
            }
            let resp = req.send().await.unwrap();
            let status = resp.status().as_u16();
            (status, resp.text().await.unwrap())
        }
    };

    let saved = json!({
        "name": "zqnamezq",
        "title": "zqtitlezq",
        "description": "zqdescriptionzq",
        "body": "zqbodyzq {{who}}",
        "variables": [{"id": "who", "question": "zqquestionzq?", "type": "text"}],
        "audience": ["zqrolezq"],
    });
    let (status, text) = send("alice", "POST", "/api/prompts".into(), Some(saved)).await;
    assert_eq!(status, 201, "{text}");
    let id = serde_json::from_str::<Value>(&text).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    // the audience reads it and renders it with an answer of made-up words
    let (status, _) = send("carol", "GET", format!("/api/prompts/{id}"), None).await;
    assert_eq!(status, 200);
    let answers = json!({"answers": [{"id": "who", "values": [], "text": "zqanswerzq"}]});
    let (status, text) = send(
        "carol",
        "POST",
        format!("/api/prompts/{id}/render"),
        Some(answers),
    )
    .await;
    assert_eq!(status, 200);
    assert!(text.contains("zqanswerzq"));
    // refused and not understood: the body is not quoted
    let (status, text) = send(
        "alice",
        "POST",
        "/api/prompts".into(),
        Some(json!({"name": "zqbadnamezq A", "title": "zqtitle2zq", "body": "zqbody2zq {{ x }}"})),
    )
    .await;
    assert_eq!(status, 400);
    assert!(
        !text.contains("zqbody2zq") && !text.contains("zqtitle2zq"),
        "{text}"
    );
    let (status, text) = send(
        "alice",
        "POST",
        format!("/api/prompts/{id}/render"),
        Some(json!({"answers": [{"id": "who", "values": ["zqwrongzq"], "text": "zqanswer2zq"}]})),
    )
    .await;
    assert_eq!(status, 422);
    assert!(
        !text.contains("zqwrongzq") && !text.contains("zqanswer2zq"),
        "{text}"
    );
    let raw = client
        .post(format!("{base}/api/prompts"))
        .bearer_auth("alice")
        .header("Content-Type", "application/json")
        .body(r#"{"name": "zqrawnamezq", "title": "#)
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status().as_u16(), 400);
    let text = raw.text().await.unwrap();
    assert!(!text.contains("zqrawnamezq"), "{text}");
    server.abort();

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        !log.is_empty(),
        "the capture works: the server logged nothing at all"
    );
    for word in [
        "zqnamezq",
        "zqtitlezq",
        "zqdescriptionzq",
        "zqbodyzq",
        "zqquestionzq",
        "zqrolezq",
        "zqanswerzq",
        "zqbadnamezq",
        "zqtitle2zq",
        "zqbody2zq",
        "zqwrongzq",
        "zqanswer2zq",
        "zqrawnamezq",
    ] {
        assert!(!log.contains(word), "{word} is in the log:\n{log}");
    }
}

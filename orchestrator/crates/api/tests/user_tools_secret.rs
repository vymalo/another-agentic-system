//! A secret a person types for their own MCP server is in no response, no log line, no event and no
//! definition (ADR 0046, decision 3: "a test that plants a sentinel secret and greps every output is
//! the acceptance of the slice"; the relay's half is slice 5).
//!
//! A test of its own, in its own process: the logs are captured with a subscriber that is set for the
//! test and read by it, which must not share the process with tests that run at the same time on other
//! threads (the callsites cache what they are told is of interest).
//!
//! Two kinds of secret are planted, a bearer token and an API key, and a third thing that must not be
//! logged either: a secret in the path of a URL. They are put through every route, through the failures
//! (a body that cannot be read, a role without the permission, a URL the guard refuses, the limit, a
//! stale revision, a broker that is down) and through a delete. The captures are of everything the
//! client saw (status, headers, body), everything the process logged at the lowest level with the
//! spans, the thread log, the definitions and every `Debug` the application offers. A canary shows
//! that the capture would find a secret if one were there.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, App, AppConfig, Permission, Policy, RoleGrant};
use orch_core::UserId;
use orch_credential_broker::Broker;
use orch_ports::memory::{
    MemoryAuth, MemoryPendingAuthorizations, MemorySecretStore, MemoryStore, MemoryUserToolServers,
    MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    EgressPolicy, NoArtifacts, NoModel, PortSet, Principal, Role, SecretServer, SecretStore,
    SystemClock, ThreadListing, UserToolServerId, UserToolServerStore, UserTools,
};
use serde_json::{Value, json};
use tracing_subscriber::fmt::format::FmtSpan;

const BEARER: &str = "SENTINEL-bearer-8d41e0c97a";
const BEARER_AGAIN: &str = "SENTINEL-bearer-again-2b67f3";
const KEY: &str = "SENTINEL-apikey-5f09aa13c2";
const KEY_AGAIN: &str = "SENTINEL-apikey-again-e41d77";
const IN_PATH: &str = "SENTINEL-in-the-path-90b1c8";
/// What the person is told back about their own URL (it is theirs) but nothing may log.
const SECRETS: [&str; 4] = [BEARER, BEARER_AGAIN, KEY, KEY_AGAIN];

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

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// Everything a client saw of every request.
struct Transcript {
    client: reqwest::Client,
    base: String,
    seen: Mutex<String>,
}

impl Transcript {
    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<(&str, String)>,
    ) -> (u16, Value) {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some((content_type, body)) = body {
            req = req.header("Content-Type", content_type).body(body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let headers = format!("{:?}", resp.headers());
        let bytes = resp.bytes().await.unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        self.seen
            .lock()
            .unwrap()
            .push_str(&format!("{status}\n{headers}\n{text}\n"));
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Value,
    ) -> (u16, Value) {
        self.send(
            method,
            path,
            token,
            Some(("application/json", body.to_string())),
        )
        .await
    }
}

#[tokio::test]
async fn a_secret_typed_for_a_server_is_in_no_response_no_log_no_event_and_no_definition() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_span_events(FmtSpan::FULL)
        .with_writer(captured.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // a canary: the capture finds a secret if one is logged
    tracing::info!(canary = BEARER, "the canary");

    let servers = MemoryUserToolServers::new();
    let secrets = MemorySecretStore::new();
    let egress = EgressPolicy::new();
    let broker = Broker::builder(secrets.clone(), servers.clone(), egress.clone())
        .build()
        .unwrap();
    let user_tools = UserTools::<
        MemoryUserToolServers,
        Broker<MemorySecretStore, MemoryUserToolServers, SystemClock>,
        MemoryPendingAuthorizations,
    >::full(
        egress,
        servers.clone(),
        broker,
        MemoryPendingAuthorizations::new(),
    );
    let auth = MemoryAuth::new();
    let alice = principal("alice@example.com", &["user", "maker"]);
    auth.allow("alice", alice.clone());
    auth.allow("carol", principal("carol@example.com", &["user"]));
    let policy = Policy::new(
        BTreeMap::from([
            (Role::new("user"), RoleGrant::user()),
            (
                Role::new("maker"),
                RoleGrant {
                    permissions: [Permission::ToolsUserAdd].into_iter().collect(),
                    ..RoleGrant::none()
                },
            ),
        ]),
        None,
    )
    .unwrap();
    let directory = AgentDirectory::new(Vec::new());
    let app = Arc::new(
        App::new(
            PortSet {
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: NoModel,
                auth,
                registry: directory.fixed_registry(),
                artifacts: NoArtifacts,
                user_tools,
            },
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
    let router = orch_api::router_with_surfaces(Arc::clone(&app), ApiConfig::default(), Vec::new());
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let t = Transcript {
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        base,
        seen: Mutex::new(String::new()),
    };
    let api = "/api/user-tool-servers";
    let path_url = format!("https://mcp.example.com/{IN_PATH}/mcp");

    // add one of each kind
    let (status, bearer) = t
        .json(
            reqwest::Method::POST,
            api,
            "alice",
            json!({"name": "GitHub", "url": path_url, "auth": {"kind": "bearer"}, "secret": BEARER}),
        )
        .await;
    assert_eq!(status, 201, "{bearer}");
    let bearer_id = bearer["id"].as_str().unwrap().to_owned();
    let (status, api_key) = t
        .json(
            reqwest::Method::POST,
            api,
            "alice",
            json!({"name": "Search", "url": "https://search.example.com/mcp",
                   "auth": {"kind": "api_key", "header": "X-Api-Key"}, "secret": KEY}),
        )
        .await;
    assert_eq!(status, 201, "{api_key}");
    let key_id = api_key["id"].as_str().unwrap().to_owned();

    // the control: the broker does hold the secret, sealed in its store, so a check of the
    // definitions and the logs for it is a check that could find something
    let held = |id: &str| {
        let secrets = secrets.clone();
        let id = UserToolServerId::parse(id).unwrap();
        async move {
            let (_, material) = secrets
                .get(
                    &UserId::new("alice@example.com"),
                    &SecretServer::credential(&id),
                )
                .await
                .unwrap()
                .expect("a credential is kept");
            String::from_utf8_lossy(material.expose()).into_owned()
        }
    };
    assert!(held(&bearer_id).await.contains(BEARER));
    assert!(held(&key_id).await.contains(KEY));

    // read it, list it
    for path in [api, "/api/tool-servers"] {
        let (status, _) = t.send(reqwest::Method::GET, path, "alice", None).await;
        assert_eq!(status, 200, "{path}");
    }

    // re-key both; move the API key's server (a new URL revokes the key, a new one is given)
    let (status, _) = t
        .json(
            reqwest::Method::PATCH,
            &format!("{api}/{bearer_id}"),
            "alice",
            json!({"secret": BEARER_AGAIN}),
        )
        .await;
    assert_eq!(status, 200);
    let (status, moved) = t
        .json(
            reqwest::Method::PATCH,
            &format!("{api}/{key_id}"),
            "alice",
            json!({"url": "https://elsewhere.example.com/mcp", "secret": KEY_AGAIN}),
        )
        .await;
    assert_eq!(status, 200, "{moved}");

    // every way a request can fail with a secret in it
    let bad: Vec<(&str, reqwest::Method, String, &str, String)> = vec![
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://10.0.0.1/", "auth": {"kind": "bearer"}, "secret": BEARER}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "http://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": BEARER}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": "api_key", "header": "Authorization"}, "secret": KEY}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": "none"}, "secret": KEY}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": KEY}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": KEY}}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": "bearer"}, KEY: BEARER}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": [BEARER]}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": format!("{BEARER}\nmore")}).to_string()),
        ("alice", reqwest::Method::POST, api.into(), "application/json", format!("\"{BEARER}\"")),
        ("alice", reqwest::Method::POST, api.into(), "application/json", format!("[\"{BEARER}\"]")),
        ("alice", reqwest::Method::POST, api.into(), "application/json", format!("{{\"secret\": \"{BEARER}\"")),
        ("alice", reqwest::Method::POST, api.into(), "text/plain", format!("secret={BEARER}")),
        ("alice", reqwest::Method::PATCH, format!("{api}/{bearer_id}"), "application/json", json!({"secret": format!("{KEY} ")}).to_string()),
        ("alice", reqwest::Method::PATCH, format!("{api}/{bearer_id}"), "application/json", json!({"secret": KEY, "revision": "1"}).to_string()),
        ("alice", reqwest::Method::PATCH, format!("{api}/{bearer_id}"), "application/json", json!({"secret": KEY, "revision": KEY}).to_string()),
        ("alice", reqwest::Method::PATCH, format!("{api}/u-0000000000"), "application/json", json!({"secret": KEY}).to_string()),
        // a role without the permission
        ("carol", reqwest::Method::POST, api.into(), "application/json",
         json!({"name": "x", "url": "https://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": BEARER}).to_string()),
        ("carol", reqwest::Method::PATCH, format!("{api}/{bearer_id}"), "application/json", json!({"secret": KEY}).to_string()),
    ];
    for (who, method, path, content_type, body) in bad {
        let (status, _) = t
            .send(method.clone(), &path, who, Some((content_type, body)))
            .await;
        assert!(status >= 400, "{method} {path} as {who}: {status}");
    }

    // a broker that is down: the add, the re-key, the move and the delete all fail with a secret in
    // flight
    secrets.set_down(true);
    for (method, path, body) in [
        (
            reqwest::Method::POST,
            api.to_owned(),
            json!({"name": "y", "url": "https://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": BEARER}),
        ),
        (
            reqwest::Method::PATCH,
            format!("{api}/{bearer_id}"),
            json!({"secret": BEARER}),
        ),
        (
            reqwest::Method::PATCH,
            format!("{api}/{bearer_id}"),
            json!({"url": "https://moved.example.com/mcp", "secret": BEARER}),
        ),
    ] {
        let (status, _) = t.json(method.clone(), &path, "alice", body).await;
        assert_eq!(status, 503, "{method} {path}");
    }
    let (status, _) = t
        .send(
            reqwest::Method::DELETE,
            &format!("{api}/{bearer_id}"),
            "alice",
            None,
        )
        .await;
    assert_eq!(status, 503);
    secrets.set_down(false);

    // the limit, with a secret in the body
    for n in 0..8 {
        let (status, _) = t
            .json(
                reqwest::Method::POST,
                api,
                "alice",
                json!({"name": format!("filler{n}"), "url": "https://mcp.example.com/", "auth": {"kind": "none"}}),
            )
            .await;
        assert!(status == 201 || status == 409, "{status}");
    }
    let (status, _) = t
        .json(
            reqwest::Method::POST,
            api,
            "alice",
            json!({"name": "ninth", "url": "https://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": BEARER}),
        )
        .await;
    assert_eq!(status, 409);

    // and the delete of both
    for id in [&bearer_id, &key_id] {
        let (status, _) = t
            .send(
                reqwest::Method::DELETE,
                &format!("{api}/{id}"),
                "alice",
                None,
            )
            .await;
        assert_eq!(status, 204);
    }
    drop(_guard);
    server.abort();

    // ---- the captures ---------------------------------------------------------------------
    let seen = t.seen.lock().unwrap().clone();
    let log = captured.text();
    let at = servers
        .list(&UserId::new("alice@example.com"))
        .await
        .unwrap();
    let definitions = format!("{at:?}");
    let threads = app
        .list_threads(
            &alice,
            ThreadListing {
                before: None,
                limit: 100,
                include_edits: true,
                order: orch_ports::ListOrder::Recent,
                archived: orch_ports::ArchivedFilter::Include,
            },
        )
        .await
        .unwrap();

    // the control: the capture finds a secret when one is there
    assert!(log.contains(BEARER), "the canary was not captured:\n{log}");
    let log_without_canary = log.replace("canary=\"SENTINEL-bearer-8d41e0c97a\"", "canary=…");
    assert!(
        log_without_canary.len() > 2_000 && log_without_canary.contains("request"),
        "the spans and events were captured"
    );

    for secret in SECRETS {
        assert!(!seen.contains(secret), "{secret} is in a response:\n{seen}");
        assert!(
            !log_without_canary.contains(secret),
            "{secret} is in the log:\n{log_without_canary}"
        );
        assert!(!definitions.contains(secret), "{secret} is in a definition");
    }
    // the path of a URL is the person's, shown to them and nobody else
    assert!(seen.contains(IN_PATH), "the person is shown their own URL");
    assert!(
        !log_without_canary.contains(IN_PATH),
        "a URL's path is in the log:\n{log_without_canary}"
    );
    assert!(
        !definitions.contains(IN_PATH),
        "a definition prints no path"
    );
    // no event: nothing of this is a thread, and nothing is waiting to be sent
    assert!(threads.is_empty(), "no thread exists");
    let (_, outbox) = app.outbox_stats().await.unwrap();
    assert_eq!((outbox.due, outbox.waiting, outbox.leased), (0, 0, 0));
}

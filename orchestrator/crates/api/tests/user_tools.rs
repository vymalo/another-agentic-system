//! People's own MCP servers over real HTTP (ADR 0046, slice 4b): the four routes and the listing
//! `GET /api/tool-servers`, for the owner, for another person (a 404 that is the same as for a server
//! that never existed), for a role without `tools.user.add` (a 403 for changes; reading and deleting
//! still work), with the feature off, in a deployment that keeps no credentials, at the limit, with a
//! stale revision, with a broker that is down and with a definition that does not read back.
//!
//! Every response is validated against the schema `docs/api/chat-api.yaml` documents for that
//! operation and status. The broker is the real one over the in-memory stores; nothing needs an
//! authorization server because `none`, `bearer` and `api_key` have no OAuth.
//!
//! The secret never being in a response, a log line or an event is `user_tools_secret.rs`, a test of
//! its own process (it captures the logs).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, App, AppConfig, Permission, Policy, RoleGrant, ToolServerInfo};
use orch_core::UserId;
use orch_credential_broker::Broker;
use orch_ports::memory::{
    MemoryAuth, MemorySecretStore, MemoryStore, MemoryUserToolServers, MemoryWakeup, ScriptedAgent,
    SeqIds,
};
use orch_ports::{
    EgressPolicy, NoArtifacts, NoBroker, NoModel, NoUserToolServers, PortSet, Principal, Role,
    ServerRevision, StoredServer, SystemClock, UserAuth, UserToolPorts, UserToolServer,
    UserToolServerId, UserToolServerState, UserToolServerStore, UserToolServerStoreError,
    UserTools,
};
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";
const URL: &str = "https://mcp.example.com/mcp";
const OTHER_URL: &str = "https://other.example.com/mcp";
/// The secrets the tests plant. No response, however it fails, may carry one.
const TOKEN: &str = "SENTINEL-token-7f3a9c";
const KEY: &str = "SENTINEL-key-51be08";

// ---- the contract ----------------------------------------------------------------------------

struct Contract(Value);

impl Contract {
    fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        Contract(serde_norway::from_str(&text).unwrap())
    }

    /// The schema the contract gives the response of `method path` with `status`, `Ok(None)` for a
    /// response with no body, `Err` for a status the contract does not document.
    fn response(&self, path: &str, method: &str, status: u16) -> Result<Option<Value>, String> {
        let operation = &self.0["paths"][path][method];
        let mut response = operation["responses"]
            .get(status.to_string())
            .cloned()
            .ok_or_else(|| format!("the contract documents no {status} for {method} {path}"))?;
        if let Some(reference) = response.get("$ref").and_then(Value::as_str) {
            let name = reference.rsplit('/').next().unwrap();
            response = self.0["components"]["responses"][name].clone();
        }
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            return Ok(None);
        };
        let (_, media) = content.iter().next().unwrap();
        Ok(Some(media["schema"].clone()))
    }

    fn validate(&self, schema: &Value, instance: &Value) {
        let mut root = schema.clone();
        let object = root.as_object_mut().unwrap();
        object.insert(
            "$schema".to_owned(),
            json!("https://json-schema.org/draft/2020-12/schema"),
        );
        object.insert("components".to_owned(), self.0["components"].clone());
        let validator = jsonschema::draft202012::options()
            .should_validate_formats(true)
            .build(&root)
            .unwrap();
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| format!("{e} at {}", e.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{}\n{instance}", errors.join("\n"));
    }

    /// A request body against the schema of the operation.
    fn request(&self, path: &str, method: &str, instance: &Value) {
        let schema =
            self.0["paths"][path][method]["requestBody"]["content"]["application/json"]["schema"]
                .clone();
        self.validate(&schema, instance);
    }
}

// ---- the doubles ------------------------------------------------------------------------------

/// A place in a write sequence where a test holds a request, until it lets it go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// After `insert` has written, before the next step of the add.
    AfterInsert,
    /// After the next `replace` has written.
    AfterReplace,
    /// Before `delete` writes (the revoke has been done).
    BeforeDelete,
}

/// One hold: the request says it got there, the test lets it go on, and a flag says whether the
/// future that waits was dropped there (the handler was cancelled).
#[derive(Clone, Default)]
struct Pause {
    reached: Arc<Notify>,
    release: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

/// Sets the flag of a [`Pause`] when it is dropped before it is disarmed.
struct Waiting<'a>(&'a Pause, bool);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if self.1 {
            self.0.dropped.store(true, Ordering::SeqCst);
        }
    }
}

/// The definitions' store with the switches a test throws: a row that does not read back, a store
/// that refuses everything (the shape of the store of a deployment where the feature is off), a
/// place where a request is held, and a panic.
#[derive(Clone, Default)]
struct Faulty {
    inner: MemoryUserToolServers,
    corrupt: Arc<AtomicBool>,
    refuse: Arc<AtomicBool>,
    holds: Arc<Mutex<Vec<(Hold, Pause)>>>,
    panic_on_insert: Arc<AtomicBool>,
}

impl From<NoUserToolServers> for Faulty {
    fn from(_: NoUserToolServers) -> Self {
        let faulty = Faulty::default();
        faulty.refuse.store(true, Ordering::SeqCst);
        faulty
    }
}

impl Faulty {
    fn up(&self) -> Result<(), UserToolServerStoreError> {
        if self.refuse.load(Ordering::SeqCst) {
            return Err(UserToolServerStoreError::unavailable("off"));
        }
        Ok(())
    }

    fn reads_back(&self) -> Result<(), UserToolServerStoreError> {
        if self.corrupt.load(Ordering::SeqCst) {
            return Err(UserToolServerStoreError::Corrupt(
                "a stored definition does not read back".to_owned(),
            ));
        }
        Ok(())
    }

    /// Holds the next request that gets to `at`, once.
    fn hold(&self, at: Hold) -> Pause {
        let pause = Pause::default();
        self.holds.lock().unwrap().push((at, pause.clone()));
        pause
    }

    /// The hold at `at`, if one is set: says the request is there, and waits to be let go on.
    async fn held(&self, at: Hold) {
        let pause = {
            let mut holds = self.holds.lock().unwrap();
            holds
                .iter()
                .position(|(hold, _)| *hold == at)
                .map(|n| holds.remove(n).1)
        };
        if let Some(pause) = pause {
            let mut waiting = Waiting(&pause, true);
            pause.reached.notify_one();
            pause.release.notified().await;
            waiting.1 = false;
        }
    }
}

impl UserToolServerStore for Faulty {
    async fn insert(
        &self,
        owner: &UserId,
        server: &UserToolServer,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
        assert!(
            !self.panic_on_insert.swap(false, Ordering::SeqCst),
            "a store that panics"
        );
        let revision = self.inner.insert(owner, server).await;
        self.held(Hold::AfterInsert).await;
        revision
    }

    async fn get(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<Option<StoredServer>, UserToolServerStoreError> {
        self.up()?;
        self.reads_back()?;
        self.inner.get(owner, id).await
    }

    async fn list(&self, owner: &UserId) -> Result<Vec<StoredServer>, UserToolServerStoreError> {
        self.up()?;
        self.reads_back()?;
        self.inner.list(owner).await
    }

    async fn replace(
        &self,
        owner: &UserId,
        server: &UserToolServer,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
        let revision = self.inner.replace(owner, server, expected).await;
        self.held(Hold::AfterReplace).await;
        revision
    }

    async fn set_state(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
        state: UserToolServerState,
        expected: ServerRevision,
    ) -> Result<ServerRevision, UserToolServerStoreError> {
        self.up()?;
        self.inner.set_state(owner, id, state, expected).await
    }

    async fn delete(
        &self,
        owner: &UserId,
        id: &UserToolServerId,
    ) -> Result<(), UserToolServerStoreError> {
        self.up()?;
        self.held(Hold::BeforeDelete).await;
        self.inner.delete(owner, id).await
    }
}

// ---- the harness ------------------------------------------------------------------------------

struct Resp {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Vec<u8>,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_owned)
    }

    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

struct Harness {
    base: String,
    client: reqwest::Client,
    contract: Contract,
    servers: Faulty,
    secrets: MemorySecretStore,
    server: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// `user` is the built-in role; `maker` holds `tools.user.add` and nothing else.
fn policy() -> Policy {
    Policy::new(
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
        // a person whose roles are all unknown gets nothing at all
        None,
    )
    .unwrap()
}

impl Harness {
    /// Alice and Bob may add servers (`alice`, `bob`); `carol` may not; `alice-demoted` is Alice
    /// after her role was taken away; `ghost` has a role nobody defined and so no access at all.
    async fn full() -> Self {
        Self::full_with(ApiConfig::default()).await
    }

    async fn full_with(config: ApiConfig) -> Self {
        let servers = Faulty::default();
        let secrets = MemorySecretStore::new();
        let egress = EgressPolicy::new();
        let broker = Broker::builder(secrets.clone(), servers.clone(), egress.clone())
            .build()
            .unwrap();
        let bundle = UserTools::<Faulty, Broker<MemorySecretStore, Faulty, SystemClock>>::full(
            egress,
            servers.clone(),
            broker,
        );
        Self::start(bundle, servers, secrets, config).await
    }

    async fn without_credentials() -> Self {
        let servers = Faulty::default();
        let bundle = UserTools::<Faulty, NoBroker>::without_credentials(
            EgressPolicy::new(),
            servers.clone(),
        );
        Self::start(
            bundle,
            servers,
            MemorySecretStore::new(),
            ApiConfig::default(),
        )
        .await
    }

    async fn off() -> Self {
        let bundle = UserTools::<Faulty, NoBroker>::off();
        Self::start(
            bundle,
            Faulty::default(),
            MemorySecretStore::new(),
            ApiConfig::default(),
        )
        .await
    }

    async fn start<T: UserToolPorts>(
        user_tools: T,
        servers: Faulty,
        secrets: MemorySecretStore,
        config: ApiConfig,
    ) -> Self {
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user", "maker"]));
        auth.allow("bob", principal(BOB, &["user", "maker"]));
        auth.allow("carol", principal("carol@example.com", &["user"]));
        auth.allow("alice-demoted", principal(ALICE, &["user"]));
        auth.allow("ghost", principal("ghost@example.com", &["wizard"]));
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
                    policy: policy(),
                    tool_servers: vec![ToolServerInfo::new("docs", "Documentation")],
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = orch_api::router_with_surfaces(app, config, Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Harness {
            base: format!("http://{addr}"),
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            contract: Contract::load(),
            servers,
            secrets,
            server,
        }
    }

    async fn raw(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<(&str, String)>,
    ) -> Resp {
        let mut req = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some((content_type, body)) = body {
            req = req.header("Content-Type", content_type).body(body);
        }
        let resp = req.send().await.unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    /// A request whose body is sent with **no** `Content-Type` header at all.
    async fn undeclared(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: String,
    ) -> Resp {
        let resp = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token)
            .body(body)
            .send()
            .await
            .unwrap();
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    async fn json(
        &self,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) -> Resp {
        let body = body.map(|b| ("application/json", b.to_string()));
        self.raw(method, path, Some(token), body).await
    }

    /// Validates `resp` against what the contract documents for `method` and `template`, and returns
    /// it. A response with a body is checked against its schema; a 204 has none.
    fn checked(&self, resp: Resp, method: &str, template: &str) -> Resp {
        match self
            .contract
            .response(template, method, resp.status)
            .unwrap_or_else(|e| panic!("{e}: {}", resp.text()))
        {
            Some(schema) => self.contract.validate(&schema, &resp.json()),
            None => assert!(resp.body.is_empty(), "{}", resp.text()),
        }
        resp
    }

    async fn post(&self, token: &str, body: Value) -> Resp {
        self.contract
            .request("/api/user-tool-servers", "post", &body);
        let resp = self
            .json(
                reqwest::Method::POST,
                "/api/user-tool-servers",
                token,
                Some(&body),
            )
            .await;
        self.checked(resp, "post", "/api/user-tool-servers")
    }

    /// A request the contract does not describe (a body it forbids): not validated on the way out.
    async fn post_unchecked(&self, token: &str, body: &Value) -> Resp {
        let resp = self
            .json(
                reqwest::Method::POST,
                "/api/user-tool-servers",
                token,
                Some(body),
            )
            .await;
        self.checked(resp, "post", "/api/user-tool-servers")
    }

    async fn patch(&self, token: &str, id: &str, body: Value) -> Resp {
        let resp = self
            .json(
                reqwest::Method::PATCH,
                &format!("/api/user-tool-servers/{id}"),
                token,
                Some(&body),
            )
            .await;
        self.checked(resp, "patch", "/api/user-tool-servers/{serverId}")
    }

    async fn delete(&self, token: &str, id: &str) -> Resp {
        let resp = self
            .json(
                reqwest::Method::DELETE,
                &format!("/api/user-tool-servers/{id}"),
                token,
                None,
            )
            .await;
        self.checked(resp, "delete", "/api/user-tool-servers/{serverId}")
    }

    async fn list(&self, token: &str) -> Resp {
        let resp = self
            .json(reqwest::Method::GET, "/api/user-tool-servers", token, None)
            .await;
        self.checked(resp, "get", "/api/user-tool-servers")
    }

    async fn picker(&self, token: &str) -> Resp {
        let resp = self
            .json(reqwest::Method::GET, "/api/tool-servers", token, None)
            .await;
        self.checked(resp, "get", "/api/tool-servers")
    }

    /// Adds a server and returns it.
    async fn add(&self, token: &str, body: Value) -> Value {
        let resp = self.post(token, body).await;
        assert_eq!(resp.status, 201, "{}", resp.text());
        resp.json()
    }
}

fn none(name: &str) -> Value {
    json!({"name": name, "url": URL, "auth": {"kind": "none"}})
}

fn bearer(name: &str) -> Value {
    json!({"name": name, "url": URL, "auth": {"kind": "bearer"}, "secret": TOKEN})
}

fn api_key(name: &str) -> Value {
    json!({"name": name, "url": URL, "auth": {"kind": "api_key", "header": "X-Api-Key"}, "secret": KEY})
}

fn id_of(server: &Value) -> String {
    server["id"].as_str().unwrap().to_owned()
}

// ---- the tests ---------------------------------------------------------------------------------

#[tokio::test]
async fn with_the_feature_off_every_route_is_a_404_and_the_listing_is_what_it_was() {
    let h = Harness::off().await;
    // the body is not even read: a garbage body is the same 404
    let r = h
        .raw(
            reqwest::Method::POST,
            "/api/user-tool-servers",
            Some("alice"),
            Some(("application/json", "{not json".to_owned())),
        )
        .await;
    let r = h.checked(r, "post", "/api/user-tool-servers");
    assert_eq!(r.status, 404, "{}", r.text());
    let r = h.list("alice").await;
    assert_eq!(r.status, 404);
    let r = h.patch("alice", "u-abcdefghij", json!({"name": "x"})).await;
    assert_eq!(r.status, 404);
    let r = h.delete("alice", "u-abcdefghij").await;
    assert_eq!(r.status, 404);
    // nobody asks the refusing store, so the picker is the deployment's list, byte for byte
    let r = h.picker("alice").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!([{"id": "docs", "name": "Documentation"}]));
    assert_eq!(r.header("cache-control"), Some("no-store"));
}

#[tokio::test]
async fn every_route_needs_a_token_and_a_role_that_grants_something() {
    let h = Harness::full().await;
    for (method, path) in [
        (reqwest::Method::GET, "/api/user-tool-servers"),
        (reqwest::Method::POST, "/api/user-tool-servers"),
        (
            reqwest::Method::PATCH,
            "/api/user-tool-servers/u-abcdefghij",
        ),
        (
            reqwest::Method::DELETE,
            "/api/user-tool-servers/u-abcdefghij",
        ),
    ] {
        let r = h.raw(method.clone(), path, None, None).await;
        assert_eq!(r.status, 401, "{method} {path}");
        let r = h
            .json(method.clone(), path, "ghost", Some(&json!({"name": "x"})))
            .await;
        assert_eq!(r.status, 403, "{method} {path}");
        assert_eq!(r.code().as_deref(), Some("no_access"));
    }
}

#[tokio::test]
async fn a_none_server_goes_through_its_whole_life() {
    let h = Harness::full().await;
    let added = h.add("alice", none("  Docs  ")).await;
    let id = id_of(&added);
    assert!(id.starts_with("u-") && id.len() == 12, "{id}");
    assert_eq!(
        added,
        json!({
            "id": id, "name": "Docs", "own": true, "status": "ready", "url": URL,
            "auth": {"kind": "none"}, "state": "connected", "hasCredential": false,
            "revision": added["revision"],
        })
    );
    // an answer that carries a URL is never cached
    let r = h.post("alice", none("again")).await;
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let again = r.json();

    // the listing, and the picker beside the deployment's: the deployment's first, then the
    // person's by name
    let r = h.list("alice").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let list = r.json();
    assert_eq!(list["limit"], 8);
    assert_eq!(
        list["kinds"],
        json!(["none", "bearer", "api_key", "oauth2"])
    );
    assert_eq!(list["servers"], json!([again, added]));
    assert!(list.get("error").is_none());
    let r = h.picker("alice").await;
    assert_eq!(
        r.json(),
        json!([
            {"id": "docs", "name": "Documentation"},
            {"id": again["id"], "name": "again", "own": true, "status": "ready"},
            {"id": added["id"], "name": "Docs", "own": true, "status": "ready"},
        ])
    );

    // a new name
    let first_revision = added["revision"].as_str().unwrap().to_owned();
    let r = h.patch("alice", &id, json!({"name": "Docs two"})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let renamed = r.json();
    assert_eq!(renamed["name"], "Docs two");
    assert_eq!(renamed["state"], "connected");
    assert_ne!(renamed["revision"], first_revision.as_str());

    // delete, and again
    let r = h.delete("alice", &id).await;
    assert_eq!(r.status, 204);
    let r = h.delete("alice", &id).await;
    assert_eq!(r.status, 404);
    assert_eq!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn a_bearer_server_is_keyed_rekeyed_moved_and_deleted() {
    let h = Harness::full().await;
    let added = h.add("alice", bearer("GitHub")).await;
    let id = id_of(&added);
    assert_eq!(added["state"], "connected");
    assert_eq!(added["hasCredential"], true);
    assert_eq!(added["auth"], json!({"kind": "bearer"}));
    assert_eq!(h.secrets.len(), 1, "the broker holds the secret");
    // write-only: it is not in the answer, in the listing or in the picker
    for body in [
        added.to_string(),
        h.list("alice").await.text(),
        h.picker("alice").await.text(),
    ] {
        assert!(!body.contains(TOKEN), "{body}");
    }

    // re-keying: the same server, `connected`, one credential
    let r = h
        .patch("alice", &id, json!({"secret": "SENTINEL-token-second"}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["state"], "connected");
    assert_eq!(h.secrets.len(), 1);

    // a new URL: the credential was issued for the old one, so it is revoked, and the server waits
    let r = h.patch("alice", &id, json!({"url": OTHER_URL})).await;
    assert_eq!(r.status, 200, "{}", r.text());
    let moved = r.json();
    assert_eq!(moved["url"], OTHER_URL);
    assert_eq!(moved["state"], "added");
    assert_eq!(moved["hasCredential"], false);
    assert_eq!(moved["status"], "needs_auth");
    assert_eq!(moved["reason"], "not_connected");
    assert!(h.secrets.is_empty(), "the old credential is gone");
    let r = h.picker("alice").await;
    assert_eq!(r.json()[1]["status"], "needs_auth");
    assert_eq!(r.json()[1]["reason"], "not_connected");

    // given a key again it is connected
    let r = h.patch("alice", &id, json!({"secret": TOKEN})).await;
    assert_eq!(r.json()["state"], "connected");
    assert_eq!(r.json()["status"], "ready");
    assert_eq!(h.secrets.len(), 1);

    let r = h.delete("alice", &id).await;
    assert_eq!(r.status, 204);
    assert!(h.secrets.is_empty(), "the credential went with the server");
}

#[tokio::test]
async fn an_api_key_server_names_its_header_and_a_new_header_keeps_the_key() {
    let h = Harness::full().await;
    let added = h.add("alice", api_key("Search")).await;
    let id = id_of(&added);
    assert_eq!(
        added["auth"],
        json!({"kind": "api_key", "header": "X-Api-Key"})
    );
    let r = h
        .patch(
            "alice",
            &id,
            json!({"auth": {"kind": "api_key", "header": "X-Other"}}),
        )
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["auth"]["header"], "X-Other");
    assert_eq!(r.json()["state"], "connected");
    assert_eq!(r.json()["hasCredential"], true);
    assert_eq!(h.secrets.len(), 1);
}

#[tokio::test]
async fn an_oauth2_server_is_stored_and_says_it_needs_the_person_to_authorize() {
    let h = Harness::full().await;
    let added = h
        .add(
            "alice",
            json!({"name": "Notion", "url": URL,
                   "auth": {"kind": "oauth2", "scopes": ["read", "write"], "clientId": "my-client"}}),
        )
        .await;
    assert_eq!(added["state"], "added");
    assert_eq!(added["status"], "needs_auth");
    assert_eq!(added["reason"], "not_connected");
    assert_eq!(added["hasCredential"], false);
    assert_eq!(
        added["auth"],
        json!({"kind": "oauth2", "scopes": ["read", "write"], "clientId": "my-client"})
    );
    // an oauth2 without scopes says so with an empty list
    let bare = h
        .add(
            "alice",
            json!({"name": "Bare", "url": URL, "auth": {"kind": "oauth2"}}),
        )
        .await;
    assert_eq!(bare["auth"], json!({"kind": "oauth2", "scopes": []}));
    // a secret is for a bearer or an api_key
    let r = h
        .post_unchecked(
            "alice",
            &json!({"name": "x", "url": URL, "auth": {"kind": "oauth2"}, "secret": TOKEN}),
        )
        .await;
    assert_eq!(r.status, 400, "{}", r.text());
    assert!(!r.text().contains(TOKEN));
    assert!(h.secrets.is_empty());
}

#[tokio::test]
async fn adding_and_editing_need_the_permission_and_reading_and_deleting_do_not() {
    let h = Harness::full().await;
    // carol holds `user` and not `maker`
    let r = h.post("carol", none("x")).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("forbidden"));
    assert!(
        h.list("carol").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(h.picker("carol").await.status, 200);

    // Alice added a server; then her role was taken away
    let added = h.add("alice", bearer("GitHub")).await;
    let id = id_of(&added);
    let r = h.list("alice-demoted").await;
    assert_eq!(r.status, 200);
    let mine = r.json()["servers"][0].clone();
    assert_eq!(mine["status"], "unavailable");
    assert_eq!(mine["reason"], "not_permitted");
    assert_eq!(mine["state"], "connected", "the state is the definition's");
    let r = h.picker("alice-demoted").await;
    assert_eq!(r.json()[1]["status"], "unavailable");
    assert_eq!(r.json()[1]["reason"], "not_permitted");
    let r = h.patch("alice-demoted", &id, json!({"name": "y"})).await;
    assert_eq!(r.status, 403, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("forbidden"));
    let r = h.post("alice-demoted", none("z")).await;
    assert_eq!(r.status, 403);
    // but she can remove what she added, and its credential goes with it
    let r = h.delete("alice-demoted", &id).await;
    assert_eq!(r.status, 204);
    assert!(h.secrets.is_empty());
}

#[tokio::test]
async fn another_persons_server_is_a_404_that_is_the_answer_for_one_that_never_existed() {
    let h = Harness::full().await;
    let mine = h.add("alice", bearer("mine")).await;
    let id = id_of(&mine);
    assert!(
        h.list("bob").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        h.picker("bob").await.json(),
        json!([{"id": "docs", "name": "Documentation"}])
    );

    let mut seen = Vec::new();
    for target in [id.as_str(), "u-0000000000", "nonsense"] {
        let patch = h.patch("bob", target, json!({"name": "stolen"})).await;
        let delete = h.delete("bob", target).await;
        assert_eq!((patch.status, delete.status), (404, 404), "{target}");
        seen.push((patch.text(), delete.text()));
    }
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "one answer for all: {seen:?}"
    );
    // nothing of Alice's changed, and her credential is still there
    assert_eq!(h.list("alice").await.json()["servers"][0]["name"], "mine");
    assert_eq!(h.secrets.len(), 1);
}

#[tokio::test]
async fn a_body_that_cannot_be_used_is_a_400_that_quotes_nothing_of_it() {
    let h = Harness::full().await;
    let planted = "SENTINEL-planted-b1c2";
    let obj = |v: Value| v;
    let cases: Vec<(&str, Value)> = vec![
        (
            "a name that is not a string",
            obj(json!({"name": 5, "url": URL, "auth": {"kind": "none"}})),
        ),
        (
            "an empty name",
            obj(json!({"name": "  ", "url": URL, "auth": {"kind": "none"}})),
        ),
        (
            "a long name",
            obj(json!({"name": "n".repeat(81), "url": URL, "auth": {"kind": "none"}})),
        ),
        (
            "no name",
            obj(json!({"url": URL, "auth": {"kind": "none"}})),
        ),
        (
            "no url",
            obj(json!({"name": "x", "auth": {"kind": "none"}})),
        ),
        (
            "a url that is a number",
            obj(json!({"name": "x", "url": 5, "auth": {"kind": "none"}})),
        ),
        ("no auth", obj(json!({"name": "x", "url": URL}))),
        (
            "an auth that is a string",
            obj(json!({"name": "x", "url": URL, "auth": planted})),
        ),
        (
            "an auth that is an array",
            obj(json!({"name": "x", "url": URL, "auth": [planted]})),
        ),
        (
            "an unknown kind",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": planted}})),
        ),
        ("no kind", obj(json!({"name": "x", "url": URL, "auth": {}}))),
        (
            "a kind that is not a string",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": [planted]}})),
        ),
        (
            "an unknown member of the body",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "none"}, planted: 1})),
        ),
        (
            "an unknown member of auth",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "none", planted: 1}})),
        ),
        (
            "a header on a bearer",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "bearer", "header": "X-A"}, "secret": planted}),
            ),
        ),
        (
            "an api_key with no header",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "api_key"}, "secret": planted})),
        ),
        (
            "an api_key with a reserved header",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "api_key", "header": "Authorization"}, "secret": planted}),
            ),
        ),
        (
            "an api_key with a header that is not a name",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "api_key", "header": "X A"}, "secret": planted}),
            ),
        ),
        (
            "scopes that are not an array",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "oauth2", "scopes": planted}})),
        ),
        (
            "scopes with a number",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "oauth2", "scopes": [planted, 5]}}),
            ),
        ),
        (
            "a bad scope",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "oauth2", "scopes": ["a b"]}})),
        ),
        (
            "a bearer with no secret",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}})),
        ),
        (
            "a secret that is a number",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}, "secret": 12345})),
        ),
        (
            "a secret that is an array",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}, "secret": [planted]})),
        ),
        (
            "a secret that is an object",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}, "secret": {planted: planted}}),
            ),
        ),
        (
            "a secret with a newline",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}, "secret": format!("{planted}\nmore")}),
            ),
        ),
        (
            "a secret with a space at its end",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}, "secret": format!("{planted} ")}),
            ),
        ),
        (
            "a secret that is too long",
            obj(
                json!({"name": "x", "url": URL, "auth": {"kind": "bearer"}, "secret": planted.repeat(400)}),
            ),
        ),
        (
            "a secret for none",
            obj(json!({"name": "x", "url": URL, "auth": {"kind": "none"}, "secret": planted})),
        ),
        (
            "a private address",
            obj(
                json!({"name": "x", "url": "https://10.0.0.1/mcp", "auth": {"kind": "bearer"}, "secret": planted}),
            ),
        ),
        (
            "plain http",
            obj(
                json!({"name": "x", "url": "http://mcp.example.com/", "auth": {"kind": "bearer"}, "secret": planted}),
            ),
        ),
        (
            "a url with credentials",
            obj(
                json!({"name": "x", "url": "https://u:p@mcp.example.com/", "auth": {"kind": "none"}}),
            ),
        ),
        (
            "a url that is not one",
            obj(json!({"name": "x", "url": planted, "auth": {"kind": "none"}})),
        ),
    ];
    for (label, body) in cases {
        let r = h.post_unchecked("alice", &body).await;
        assert_eq!(r.status, 400, "{label}: {}", r.text());
        assert_eq!(r.header("content-type"), Some("application/problem+json"));
        let text = r.text();
        for leaked in [planted, "mcp.example.com", "10.0.0.1"] {
            assert!(!text.contains(leaked), "{label}: {text}");
        }
    }
    // a body that is not an object, or not JSON, or not declared as JSON
    for (label, content_type, body) in [
        ("a string", "application/json", format!("\"{planted}\"")),
        ("an array", "application/json", format!("[\"{planted}\"]")),
        ("a number", "application/json", "5".to_owned()),
        (
            "broken JSON",
            "application/json",
            format!("{{\"secret\": \"{planted}\""),
        ),
        ("empty", "application/json", String::new()),
        (
            "text",
            "text/plain",
            format!("{{\"secret\": \"{planted}\"}}"),
        ),
        (
            "a form",
            "application/x-www-form-urlencoded",
            format!("secret={planted}"),
        ),
    ] {
        let r = h
            .raw(
                reqwest::Method::POST,
                "/api/user-tool-servers",
                Some("alice"),
                Some((content_type, body)),
            )
            .await;
        let r = h.checked(r, "post", "/api/user-tool-servers");
        assert_eq!(r.status, 400, "{label}: {}", r.text());
        assert!(!r.text().contains(planted), "{label}: {}", r.text());
        // the same for an edit
        let r = h
            .raw(
                reqwest::Method::PATCH,
                "/api/user-tool-servers/u-abcdefghij",
                Some("alice"),
                Some((content_type, format!("[\"{planted}\"]"))),
            )
            .await;
        let r = h.checked(r, "patch", "/api/user-tool-servers/{serverId}");
        assert_eq!(r.status, 400, "{label}: {}", r.text());
        assert!(!r.text().contains(planted));
    }
    // a body larger than the routes read
    let r = h
        .raw(
            reqwest::Method::POST,
            "/api/user-tool-servers",
            Some("alice"),
            Some((
                "application/json",
                format!("{{\"name\": \"{}\"}}", "n".repeat(40 * 1024)),
            )),
        )
        .await;
    assert_eq!(h.checked(r, "post", "/api/user-tool-servers").status, 400);
    // nothing was written, nothing was kept
    assert!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(h.secrets.is_empty());
}

/// The sentence of every body that is not declared as JSON.
const NOT_DECLARED_JSON: &str = "the body must be JSON (Content-Type: application/json)";

#[tokio::test]
async fn a_valid_body_that_is_not_declared_as_json_is_the_fixed_400_and_writes_nothing() {
    // the content type is the CSRF guard of these routes: a cross-site form or a `fetch` in
    // `no-cors` mode can send `text/plain` or nothing, and cannot send `application/json`. The bodies
    // here would be accepted if they were read, so only the check of the type can refuse them
    let h = Harness::full().await;
    let id = id_of(&h.add("alice", none("a")).await);
    let add = none("added").to_string();
    let rename = json!({"name": "renamed"}).to_string();
    let edit_path = format!("/api/user-tool-servers/{id}");
    for (label, content_type) in [
        ("text/plain", Some("text/plain")),
        (
            "text/plain with a charset",
            Some("text/plain; charset=utf-8"),
        ),
        ("text/json", Some("text/json")),
        ("a form", Some("application/x-www-form-urlencoded")),
        ("a multipart form", Some("multipart/form-data; boundary=x")),
        ("application/jsonp", Some("application/jsonp")),
        ("not a media type", Some("json")),
        ("nothing declared", None),
    ] {
        for (method, path, template, body) in [
            (
                reqwest::Method::POST,
                "/api/user-tool-servers",
                "/api/user-tool-servers",
                &add,
            ),
            (
                reqwest::Method::PATCH,
                edit_path.as_str(),
                "/api/user-tool-servers/{serverId}",
                &rename,
            ),
        ] {
            let r = match content_type {
                Some(content_type) => {
                    h.raw(
                        method.clone(),
                        path,
                        Some("alice"),
                        Some((content_type, body.clone())),
                    )
                    .await
                }
                None => {
                    h.undeclared(method.clone(), path, "alice", body.clone())
                        .await
                }
            };
            let r = h.checked(r, &method.as_str().to_lowercase(), template);
            assert_eq!(r.status, 400, "{label} {method}: {}", r.text());
            assert_eq!(r.json()["detail"], NOT_DECLARED_JSON, "{label} {method}");
        }
    }
    // nothing was added and nothing was renamed
    let listed = h.list("alice").await.json()["servers"].clone();
    assert_eq!(listed.as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed[0]["name"], "a");

    // what is declared as JSON is read, whatever its case and parameters
    for (n, content_type) in [
        "application/json; charset=utf-8",
        "Application/JSON",
        "application/merge-patch+json",
    ]
    .into_iter()
    .enumerate()
    {
        let r = h
            .raw(
                reqwest::Method::POST,
                "/api/user-tool-servers",
                Some("alice"),
                Some((content_type, none(&format!("j{n}")).to_string())),
            )
            .await;
        let r = h.checked(r, "post", "/api/user-tool-servers");
        assert_eq!(r.status, 201, "{content_type}: {}", r.text());
    }
}

#[tokio::test]
async fn a_body_over_the_cap_is_a_400_even_when_it_is_valid_json_and_writes_nothing() {
    // a valid add padded with whitespace, which JSON allows: only the size can refuse it
    let h = Harness::full().await;
    let base = none("padded").to_string();
    let padded = |total: usize| format!("{base}{}", " ".repeat(total - base.len()));
    let send = |body: String| {
        h.raw(
            reqwest::Method::POST,
            "/api/user-tool-servers",
            Some("alice"),
            Some(("application/json", body)),
        )
    };
    let cap = 32 * 1024;
    // 33 KiB, 40 KiB (the over-the-cap bodies of the routes) and one byte over the cap: the routes'
    // own sentence, naming the cap
    for total in [cap + 1, 33 * 1024, 40 * 1024, 1024 * 1024] {
        let r = send(padded(total)).await;
        let r = h.checked(r, "post", "/api/user-tool-servers");
        assert_eq!(r.status, 400, "{total} bytes: {}", r.text());
        assert_eq!(
            r.json()["detail"],
            format!("the body is larger than {cap} bytes"),
            "{total} bytes"
        );
    }
    // 2 MiB and 3 MiB are past the limit of the whole API (1 MiB): refused before the routes read them
    for total in [2 * 1024 * 1024, 3 * 1024 * 1024] {
        let r = send(padded(total)).await;
        let r = h.checked(r, "post", "/api/user-tool-servers");
        assert_eq!(r.status, 400, "{total} bytes: {}", r.text());
    }
    assert!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty(),
        "an oversize body added a server"
    );
    // a body of exactly the cap is read (the limit is the cap, not one under it)
    let r = send(padded(cap)).await;
    let r = h.checked(r, "post", "/api/user-tool-servers");
    assert_eq!(r.status, 201, "{}", r.text());
}

#[tokio::test]
async fn an_unknown_member_beside_valid_ones_is_a_400_for_an_edit_and_renames_nothing() {
    // `{"nam": …}` alone is a 400 for having nothing to change, whatever the unknown-member check
    // does: the body here is one that WOULD rename the server if the unknown member were let through
    let h = Harness::full().await;
    let before = h.add("alice", bearer("a")).await;
    let id = id_of(&before);
    let planted = "SENTINEL-planted-c3d4";
    for (label, body) in [
        ("a mistyped secret", json!({"name": "x", "secrte": planted})),
        ("an unknown member", json!({"name": "x", planted: 1})),
        (
            "a member of the add request",
            json!({"name": "x", "kind": "none"}),
        ),
    ] {
        let r = h.patch("alice", &id, body).await;
        assert_eq!(r.status, 400, "{label}: {}", r.text());
        assert_eq!(
            r.json()["detail"],
            "unknown member; the members are `name`, `url`, `auth`, `secret` and `revision`",
            "{label}"
        );
        assert!(!r.text().contains(planted), "{label}: {}", r.text());
    }
    let after = h.list("alice").await.json()["servers"][0].clone();
    assert_eq!(after["name"], "a", "renamed");
    assert_eq!(after["revision"], before["revision"], "written");
}

#[tokio::test]
async fn auth_refuses_an_unknown_member_and_a_member_of_another_kind_each_in_its_own_words() {
    // two checks guard `auth`: the members no kind has, and the members another kind has. Each says
    // something different, and a body that only one of them refuses is the proof that it is there
    let h = Harness::full().await;
    let planted = "SENTINEL-planted-e5f6";
    let unknown =
        "unknown member of `auth`; the members are `kind`, `header`, `scopes` and `clientId`";
    let other_kind = "`auth` has a member that is not for this kind";
    for (label, auth, detail) in [
        (
            "an unknown member of none",
            json!({"kind": "none", planted: 1}),
            unknown,
        ),
        (
            "an unknown member of api_key",
            json!({"kind": "api_key", "header": "X-A", planted: 1}),
            unknown,
        ),
        (
            "an unknown member of oauth2",
            json!({"kind": "oauth2", planted: 1}),
            unknown,
        ),
        (
            "a header on a bearer",
            json!({"kind": "bearer", "header": "X-A"}),
            other_kind,
        ),
        (
            "scopes on a none",
            json!({"kind": "none", "scopes": ["a"]}),
            other_kind,
        ),
        (
            "a client id on an api_key",
            json!({"kind": "api_key", "header": "X-A", "clientId": "c"}),
            other_kind,
        ),
    ] {
        let r = h
            .post_unchecked(
                "alice",
                &json!({"name": "x", "url": URL, "auth": auth, "secret": TOKEN}),
            )
            .await;
        // `secret` is for a bearer or an api_key; the 400 comes from `auth`, before that is looked at
        assert_eq!(r.status, 400, "{label}: {}", r.text());
        assert_eq!(r.json()["detail"], detail, "{label}");
        assert!(!r.text().contains(planted), "{label}");
    }
    assert!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_secret_at_the_cap_is_kept_even_made_of_the_characters_json_doubles() {
    // the broker seals the secret and the URL as JSON, where a quote or a backslash costs two bytes:
    // the cap leaves room for the worst secret in a record the store takes (16 KiB)
    let h = Harness::full().await;
    for worst in ["\"", "\\"] {
        let secret = worst.repeat(4096);
        let r = h
            .post(
                "alice",
                json!({"name": "big", "url": URL, "auth": {"kind": "bearer"}, "secret": secret}),
            )
            .await;
        assert_eq!(r.status, 201, "{}", r.text());
        assert_eq!(r.json()["state"], "connected");
        let r = h.delete("alice", r.json()["id"].as_str().unwrap()).await;
        assert_eq!(r.status, 204);
        // one more is refused before anything is written
        let r = h
            .post_unchecked(
                "alice",
                &json!({"name": "big", "url": URL, "auth": {"kind": "bearer"}, "secret": worst.repeat(4097)}),
            )
            .await;
        assert_eq!(r.status, 400, "{}", r.text());
    }
    assert!(h.secrets.is_empty());
}

#[tokio::test]
async fn an_edit_that_changes_nothing_or_names_unknown_members_is_a_400() {
    let h = Harness::full().await;
    let id = id_of(&h.add("alice", none("a")).await);
    for (label, body) in [
        ("nothing", json!({})),
        ("only a revision", json!({"revision": "1"})),
        ("an unknown member", json!({"nam": "x"})),
        (
            "a revision that is a number",
            json!({"name": "b", "revision": 1}),
        ),
        (
            "a revision that is not a number",
            json!({"name": "b", "revision": "latest"}),
        ),
        (
            "a secret for a server that wants none",
            json!({"secret": TOKEN}),
        ),
        ("a private url", json!({"url": "https://192.168.1.10/"})),
        ("a bad kind", json!({"auth": {"kind": "kerberos"}})),
    ] {
        let r = h.patch("alice", &id, body).await;
        assert_eq!(r.status, 400, "{label}: {}", r.text());
        assert!(!r.text().contains(TOKEN), "{label}");
    }
    assert_eq!(h.list("alice").await.json()["servers"][0]["name"], "a");
}

#[tokio::test]
async fn a_url_the_guard_refuses_is_shown_the_public_reason_and_never_the_rule() {
    let h = Harness::full().await;
    for (url, shown) in [
        (
            "https://10.0.0.1/mcp",
            "that server cannot be reached from here",
        ),
        (
            "https://169.254.169.254/latest",
            "that server cannot be reached from here",
        ),
        (
            "https://localhost/mcp",
            "that server cannot be reached from here",
        ),
        (
            "http://mcp.example.com/mcp",
            "the URL scheme is not allowed: https only",
        ),
        (
            "https://mcp.example.com/mcp?key=1",
            "the URL must not carry a query",
        ),
    ] {
        let r = h
            .post(
                "alice",
                json!({"name": "x", "url": url, "auth": {"kind": "none"}}),
            )
            .await;
        assert_eq!(r.status, 400, "{url}: {}", r.text());
        assert_eq!(r.code().as_deref(), Some("url_refused"));
        assert_eq!(r.json()["detail"], shown, "{url}");
        for rule in ["private-address", "no-address", "deny-list", "allow-list"] {
            assert!(!r.text().contains(rule), "{url}: {}", r.text());
        }
        // an edit says the same
        let added = h.add("alice", none("ok")).await;
        let r = h.patch("alice", &id_of(&added), json!({"url": url})).await;
        assert_eq!(r.status, 400);
        assert_eq!(r.code().as_deref(), Some("url_refused"));
        assert_eq!(r.json()["detail"], shown, "{url}");
        h.delete("alice", &id_of(&added)).await;
    }
}

#[tokio::test]
async fn the_ninth_server_is_a_409_and_a_deleted_one_gives_its_place_back() {
    let h = Harness::full().await;
    let mut ids = Vec::new();
    for n in 0..8 {
        ids.push(id_of(&h.add("alice", none(&format!("s{n}"))).await));
    }
    let r = h.post("alice", none("ninth")).await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("too_many_servers"));
    assert!(r.json()["detail"].as_str().unwrap().contains('8'));
    // Bob has his own eight
    h.add("bob", none("bob's")).await;
    assert_eq!(h.delete("alice", &ids[0]).await.status, 204);
    h.add("alice", none("ninth")).await;
    // an edit at the limit is not a ninth
    let r = h.patch("alice", &ids[1], json!({"name": "renamed"})).await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn an_edit_on_a_stale_revision_is_a_409_and_writes_nothing() {
    let h = Harness::full().await;
    let added = h.add("alice", none("a")).await;
    let (id, first) = (id_of(&added), added["revision"].clone());
    let r = h
        .patch("alice", &id, json!({"name": "b", "revision": first}))
        .await;
    assert_eq!(r.status, 200, "{}", r.text());
    let second = r.json()["revision"].clone();
    assert_ne!(second, first);
    // the first revision is stale now
    let r = h
        .patch("alice", &id, json!({"name": "c", "revision": first}))
        .await;
    assert_eq!(r.status, 409, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("stale_revision"));
    assert_eq!(h.list("alice").await.json()["servers"][0]["name"], "b");
    // the current one is not
    let r = h
        .patch("alice", &id, json!({"name": "c", "revision": second}))
        .await;
    assert_eq!(r.status, 200);
    // the listing's revision is the one to send back
    let listed = h.list("alice").await.json()["servers"][0]["revision"].clone();
    assert_eq!(listed, r.json()["revision"]);
}

#[tokio::test]
async fn a_deployment_that_keeps_no_credentials_takes_only_servers_that_want_none() {
    let h = Harness::without_credentials().await;
    let added = h.add("alice", none("Docs")).await;
    assert_eq!(added["status"], "ready");
    let r = h.list("alice").await;
    assert_eq!(r.json()["kinds"], json!(["none"]));
    for body in [
        bearer("x"),
        api_key("x"),
        json!({"name": "x", "url": URL, "auth": {"kind": "oauth2"}}),
    ] {
        let r = h.post("alice", body).await;
        assert_eq!(r.status, 422, "{}", r.text());
        assert_eq!(r.code().as_deref(), Some("credentials_not_kept"));
        assert!(!r.text().contains(TOKEN) && !r.text().contains(KEY));
    }
    // and an edit into a kind that wants one
    let r = h
        .patch(
            "alice",
            &id_of(&added),
            json!({"auth": {"kind": "bearer"}, "secret": TOKEN}),
        )
        .await;
    assert_eq!(r.status, 422, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("credentials_not_kept"));
    // a server that holds a credential from before (the operator removed the section): it is
    // unavailable, its name can change, it cannot be moved, and it can be deleted
    let target = EgressPolicy::new().check_url(URL).unwrap();
    let stranded = UserToolServer::new(
        UserToolServerId::generate(),
        "Stranded",
        &target,
        UserAuth::Bearer,
    )
    .unwrap()
    .with_state(UserToolServerState::Connected)
    .with_credential(orch_ports::CredentialRef::new("cred-old-v1").unwrap());
    h.servers
        .inner
        .insert(&UserId::new(ALICE), &stranded)
        .await
        .unwrap();
    let r = h.list("alice").await;
    let strand = r.json()["servers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "Stranded")
        .unwrap()
        .clone();
    assert_eq!(strand["status"], "unavailable");
    assert_eq!(strand["reason"], "broker_down");
    let r = h.picker("alice").await;
    assert!(
        r.json()
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["reason"] == "broker_down")
    );
    let sid = id_of(&strand);
    assert_eq!(
        h.patch("alice", &sid, json!({"name": "Renamed"}))
            .await
            .status,
        200
    );
    let r = h.patch("alice", &sid, json!({"url": OTHER_URL})).await;
    assert_eq!(r.status, 422);
    assert_eq!(r.code().as_deref(), Some("credentials_not_kept"));
    // NoBroker refuses a revoke: the delete succeeding is the proof that none was asked
    assert_eq!(h.delete("alice", &sid).await.status, 204);
}

#[tokio::test]
async fn a_broker_that_cannot_keep_the_secret_is_a_503_and_leaves_nothing() {
    let h = Harness::full().await;
    h.secrets.set_down(true);
    let r = h.post("alice", bearer("GitHub")).await;
    assert_eq!(r.status, 503, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("broker_down"));
    assert!(r.header("retry-after").is_some());
    assert!(!r.text().contains(TOKEN));
    // the definition was taken back: no server, and no place spent
    assert!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    h.secrets.set_down(false);
    let added = h.add("alice", bearer("GitHub")).await;
    let id = id_of(&added);

    // a revoke that cannot be done aborts the delete: the server stays
    h.secrets.set_down(true);
    let r = h.delete("alice", &id).await;
    assert_eq!(r.status, 503, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("broker_down"));
    assert_eq!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // a move of its URL writes the definition first and then cannot revoke: the answer says so, and
    // the server is `added` with no credential named, which is what the person asked for (the
    // sealed record that is left is not lent, and the delete below forgets it)
    let r = h.patch("alice", &id, json!({"url": OTHER_URL})).await;
    assert_eq!(r.status, 503, "{}", r.text());
    assert_eq!(r.code().as_deref(), Some("broker_down"));
    let moved = h.list("alice").await.json()["servers"][0].clone();
    assert_eq!(moved["url"], OTHER_URL);
    assert_eq!(moved["state"], "added");
    assert_eq!(moved["hasCredential"], false);
    assert_eq!(moved["reason"], "not_connected");
    assert_eq!(h.secrets.len(), 1, "the revoke did not get through");
    h.secrets.set_down(false);
    assert_eq!(h.delete("alice", &id).await.status, 204);
    assert!(h.secrets.is_empty());
}

#[tokio::test]
async fn a_definition_that_does_not_read_back_is_reported_and_can_still_be_deleted_by_its_id() {
    let h = Harness::full().await;
    let mine = id_of(&h.add("alice", bearer("mine")).await);
    h.add("bob", none("bob's")).await;
    h.servers.corrupt.store(true, Ordering::SeqCst);

    let r = h.list("alice").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json()["servers"], json!([]));
    assert_eq!(r.json()["error"]["code"], "unreadable");
    // the person is told where the id of the damaged row is to be found, and that it is not in this answer
    let detail = r.json()["error"]["detail"].as_str().unwrap().to_owned();
    assert!(detail.contains("orchestrator's log"), "{detail}");
    assert!(detail.contains("delete"), "{detail}");
    assert!(
        !r.text().contains(&mine),
        "the damaged row is not named: {}",
        r.text()
    );
    // the picker still lists the deployment's servers, and the person's own are left out
    let r = h.picker("alice").await;
    assert_eq!(r.status, 200, "{}", r.text());
    assert_eq!(r.json(), json!([{"id": "docs", "name": "Documentation"}]));
    // an edit cannot read the row; the delete does not need to
    let r = h
        .json(
            reqwest::Method::PATCH,
            &format!("/api/user-tool-servers/{mine}"),
            "alice",
            Some(&json!({"name": "x"})),
        )
        .await;
    assert_eq!(r.status, 500, "{}", r.text());
    assert_eq!(r.json()["detail"], "internal error");
    let r = h.delete("alice", &mine).await;
    assert_eq!(r.status, 204, "{}", r.text());
    assert!(h.secrets.is_empty(), "the credential went too");
    h.servers.corrupt.store(false, Ordering::SeqCst);
    assert!(
        h.list("alice").await.json()["servers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn a_store_that_is_down_is_a_503_and_not_an_empty_list() {
    let h = Harness::full().await;
    h.servers.inner.set_down(true);
    for r in [
        h.list("alice").await,
        h.picker("alice").await,
        h.post("alice", none("x")).await,
    ] {
        assert_eq!(r.status, 503, "{}", r.text());
        assert!(r.header("retry-after").is_some());
    }
}

#[tokio::test]
async fn the_picker_lists_the_deployments_servers_then_the_persons_by_name() {
    let h = Harness::full().await;
    h.add("alice", none("beta")).await;
    h.add("alice", none("Alpha")).await;
    let list = h.picker("alice").await.json();
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Documentation", "Alpha", "beta"]);
    // the deployment's entry has not changed shape
    assert_eq!(list[0], json!({"id": "docs", "name": "Documentation"}));
    assert_eq!(list[1]["own"], true);
    // a URL, a header and the kind are in the management listing and not in the picker
    for entry in list.as_array().unwrap() {
        for member in ["url", "auth", "state", "revision", "hasCredential"] {
            assert!(
                entry.get(member).is_none(),
                "{member} is in the picker: {entry}"
            );
        }
    }
}

// ---- a request that is dropped mid-sequence ------------------------------------------------------

/// How a request is dropped: the client hangs up, or the server's own timeout fires. Either way the
/// future of the handler is dropped at its next `await`.
#[derive(Debug, Clone, Copy)]
enum Gone {
    Client,
    Timeout,
}

impl Gone {
    fn config(self) -> ApiConfig {
        match self {
            Gone::Client => ApiConfig::default(),
            Gone::Timeout => ApiConfig {
                request_timeout: Duration::from_millis(300),
                ..ApiConfig::default()
            },
        }
    }
}

/// Polls `check` until it holds, for five seconds.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    for _ in 0..100 {
        if check().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("never happened: {what}");
}

impl Harness {
    /// Sends the request, waits until it is held at `pause`, and drops it the way `gone` says. The
    /// handler's future is not let go on: `pause` is, by the caller, afterwards.
    async fn dropped_at(
        &self,
        gone: Gone,
        pause: &Pause,
        method: reqwest::Method,
        path: &str,
        token: &str,
        body: Option<&Value>,
    ) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(token);
        if let Some(body) = body {
            request = request
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }
        let sent = tokio::spawn(request.send());
        tokio::time::timeout(Duration::from_secs(5), pause.reached.notified())
            .await
            .expect("the request got to the place where it is held");
        match gone {
            Gone::Client => {
                // the client hangs up
                sent.abort();
            }
            Gone::Timeout => {
                // the server's timeout answers, and drops the handler
                let answered = sent.await.unwrap().unwrap();
                assert_eq!(answered.status().as_u16(), 503);
            }
        }
        // time for the server to notice, and to drop the handler's future if it is going to
        tokio::time::sleep(Duration::from_millis(400)).await;
    }

    async fn own_servers(&self, token: &str) -> Vec<Value> {
        self.list(token).await.json()["servers"]
            .as_array()
            .unwrap()
            .clone()
    }
}

#[tokio::test]
async fn an_add_that_is_dropped_mid_sequence_still_takes_its_definition_back() {
    for gone in [Gone::Client, Gone::Timeout] {
        let h = Harness::full_with(gone.config()).await;
        // the definition is written; the add is held before it asks the broker to keep the secret
        let pause = h.servers.hold(Hold::AfterInsert);
        h.dropped_at(
            gone,
            &pause,
            reqwest::Method::POST,
            "/api/user-tool-servers",
            "alice",
            Some(&bearer("GitHub")),
        )
        .await;
        // by the time the add goes on, the broker cannot keep it
        h.secrets.set_down(true);
        pause.release.notify_one();
        eventually(
            &format!("{gone:?}: the definition is taken back"),
            || async { h.own_servers("alice").await.is_empty() },
        )
        .await;
        assert!(
            !pause.dropped.load(Ordering::SeqCst),
            "{gone:?}: the handler's future was dropped in the middle of the add"
        );
        // no place of the eight is spent: eight more fit
        h.secrets.set_down(false);
        for n in 0..8 {
            let r = h.post("alice", none(&format!("s{n}"))).await;
            assert_eq!(r.status, 201, "{gone:?}: {}", r.text());
        }
        assert_eq!(h.post("alice", none("ninth")).await.status, 409);
    }
}

#[tokio::test]
async fn an_edit_that_is_dropped_mid_sequence_finishes_the_move() {
    for gone in [Gone::Client, Gone::Timeout] {
        let h = Harness::full_with(gone.config()).await;
        let id = id_of(&h.add("alice", bearer("GitHub")).await);
        // held after the definition is written `added`: the old credential is not revoked yet and
        // the new key is not kept
        let pause = h.servers.hold(Hold::AfterReplace);
        h.dropped_at(
            gone,
            &pause,
            reqwest::Method::PATCH,
            &format!("/api/user-tool-servers/{id}"),
            "alice",
            Some(&json!({"url": OTHER_URL, "secret": KEY})),
        )
        .await;
        pause.release.notify_one();
        eventually(&format!("{gone:?}: the move is finished"), || async {
            let servers = h.own_servers("alice").await;
            servers[0]["state"] == "connected" && servers[0]["url"] == OTHER_URL
        })
        .await;
        assert!(
            !pause.dropped.load(Ordering::SeqCst),
            "{gone:?}: the handler's future was dropped in the middle of the edit"
        );
        let servers = h.own_servers("alice").await;
        assert_eq!(servers[0]["hasCredential"], true, "{gone:?}");
        assert_eq!(h.secrets.len(), 1, "{gone:?}: one credential, the new one");
    }
}

#[tokio::test]
async fn a_delete_that_is_dropped_between_its_revoke_and_its_delete_still_deletes() {
    for gone in [Gone::Client, Gone::Timeout] {
        let h = Harness::full_with(gone.config()).await;
        let id = id_of(&h.add("alice", bearer("GitHub")).await);
        // held after the credential is revoked, before the definition goes
        let pause = h.servers.hold(Hold::BeforeDelete);
        h.dropped_at(
            gone,
            &pause,
            reqwest::Method::DELETE,
            &format!("/api/user-tool-servers/{id}"),
            "alice",
            None,
        )
        .await;
        pause.release.notify_one();
        eventually(&format!("{gone:?}: the server is deleted"), || async {
            h.own_servers("alice").await.is_empty()
        })
        .await;
        assert!(
            !pause.dropped.load(Ordering::SeqCst),
            "{gone:?}: the handler's future was dropped in the middle of the delete"
        );
        assert!(h.secrets.is_empty(), "{gone:?}");
    }
}

#[tokio::test]
async fn a_write_that_panics_is_a_500_with_a_fixed_body_and_the_server_goes_on() {
    let h = Harness::full().await;
    h.servers.panic_on_insert.store(true, Ordering::SeqCst);
    let r = h
        .json(
            reqwest::Method::POST,
            "/api/user-tool-servers",
            "alice",
            Some(&none("boom")),
        )
        .await;
    let r = h.checked(r, "post", "/api/user-tool-servers");
    assert_eq!(r.status, 500, "{}", r.text());
    assert_eq!(r.json()["detail"], "internal error");
    assert!(!r.text().contains("panics"), "{}", r.text());
    // the process is up, and the next request is served
    assert_eq!(h.post("alice", none("fine")).await.status, 201);
}

#[tokio::test]
async fn the_four_routes_answer_the_methods_they_have() {
    let h = Harness::full().await;
    for (method, path) in [
        (reqwest::Method::PUT, "/api/user-tool-servers"),
        (reqwest::Method::DELETE, "/api/user-tool-servers"),
        (reqwest::Method::GET, "/api/user-tool-servers/u-abcdefghij"),
        (reqwest::Method::POST, "/api/user-tool-servers/u-abcdefghij"),
    ] {
        let r = h.json(method.clone(), path, "alice", None).await;
        assert_eq!(r.status, 405, "{method} {path}");
    }
}

#[tokio::test]
async fn the_requests_the_tests_send_are_requests_of_the_contract() {
    let h = Harness::full().await;
    for body in [none("a"), bearer("b"), api_key("c")] {
        h.contract.request("/api/user-tool-servers", "post", &body);
    }
    h.contract.request(
        "/api/user-tool-servers/{serverId}",
        "patch",
        &json!({"name": "x", "revision": "7", "secret": TOKEN}),
    );
}

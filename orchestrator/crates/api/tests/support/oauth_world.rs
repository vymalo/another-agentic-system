//! The world of the OAuth tests (ADR 0046, slice 4c): the orchestrator's real router over the real
//! credential broker and the in-memory stores, an authorization server of the broker's own tests
//! (`orch-credential-broker`'s `tests/support`, included as source: it plays the MCP server and its
//! authorization server on a loopback address), and a client that plays the person's browser.
//!
//! Included by `#[path]` from `user_tools_oauth.rs` and `user_tools_oauth_secret.rs`, which are
//! separate processes (the second captures the logs).
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orch_api::ApiConfig;
use orch_app::{AgentDirectory, App, AppConfig, Permission, Policy, RoleGrant};
use orch_core::UserId;
use orch_credential_broker::Broker;
use orch_ports::memory::{
    FixedClock, MemoryAuth, MemoryPendingAuthorizations, MemorySecretStore, MemoryStore,
    MemoryUserToolServers, MemoryWakeup, ScriptedAgent, SeqIds,
};
use orch_ports::{
    EgressPolicy, NoArtifacts, NoBroker, NoModel, NoUserTools, PortSet, Principal, RedirectUri,
    Role, UserToolPorts, UserTools,
};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

#[path = "../../../credential-broker/tests/support/mod.rs"]
pub mod as_support;
pub use as_support::{Cfg, MockAs};

pub const ALICE: &str = "alice@example.com";
pub const BOB: &str = "bob@example.com";
/// Where the callback is, for a world that has not been told otherwise.
pub const CALLBACK_PATH: &str = "/api/user-tool-servers/oauth/callback";
/// The prefix the authorization server gives the codes and the tokens it issues, and the verifier it
/// is shown: nothing the orchestrator says may carry anything that starts with it.
pub const SENTINEL: &str = "SENTINEL-";

pub fn start_time() -> orch_core::Timestamp {
    orch_core::Timestamp::from_second(1_790_000_000).unwrap()
}

// ---- the contract -------------------------------------------------------------------------------

pub struct Contract(pub Value);

impl Contract {
    pub fn load() -> Self {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../docs/api/chat-api.yaml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        Contract(serde_norway::from_str(&text).unwrap())
    }

    /// The declared response of `method path` for `status`: its media type and schema, `None` for a
    /// status the contract does not document.
    pub fn response(
        &self,
        path: &str,
        method: &str,
        status: u16,
    ) -> Option<Option<(String, Value)>> {
        let mut response = self.0["paths"][path][method]["responses"]
            .get(status.to_string())
            .cloned()?;
        if let Some(reference) = response.get("$ref").and_then(Value::as_str) {
            let name = reference.rsplit('/').next().unwrap();
            response = self.0["components"]["responses"][name].clone();
        }
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            return Some(None);
        };
        let (content_type, media) = content.iter().next().unwrap();
        Some(Some((
            content_type.clone(),
            media.get("schema").cloned().unwrap_or_else(|| json!({})),
        )))
    }

    pub fn validate(&self, schema: &Value, instance: &Value) {
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
}

// ---- a response ---------------------------------------------------------------------------------

pub struct Resp {
    pub status: u16,
    pub headers: reqwest::header::HeaderMap,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn code(&self) -> Option<String> {
        self.json()["code"].as_str().map(str::to_owned)
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The whole of what a client saw: status, headers and body.
    pub fn transcript(&self) -> String {
        format!("{}\n{:?}\n{}", self.status, self.headers, self.text())
    }
}

// ---- the world ----------------------------------------------------------------------------------

pub struct Options {
    /// The API's configuration (the rate limits, the document).
    pub api: ApiConfig,
    /// Whether the application knows where the callback is.
    pub with_redirect: bool,
    /// The egress policy of the application and the broker.
    pub egress: EgressPolicy,
    /// What the authorization server is told to do.
    pub mock: Box<dyn FnOnce(&mut Cfg)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            api: ApiConfig::default(),
            with_redirect: true,
            egress: as_support::policy(),
            mock: Box::new(|cfg| cfg.sentinel = Some(SENTINEL.to_owned())),
        }
    }
}

pub struct World {
    /// The orchestrator's origin, `http://127.0.0.1:<port>`.
    pub base: String,
    /// The person's browser: follows no redirect and sends nothing it was not told to.
    pub browser: reqwest::Client,
    pub mock: MockAs,
    pub servers: MemoryUserToolServers,
    pub secrets: MemorySecretStore,
    pub pending: MemoryPendingAuthorizations,
    pub clock: FixedClock,
    pub contract: Contract,
    server: JoinHandle<()>,
}

impl Drop for World {
    fn drop(&mut self) {
        self.server.abort();
    }
}

pub fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(UserId::new(email))
    }
}

/// `user` is the built-in role; `maker` holds `tools.user.add` and nothing else.
pub fn policy() -> Policy {
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
        None,
    )
    .unwrap()
}

impl World {
    /// Credentials are kept, the authorization server is the mock, and the application knows where
    /// the callback is.
    pub async fn full() -> World {
        Self::full_with(Options::default()).await
    }

    pub async fn full_with(options: Options) -> World {
        let servers = MemoryUserToolServers::new();
        let secrets = MemorySecretStore::new();
        let pending = MemoryPendingAuthorizations::new();
        let clock = FixedClock::new(start_time());
        let broker = Broker::builder(secrets.clone(), servers.clone(), options.egress.clone())
            .clock(clock.clone())
            .build()
            .unwrap();
        let bundle = UserTools::<
            MemoryUserToolServers,
            Broker<MemorySecretStore, MemoryUserToolServers, FixedClock>,
            MemoryPendingAuthorizations,
        >::full(
            options.egress.clone(),
            servers.clone(),
            broker,
            pending.clone(),
        );
        Self::start(bundle, servers, secrets, pending, clock, options).await
    }

    /// As [`full`](Self::full), over a store of definitions that stops **after** the `replace` that
    /// the gate was armed for has written (the moment, in an edit that moves a server, between the
    /// write of the new definition and the revoke that follows it). `World::servers` is the store
    /// underneath, which never stops.
    pub async fn full_pausing() -> (World, Arc<as_support::Gate>) {
        let options = Options::default();
        let servers = MemoryUserToolServers::new();
        let paused = as_support::PausedAfterReplace::new(servers.clone());
        let gate = paused.gate();
        let secrets = MemorySecretStore::new();
        let pending = MemoryPendingAuthorizations::new();
        let clock = FixedClock::new(start_time());
        let broker = Broker::builder(secrets.clone(), paused.clone(), options.egress.clone())
            .clock(clock.clone())
            .build()
            .unwrap();
        let bundle = UserTools::<
            as_support::PausedAfterReplace<MemoryUserToolServers>,
            Broker<
                MemorySecretStore,
                as_support::PausedAfterReplace<MemoryUserToolServers>,
                FixedClock,
            >,
            MemoryPendingAuthorizations,
        >::full(options.egress.clone(), paused, broker, pending.clone());
        let world = Self::start(bundle, servers, secrets, pending, clock, options).await;
        (world, gate)
    }

    /// Enabled, with no `secrets` section.
    pub async fn without_credentials() -> World {
        let servers = MemoryUserToolServers::new();
        let pending = MemoryPendingAuthorizations::new();
        let options = Options::default();
        let bundle = UserTools::<
            MemoryUserToolServers,
            NoBroker,
            MemoryPendingAuthorizations,
        >::without_credentials(options.egress.clone(), servers.clone());
        Self::start(
            bundle,
            servers,
            MemorySecretStore::new(),
            pending,
            FixedClock::new(start_time()),
            options,
        )
        .await
    }

    /// The feature is off.
    pub async fn off() -> World {
        Self::start(
            NoUserTools::off(),
            MemoryUserToolServers::new(),
            MemorySecretStore::new(),
            MemoryPendingAuthorizations::new(),
            FixedClock::new(start_time()),
            Options::default(),
        )
        .await
    }

    async fn start<T: UserToolPorts>(
        user_tools: T,
        servers: MemoryUserToolServers,
        secrets: MemorySecretStore,
        pending: MemoryPendingAuthorizations,
        clock: FixedClock,
        options: Options,
    ) -> World {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mock = MockAs::start().await;
        mock.configure(options.mock);
        let auth = MemoryAuth::new();
        auth.allow("alice", principal(ALICE, &["user", "maker"]));
        auth.allow("bob", principal(BOB, &["user", "maker"]));
        auth.allow("carol", principal("carol@example.com", &["user"]));
        let directory = AgentDirectory::new(Vec::new());
        let redirect = format!("http://{addr}{CALLBACK_PATH}");
        let app = Arc::new(
            App::new(
                PortSet {
                    store: MemoryStore::new(),
                    wakeup: MemoryWakeup::new(),
                    agents: ScriptedAgent::new(),
                    clock: clock.clone(),
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
                    user_tool_redirect: options
                        .with_redirect
                        .then(|| RedirectUri::parse(&redirect).unwrap()),
                    ..AppConfig::default()
                },
            )
            .unwrap(),
        );
        let router = orch_api::router_with_surfaces(app, options.api, Vec::new());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        World {
            base: format!("http://{addr}"),
            browser: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            mock,
            servers,
            secrets,
            pending,
            clock,
            contract: Contract::load(),
            server,
        }
    }

    // ---- requests

    async fn answer(resp: reqwest::Response) -> Resp {
        Resp {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body: resp.bytes().await.unwrap().to_vec(),
        }
    }

    /// A request of the app: with `token` as a bearer, and `body` as JSON when there is one.
    pub async fn api(
        &self,
        method: reqwest::Method,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Resp {
        let mut req = self.browser.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(body.to_string());
        }
        Self::answer(req.send().await.unwrap()).await
    }

    /// A request to the orchestrator as a browser that signed in nowhere sends it: nothing but the
    /// request line.
    pub async fn bare_get(&self, path_and_query: &str) -> Resp {
        let resp = self
            .browser
            .get(format!("{}{path_and_query}", self.base))
            .send()
            .await
            .unwrap();
        Self::answer(resp).await
    }

    /// `GET url`, with no redirect followed (the authorization server's, or the orchestrator's).
    pub async fn get_url(&self, url: &str) -> Resp {
        Self::answer(self.browser.get(url).send().await.unwrap()).await
    }

    /// Validates a JSON response against what the contract documents for `method path`, and returns it.
    pub fn checked(&self, resp: Resp, method: &str, template: &str) -> Resp {
        match self
            .contract
            .response(template, method, resp.status)
            .unwrap_or_else(|| {
                panic!(
                    "the contract documents no {} for {method} {template}: {}",
                    resp.status,
                    resp.text()
                )
            }) {
            Some((content_type, schema)) => {
                let got = resp.header("content-type").unwrap_or_default();
                assert!(
                    got.starts_with(&content_type),
                    "{method} {template} {}: content-type {got:?}, the contract says {content_type}",
                    resp.status
                );
                if content_type.contains("json") {
                    self.contract.validate(&schema, &resp.json());
                }
            }
            None => assert!(resp.body.is_empty(), "{}", resp.text()),
        }
        resp
    }

    // ---- the servers of a person

    /// Adds an `oauth2` server of `token`'s person at the mock, and returns its id.
    pub async fn add_oauth(&self, token: &str) -> String {
        self.add_oauth_scoped(token, &["repo"]).await
    }

    pub async fn add_oauth_scoped(&self, token: &str, scopes: &[&str]) -> String {
        let body = json!({
            "name": "GitHub",
            "url": self.mock.mcp_url(),
            "auth": {"kind": "oauth2", "scopes": scopes},
        });
        let resp = self
            .api(
                reqwest::Method::POST,
                "/api/user-tool-servers",
                Some(token),
                Some(&body),
            )
            .await;
        let resp = self.checked(resp, "post", "/api/user-tool-servers");
        assert_eq!(resp.status, 201, "{}", resp.text());
        resp.json()["id"].as_str().unwrap().to_owned()
    }

    /// `POST …/authorize` as `token`, checked against the contract.
    pub async fn authorize(&self, token: &str, id: &str) -> Resp {
        let resp = self
            .api(
                reqwest::Method::POST,
                &format!("/api/user-tool-servers/{id}/authorize"),
                Some(token),
                None,
            )
            .await;
        self.checked(resp, "post", "/api/user-tool-servers/{serverId}/authorize")
    }

    /// The person's server as `GET /api/user-tool-servers` lists it.
    pub async fn listed(&self, token: &str, id: &str) -> Value {
        let resp = self
            .api(
                reqwest::Method::GET,
                "/api/user-tool-servers",
                Some(token),
                None,
            )
            .await;
        let resp = self.checked(resp, "get", "/api/user-tool-servers");
        assert_eq!(resp.status, 200, "{}", resp.text());
        resp.json()["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} is not listed: {}", resp.text()))
    }

    /// The server as the picker (`GET /api/tool-servers`) shows it.
    pub async fn picked(&self, token: &str, id: &str) -> Value {
        let resp = self
            .api(reqwest::Method::GET, "/api/tool-servers", Some(token), None)
            .await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        resp.json()
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} is not in the picker: {}", resp.text()))
    }

    // ---- the browser

    /// What the person's browser does with `authorize_url`: asks the authorization server, which
    /// consents at once and answers a redirect to the callback; returns that address as the browser
    /// would request it from the orchestrator (`/api/…?code=…&state=…&iss=…`).
    pub async fn consent(&self, authorize_url: &str) -> String {
        let resp = self.get_url(authorize_url).await;
        assert_eq!(
            resp.status,
            302,
            "the authorization server: {}",
            resp.text()
        );
        let location = resp.header("location").expect("a redirect").to_owned();
        let url = url::Url::parse(&location).unwrap();
        assert_eq!(url.origin().ascii_serialization(), self.base, "{location}");
        match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        }
    }

    /// Begins an authorization of `id` for `token`'s person and consents: the callback address that
    /// the person's browser would be sent to.
    pub async fn callback_of(&self, token: &str, id: &str) -> String {
        let resp = self.authorize(token, id).await;
        assert_eq!(resp.status, 200, "{}", resp.text());
        let url = resp.json()["authorizeUrl"].as_str().unwrap().to_owned();
        self.consent(&url).await
    }
}

/// `path?query` with the parameter `name` replaced by `value`, or removed when `value` is `None`.
pub fn with_param(path_and_query: &str, name: &str, value: Option<&str>) -> String {
    let url = url::Url::parse(&format!("http://x{path_and_query}")).unwrap();
    let mut pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(n, v)| (n.into_owned(), v.into_owned()))
        .filter(|(n, _)| n != name)
        .collect();
    if let Some(value) = value {
        pairs.push((name.to_owned(), value.to_owned()));
    }
    let query = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish();
    format!("{}?{query}", url.path())
}

/// The value of the query parameter `name` of `path?query`.
pub fn param(path_and_query: &str, name: &str) -> Option<String> {
    url::Url::parse(&format!("http://x{path_and_query}"))
        .unwrap()
        .query_pairs()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.into_owned())
}

/// What a page of the callback must be, whatever it says: HTML, with the headers of the page, and
/// nothing in it that could run, load or leave. Returns its text.
pub fn assert_a_safe_page(resp: &Resp, status: u16) -> String {
    assert_eq!(resp.status, status, "{}", resp.transcript());
    assert_eq!(
        resp.header("content-type"),
        Some("text/html; charset=utf-8"),
        "{}",
        resp.transcript()
    );
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
        ),
        ("referrer-policy", "no-referrer"),
        ("cache-control", "no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("x-robots-tag", "noindex, nofollow"),
    ] {
        assert_eq!(
            resp.header(name),
            Some(value),
            "{name}: {}",
            resp.transcript()
        );
    }
    let page = resp.text();
    let lower = page.to_lowercase();
    for active in [
        "<script",
        "<link",
        "<img",
        "<style",
        "<iframe",
        "<form",
        "<a ",
        "<object",
        "<embed",
        "<base",
        "src=",
        "href=",
        "onload",
        "onerror",
        "javascript:",
        "style=",
        "http-equiv",
    ] {
        assert!(!lower.contains(active), "{active} is on the page: {page}");
    }
    page
}

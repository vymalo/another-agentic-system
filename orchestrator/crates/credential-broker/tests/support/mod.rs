//! Test support: an authorization server (axum on a loopback address) that plays the server of an
//! MCP server's protected resource, with the knobs the tests turn; the fixture the conformance
//! suite of `orch-ports` runs against; and a tripwire that counts connections.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use jiff::Timestamp;
use orch_core::UserId;
use orch_credential_broker::{Broker, BrokerConfig};
use orch_ports::memory::{FixedClock, MemorySecretStore, MemoryUserToolServers};
use orch_ports::testkit::credential_broker::{AsRequests, BrokerFixture, Consent, RefreshOutcome};
use orch_ports::{
    EgressPolicy, RedirectUri, SecretError, SecretMaterial, SecretServer, SecretStore,
    SecretVersion, ServerRevision, ToolSecret, UserAuth, UserToolServer, UserToolServerId,
    UserToolServerStore,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;

// ------------------------------------------------------------------------------ the server

/// What the tests can turn.
#[derive(Clone)]
pub struct Cfg {
    pub s256: bool,
    pub ttl_secs: u64,
    pub token_delay: Duration,
    pub refresh: RefreshOutcome,
    pub sentinel: Option<String>,
    /// Only `openid-configuration` is published.
    pub oidc_only: bool,
    pub revocation: bool,
    pub revocation_status: u16,
    pub registration: bool,
    /// Registration answers with a client secret (`client_secret_basic`).
    pub registration_secret: bool,
    /// `client_id_metadata_document_supported`.
    pub cimd: bool,
    /// A client id registered out of band.
    pub pre_registered: Option<String>,
    pub token_endpoint: Option<String>,
    pub authorization_endpoint: Option<String>,
    /// What the metadata calls the server's origin, when not the address it listens on (a name
    /// the test's resolver maps to it).
    pub origin: Option<String>,
    /// The `resource` that the metadata at the root well-known path declares, when it is served.
    pub root_resource: Option<String>,
    /// The `token_type` of token responses (`Bearer` when none).
    pub token_type: Option<String>,
    pub registration_endpoint: Option<String>,
    pub revocation_endpoint: Option<String>,
    /// A new refresh token with every refresh (the old one still works).
    pub rotate_refresh: bool,
    /// The `resource` the protected resource metadata declares, when not the URL asked about.
    pub declared_resource: Option<String>,
    /// The `issuer` the metadata declares, when not the origin.
    pub declared_issuer: Option<String>,
    /// Answers every request of the metadata with a redirect to this URL.
    pub redirect_metadata_to: Option<String>,
    /// Pads the authorization server metadata to this many bytes.
    pub metadata_padding: usize,
    /// Holds the metadata for this long.
    pub metadata_delay: Duration,
    /// No `expires_in` in token responses.
    pub no_expires_in: bool,
    /// No `refresh_token` in token responses.
    pub no_refresh_token: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            s256: true,
            ttl_secs: 3600,
            token_delay: Duration::ZERO,
            refresh: RefreshOutcome::Succeeds,
            sentinel: None,
            oidc_only: false,
            revocation: true,
            revocation_status: 200,
            registration: true,
            registration_secret: false,
            cimd: false,
            pre_registered: None,
            token_endpoint: None,
            authorization_endpoint: None,
            origin: None,
            root_resource: None,
            token_type: None,
            registration_endpoint: None,
            revocation_endpoint: None,
            rotate_refresh: false,
            declared_resource: None,
            declared_issuer: None,
            redirect_metadata_to: None,
            metadata_padding: 0,
            metadata_delay: Duration::ZERO,
            no_expires_in: false,
            no_refresh_token: false,
        }
    }
}

/// One request the server received.
#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub query: Vec<(String, String)>,
    pub form: Vec<(String, String)>,
    pub json: Option<Value>,
    pub authorization: Option<String>,
}

impl Seen {
    pub fn form(&self, name: &str) -> Option<&str> {
        self.form
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

struct Code {
    client_id: String,
    redirect_uri: String,
    challenge: String,
    resource: String,
}

struct Client {
    redirect_uris: Vec<String>,
    secret: Option<String>,
}

#[derive(Default)]
struct State_ {
    cfg: Cfg,
    seen: Vec<Seen>,
    counter: u64,
    codes: HashMap<String, Code>,
    /// refresh token -> (client id, resource)
    refresh_tokens: HashMap<String, (String, String)>,
    clients: HashMap<String, Client>,
    secrets_seen: Vec<String>,
    code_exchanges: usize,
    refreshes: usize,
    revocations: usize,
}

struct Shared {
    default_origin: String,
    state: Mutex<State_>,
}

fn json_response(status: u16, body: &Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn status_only(status: u16) -> Response {
    StatusCode::from_u16(status).unwrap().into_response()
}

fn basic_of(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = String::from_utf8(STANDARD.decode(encoded).ok()?).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    let decode = |s: &str| {
        url::form_urlencoded::parse(s.as_bytes())
            .next()
            .map(|(k, v)| {
                if v.is_empty() {
                    k.into_owned()
                } else {
                    format!("{k}={v}")
                }
            })
    };
    Some((decode(id)?, decode(secret)?))
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State_> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn origin_in(&self, st: &State_) -> String {
        st.cfg
            .origin
            .clone()
            .unwrap_or_else(|| self.default_origin.clone())
    }

    fn origin(&self) -> String {
        self.origin_in(&self.lock())
    }

    fn record(&self, method: &Method, uri: &Uri, headers: &HeaderMap, body: &[u8]) {
        let query = uri
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        let is_json = headers
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"));
        let (form, json) = if is_json {
            (Vec::new(), serde_json::from_slice(body).ok())
        } else {
            (
                url::form_urlencoded::parse(body).into_owned().collect(),
                None,
            )
        };
        self.lock().seen.push(Seen {
            method: method.to_string(),
            path: uri.path().to_owned(),
            query,
            form,
            json,
            authorization: headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        });
    }

    fn resource_metadata_for(&self, resource: &str) -> Response {
        let st = self.lock();
        json_response(
            200,
            &json!({
                "resource": resource,
                "authorization_servers": [self.origin_in(&st)],
            }),
        )
    }

    fn resource_metadata(&self, path: &str) -> Response {
        let st = self.lock();
        let origin = self.origin_in(&st);
        let resource = st
            .cfg
            .declared_resource
            .clone()
            .unwrap_or_else(|| format!("{origin}{path}"));
        json_response(
            200,
            &json!({
                "resource": resource,
                "authorization_servers": [origin],
                "scopes_supported": ["read", "write"],
            }),
        )
    }

    fn server_metadata(&self) -> Response {
        let st = self.lock();
        let origin = self.origin_in(&st);
        let cfg = &st.cfg;
        let mut doc = json!({
            "issuer": cfg.declared_issuer.clone().unwrap_or_else(|| origin.clone()),
            "authorization_endpoint": cfg.authorization_endpoint.clone().unwrap_or_else(|| format!("{origin}/authorize")),
            "token_endpoint": cfg.token_endpoint.clone().unwrap_or_else(|| format!("{origin}/token")),
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_methods_supported": ["none", "client_secret_basic", "client_secret_post"],
            "client_id_metadata_document_supported": cfg.cimd,
        });
        if cfg.s256 {
            doc["code_challenge_methods_supported"] = json!(["S256"]);
        }
        if cfg.registration {
            doc["registration_endpoint"] = json!(
                cfg.registration_endpoint
                    .clone()
                    .unwrap_or_else(|| format!("{origin}/register"))
            );
        }
        if cfg.revocation {
            doc["revocation_endpoint"] = json!(
                cfg.revocation_endpoint
                    .clone()
                    .unwrap_or_else(|| format!("{origin}/revoke"))
            );
        }
        if cfg.metadata_padding > 0 {
            doc["padding"] = json!("x".repeat(cfg.metadata_padding));
        }
        json_response(200, &doc)
    }

    fn authorize(&self, uri: &Uri) -> Response {
        let query: HashMap<String, String> = uri
            .query()
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        let refuse = |why: &str| (StatusCode::BAD_REQUEST, why.to_owned()).into_response();
        let get = |name: &str| query.get(name).map(String::as_str).unwrap_or_default();
        let mut st = self.lock();
        let origin = self.origin_in(&st);
        if get("response_type") != "code" {
            return refuse("response_type must be code");
        }
        if get("code_challenge_method") != "S256" {
            return refuse("code_challenge_method must be S256");
        }
        if get("code_challenge").len() < 43 {
            return refuse("code_challenge is missing");
        }
        if get("state").is_empty() {
            return refuse("state is missing");
        }
        let resource = get("resource");
        let root = st.cfg.root_resource.clone();
        if resource != format!("{origin}/mcp")
            && resource != format!("{origin}/other")
            && root.as_deref() != Some(resource)
        {
            return refuse("resource is missing or is not a protected resource of this server");
        }
        let client_id = get("client_id").to_owned();
        let redirect_uri = get("redirect_uri").to_owned();
        let known = st.cfg.pre_registered.as_deref() == Some(client_id.as_str())
            || (st.cfg.cimd && client_id.starts_with("https://"))
            || st
                .clients
                .get(&client_id)
                .is_some_and(|c| c.redirect_uris.contains(&redirect_uri));
        if !known {
            return refuse("client_id is not a client of this server for this redirect_uri");
        }
        st.counter += 1;
        let prefix = st.cfg.sentinel.clone().unwrap_or_default();
        let code = format!("{prefix}code-{}", st.counter);
        st.secrets_seen.push(code.clone());
        st.codes.insert(
            code.clone(),
            Code {
                client_id,
                redirect_uri: redirect_uri.clone(),
                challenge: get("code_challenge").to_owned(),
                resource: resource.to_owned(),
            },
        );
        let sep = if redirect_uri.contains('?') { '&' } else { '?' };
        let location = format!(
            "{redirect_uri}{sep}code={}&state={}",
            urlencode(&code),
            urlencode(get("state"))
        );
        (StatusCode::FOUND, [(header::LOCATION, location)]).into_response()
    }

    /// An OAuth error that, when a sentinel is planted, repeats it and everything it was sent.
    fn error(
        st: &State_,
        status: u16,
        code: &str,
        form: &HashMap<String, String>,
        basic: Option<&(String, String)>,
    ) -> Response {
        let mut doc = json!({ "error": code });
        if let Some(sentinel) = &st.cfg.sentinel {
            let mut echo = sentinel.clone();
            for name in ["code", "code_verifier", "refresh_token", "client_secret"] {
                if let Some(v) = form.get(name) {
                    echo.push_str(&format!(" {name}={v}"));
                }
            }
            if let Some((_, secret)) = basic {
                echo.push_str(&format!(" basic_secret={secret}"));
            }
            doc["error_description"] = json!(echo);
        }
        json_response(status, &doc)
    }

    fn issue(&self, st: &mut State_, client_id: &str, resource: &str, with_refresh: bool) -> Value {
        st.counter += 1;
        let n = st.counter;
        let prefix = st.cfg.sentinel.clone().unwrap_or_default();
        let access = format!("{prefix}at-{n}");
        st.secrets_seen.push(access.clone());
        let token_type = st
            .cfg
            .token_type
            .clone()
            .unwrap_or_else(|| "Bearer".to_owned());
        let mut doc = json!({ "access_token": access, "token_type": token_type });
        if !st.cfg.no_expires_in {
            doc["expires_in"] = json!(st.cfg.ttl_secs);
        }
        if with_refresh && !st.cfg.no_refresh_token {
            let refresh = format!("{prefix}rt-{n}");
            st.secrets_seen.push(refresh.clone());
            st.refresh_tokens
                .insert(refresh.clone(), (client_id.to_owned(), resource.to_owned()));
            doc["refresh_token"] = json!(refresh);
        }
        doc
    }

    async fn token(&self, headers: &HeaderMap, body: &[u8]) -> Response {
        let delay = self.lock().cfg.token_delay;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(body).into_owned().collect();
        let basic = basic_of(headers);
        let mut st = self.lock();
        for name in ["code", "code_verifier", "refresh_token", "client_secret"] {
            if let Some(v) = form.get(name) {
                st.secrets_seen.push(v.clone());
            }
        }
        if let Some((_, secret)) = &basic {
            st.secrets_seen.push(secret.clone());
        }
        let client_id = form
            .get("client_id")
            .cloned()
            .or_else(|| basic.as_ref().map(|(id, _)| id.clone()))
            .unwrap_or_default();
        // a client with a secret must authenticate with it
        if let Some(expected) = st.clients.get(&client_id).and_then(|c| c.secret.clone()) {
            let given = basic
                .as_ref()
                .map(|(_, s)| s.clone())
                .or_else(|| form.get("client_secret").cloned());
            if given.as_deref() != Some(expected.as_str()) {
                return Self::error(&st, 401, "invalid_client", &form, basic.as_ref());
            }
        }
        match form.get("grant_type").map(String::as_str) {
            Some("authorization_code") => {
                st.code_exchanges += 1;
                let Some(code) = form.get("code").and_then(|c| st.codes.remove(c)) else {
                    return Self::error(&st, 400, "invalid_grant", &form, basic.as_ref());
                };
                let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
                let ok = code.client_id == client_id
                    && form.get("redirect_uri") == Some(&code.redirect_uri)
                    && form.get("resource") == Some(&code.resource)
                    && challenge == code.challenge;
                if !ok {
                    return Self::error(&st, 400, "invalid_grant", &form, basic.as_ref());
                }
                let doc = self.issue(&mut st, &client_id, &code.resource, true);
                json_response(200, &doc)
            }
            Some("refresh_token") => {
                st.refreshes += 1;
                match st.cfg.refresh {
                    RefreshOutcome::InvalidGrant => {
                        return Self::error(&st, 400, "invalid_grant", &form, basic.as_ref());
                    }
                    RefreshOutcome::ServerError => {
                        let mut text = st.cfg.sentinel.clone().unwrap_or_default();
                        for name in ["refresh_token", "client_secret"] {
                            if let Some(v) = form.get(name) {
                                text.push_str(&format!(" {name}={v}"));
                            }
                        }
                        return (StatusCode::INTERNAL_SERVER_ERROR, text).into_response();
                    }
                    RefreshOutcome::Succeeds => {}
                }
                let Some((owner, resource)) = form
                    .get("refresh_token")
                    .and_then(|t| st.refresh_tokens.get(t).cloned())
                else {
                    return Self::error(&st, 400, "invalid_grant", &form, basic.as_ref());
                };
                if owner != client_id || form.get("resource") != Some(&resource) {
                    return Self::error(&st, 400, "invalid_target", &form, basic.as_ref());
                }
                let rotate = st.cfg.rotate_refresh;
                let doc = self.issue(&mut st, &client_id, &resource, rotate);
                json_response(200, &doc)
            }
            _ => Self::error(&st, 400, "unsupported_grant_type", &form, basic.as_ref()),
        }
    }

    fn register(&self, body: &[u8]) -> Response {
        let mut st = self.lock();
        let Ok(request) = serde_json::from_slice::<Value>(body) else {
            return json_response(400, &json!({"error": "invalid_client_metadata"}));
        };
        let redirect_uris: Vec<String> = request["redirect_uris"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if redirect_uris.is_empty() {
            return json_response(400, &json!({"error": "invalid_redirect_uri"}));
        }
        st.counter += 1;
        let client_id = format!("dcr-client-{}", st.counter);
        let mut doc = json!({
            "client_id": client_id,
            "client_name": request["client_name"],
            "redirect_uris": redirect_uris,
            "token_endpoint_auth_method": "none",
        });
        let mut secret = None;
        if st.cfg.registration_secret {
            let value = format!(
                "{}dcr-secret-{}",
                st.cfg.sentinel.clone().unwrap_or_default(),
                st.counter
            );
            st.secrets_seen.push(value.clone());
            doc["client_secret"] = json!(value);
            doc["client_secret_expires_at"] = json!(0);
            doc["token_endpoint_auth_method"] = json!("client_secret_basic");
            secret = Some(value);
        }
        st.clients.insert(
            client_id,
            Client {
                redirect_uris,
                secret,
            },
        );
        json_response(201, &doc)
    }

    fn revoke(&self, body: &[u8]) -> Response {
        let form: HashMap<String, String> =
            url::form_urlencoded::parse(body).into_owned().collect();
        let mut st = self.lock();
        st.revocations += 1;
        if let Some(token) = form.get("token") {
            st.secrets_seen.push(token.clone());
        }
        status_only(st.cfg.revocation_status)
    }
}

fn urlencode(text: &str) -> String {
    url::form_urlencoded::byte_serialize(text.as_bytes()).collect()
}

async fn handle(
    State(shared): State<Arc<Shared>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    shared.record(&method, &uri, &headers, &body);
    let path = uri.path().to_owned();
    let is_metadata = path.starts_with("/.well-known/");
    if is_metadata {
        let (redirect, delay) = {
            let st = shared.lock();
            (st.cfg.redirect_metadata_to.clone(), st.cfg.metadata_delay)
        };
        if let Some(target) = redirect {
            return (StatusCode::FOUND, [(header::LOCATION, target)]).into_response();
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    match (method.as_str(), path.as_str()) {
        ("GET", "/.well-known/oauth-protected-resource/mcp") => shared.resource_metadata("/mcp"),
        ("GET", "/.well-known/oauth-protected-resource/other") => {
            shared.resource_metadata("/other")
        }
        ("GET", "/.well-known/oauth-protected-resource") => {
            let root = shared.lock().cfg.root_resource.clone();
            match root {
                Some(resource) => shared.resource_metadata_for(&resource),
                None => status_only(404),
            }
        }
        ("GET", "/.well-known/oauth-authorization-server") => {
            if shared.lock().cfg.oidc_only {
                status_only(404)
            } else {
                shared.server_metadata()
            }
        }
        ("GET", "/.well-known/openid-configuration") => shared.server_metadata(),
        ("GET", "/authorize") => shared.authorize(&uri),
        ("POST", "/token") => shared.token(&headers, &body).await,
        ("POST", "/register") => shared.register(&body),
        ("POST", "/revoke") => shared.revoke(&body),
        _ => status_only(404),
    }
}

/// The server, listening on a loopback address. Stops when dropped.
pub struct MockAs {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl MockAs {
    pub async fn start() -> MockAs {
        Self::start_on("127.0.0.1").await
    }

    pub async fn start_on(ip: &str) -> MockAs {
        let listener = tokio::net::TcpListener::bind((ip, 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Shared {
            default_origin: format!("http://{addr}"),
            state: Mutex::new(State_::default()),
        });
        let app = Router::new().fallback(handle).with_state(shared.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        MockAs { addr, shared, task }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn origin(&self) -> String {
        self.shared.origin()
    }

    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.shared.origin())
    }

    pub fn other_url(&self) -> String {
        format!("{}/other", self.shared.origin())
    }

    pub fn configure<R>(&self, f: impl FnOnce(&mut Cfg) -> R) -> R {
        f(&mut self.shared.lock().cfg)
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.shared.lock().seen.clone()
    }

    pub fn seen_on(&self, path: &str) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.path == path).collect()
    }

    pub fn requests(&self) -> AsRequests {
        let st = self.shared.lock();
        AsRequests {
            code_exchanges: st.code_exchanges,
            refreshes: st.refreshes,
            revocations: st.revocations,
        }
    }

    pub fn secrets_seen(&self) -> Vec<String> {
        self.shared.lock().secrets_seen.clone()
    }
}

impl Drop for MockAs {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A listener that counts the connections made to it and answers none.
///
/// On a machine where the address cannot be bound (the loopback range beyond `127.0.0.1` is not
/// configured everywhere), nothing listens and the count stays zero: the tests that use it still
/// show the refusal, and only lose the proof that nothing connected.
pub struct Tripwire {
    addr: SocketAddr,
    count: Arc<AtomicUsize>,
    task: Option<JoinHandle<()>>,
}

impl Tripwire {
    pub async fn start_on(ip: &str) -> Tripwire {
        let count = Arc::new(AtomicUsize::new(0));
        let Ok(listener) = tokio::net::TcpListener::bind((ip, 0)).await else {
            eprintln!("no listener on {ip}: the tripwire counts nothing here");
            return Tripwire {
                addr: SocketAddr::new(ip.parse().unwrap(), 9),
                count,
                task: None,
            };
        };
        let addr = listener.local_addr().unwrap();
        let counted = count.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                counted.fetch_add(1, Ordering::SeqCst);
                drop(socket);
            }
        });
        Tripwire {
            addr,
            count,
            task: Some(task),
        }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn connections(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

impl Drop for Tripwire {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

// ------------------------------------------------------------------------------- the fixture

static OWNERS: AtomicUsize = AtomicUsize::new(0);

/// A person no other test uses.
pub fn owner() -> UserId {
    UserId::new(&format!(
        "p{}@broker.test",
        OWNERS.fetch_add(1, Ordering::SeqCst)
    ))
}

/// Adds a server at `url` for `owner`, as the API would: the URL checked by `policy`.
pub async fn add_server_at(
    servers: &MemoryUserToolServers,
    policy: &EgressPolicy,
    owner: &UserId,
    auth: UserAuth,
    url: &str,
) -> (UserToolServer, ServerRevision) {
    let target = policy.check_url(url).expect("a URL the policy accepts");
    let definition = UserToolServer::new(UserToolServerId::generate(), "A server", &target, auth)
        .expect("a definition");
    let revision = servers.insert(owner, &definition).await.expect("insert");
    (definition, revision)
}

/// A secret store that gives up the thread after every call, so that two callers of one `join`
/// interleave at each call to the store the way two replicas do: both read before either writes.
#[derive(Clone)]
pub struct Interleaved<S>(pub S);

impl<S: SecretStore> SecretStore for Interleaved<S> {
    async fn put(
        &self,
        owner: &UserId,
        server: &SecretServer,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        let result = self.0.put(owner, server, material).await;
        tokio::task::yield_now().await;
        result
    }

    async fn get(
        &self,
        owner: &UserId,
        server: &SecretServer,
    ) -> Result<Option<(SecretVersion, SecretMaterial)>, SecretError> {
        let result = self.0.get(owner, server).await;
        tokio::task::yield_now().await;
        result
    }

    async fn delete(&self, owner: &UserId, server: &SecretServer) -> Result<(), SecretError> {
        let result = self.0.delete(owner, server).await;
        tokio::task::yield_now().await;
        result
    }

    async fn rotate(
        &self,
        owner: &UserId,
        server: &SecretServer,
        expected: SecretVersion,
        material: &SecretMaterial,
    ) -> Result<SecretVersion, SecretError> {
        let result = self.0.rotate(owner, server, expected, material).await;
        tokio::task::yield_now().await;
        result
    }
}

pub type TestBroker = Broker<Interleaved<MemorySecretStore>, MemoryUserToolServers, FixedClock>;

/// The policy of the tests: `http` is allowed and the loopback address of the mock is named, so the
/// guard is in the path and is told, explicitly, what to let through.
pub fn policy() -> EgressPolicy {
    EgressPolicy::new()
        .allowing_http()
        .with_allow("127.0.0.1".parse().unwrap())
}

pub fn start_time() -> Timestamp {
    Timestamp::from_second(1_790_000_000).unwrap()
}

/// A broker, its stores and an authorization server.
pub struct Fx {
    pub mock: MockAs,
    pub secrets: MemorySecretStore,
    pub servers: MemoryUserToolServers,
    pub clock: FixedClock,
    pub broker: TestBroker,
    pub config: BrokerConfig,
    browser: reqwest::Client,
}

pub fn broker_over(
    secrets: &MemorySecretStore,
    servers: &MemoryUserToolServers,
    clock: &FixedClock,
    egress: EgressPolicy,
    config: BrokerConfig,
) -> TestBroker {
    Broker::builder(Interleaved(secrets.clone()), servers.clone(), egress)
        .clock(clock.clone())
        .config(config)
        .build()
        .unwrap()
}

impl Fx {
    /// A server of the mock for `owner`, with the broker's own policy.
    pub async fn add(&self, owner: &UserId, auth: UserAuth) -> UserToolServer {
        add_server_at(&self.servers, &policy(), owner, auth, &self.mock.mcp_url())
            .await
            .0
    }

    pub async fn new() -> Fx {
        Self::with(BrokerConfig::default()).await
    }

    pub async fn with(config: BrokerConfig) -> Fx {
        let mock = MockAs::start().await;
        let (secrets, servers) = (MemorySecretStore::new(), MemoryUserToolServers::new());
        let clock = FixedClock::new(start_time());
        let broker = broker_over(&secrets, &servers, &clock, policy(), config.clone());
        let browser = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap();
        Fx {
            mock,
            secrets,
            servers,
            clock,
            broker,
            config,
            browser,
        }
    }
}

impl BrokerFixture for Fx {
    type Broker = TestBroker;
    type Servers = MemoryUserToolServers;
    type Secrets = MemorySecretStore;

    fn broker(&self) -> &TestBroker {
        &self.broker
    }

    fn replica(&self) -> TestBroker {
        broker_over(
            &self.secrets,
            &self.servers,
            &self.clock,
            policy(),
            self.config.clone(),
        )
    }

    fn servers(&self) -> &MemoryUserToolServers {
        &self.servers
    }

    fn secrets(&self) -> &MemorySecretStore {
        &self.secrets
    }

    fn egress(&self) -> EgressPolicy {
        policy()
    }

    fn now(&self) -> Timestamp {
        use orch_ports::Clock;
        self.clock.now()
    }

    fn advance(&self, by: Duration) {
        self.clock.advance(by);
    }

    fn server_url(&self) -> String {
        self.mock.mcp_url()
    }

    fn other_server_url(&self) -> String {
        self.mock.other_url()
    }

    fn redirect_uri(&self) -> RedirectUri {
        RedirectUri::parse("https://orchestrator.test/api/user-tool-servers/oauth/callback")
            .unwrap()
    }

    async fn consent(&self, authorize_url: &str) -> Result<Consent, String> {
        let response = self
            .browser
            .get(authorize_url)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if response.status() != reqwest::StatusCode::FOUND {
            let status = response.status();
            let why = response.text().await.unwrap_or_default();
            return Err(format!("{status}: {why}"));
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or("no Location")?
            .to_owned();
        let url = url::Url::parse(&location).map_err(|e| e.to_string())?;
        let pairs: HashMap<String, String> = url.query_pairs().into_owned().collect();
        Ok(Consent {
            code: ToolSecret::new(pairs.get("code").cloned().ok_or("no code")?),
            state: pairs.get("state").cloned().ok_or("no state")?,
        })
    }

    fn access_ttl(&self) -> Duration {
        Duration::from_secs(self.mock.configure(|c| c.ttl_secs))
    }

    fn set_pkce_s256(&self, offered: bool) {
        self.mock.configure(|c| c.s256 = offered);
    }

    fn set_refresh_outcome(&self, outcome: RefreshOutcome) {
        self.mock.configure(|c| c.refresh = outcome);
    }

    fn set_token_delay(&self, delay: Duration) {
        self.mock.configure(|c| c.token_delay = delay);
    }

    fn requests(&self) -> AsRequests {
        self.mock.requests()
    }

    fn plant(&self, sentinel: &str) {
        self.mock
            .configure(|c| c.sentinel = Some(sentinel.to_owned()));
    }

    fn secrets_seen(&self) -> Vec<String> {
        self.mock.secrets_seen()
    }
}

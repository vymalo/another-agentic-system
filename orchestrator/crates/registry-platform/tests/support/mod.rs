//! An in-process registry: a real HTTP server that serves a linkset the tests change, answers
//! conditional requests, can be taken down, delayed or made to say something odd, and keeps a
//! journal of every request it saw.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use orch_core::{AgentId, AgentSource};
use orch_ports::{AgentEndpoint, AgentTransport, RegistryEntry};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

pub const PROFILE: &str = "https://agents.vymalo.com/registry/v1";
pub const PATH: &str = "/registry/v1/agents";

/// What the server does with a request, besides serving the document.
#[derive(Clone)]
pub enum Mode {
    Serve,
    /// A status and no document.
    Status(u16),
    /// A redirect.
    Redirect,
    /// This body with this content type, as it is.
    Raw {
        content_type: String,
        body: Vec<u8>,
    },
    /// Never answers.
    Hang,
}

pub struct Script {
    pub items: Vec<Value>,
    pub mode: Mode,
    pub cache_control: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub age: Option<String>,
    pub content_type: String,
    /// The bearer token the server demands, if any.
    pub token: Option<String>,
    pub delay: Duration,
}

/// What one request looked like.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    pub authorization: Option<String>,
    pub accept: Option<String>,
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
}

pub struct Registry {
    pub script: Mutex<Script>,
    pub journal: Mutex<Vec<Seen>>,
    pub hits: AtomicUsize,
}

pub struct Server {
    pub registry: Arc<Registry>,
    pub url: String,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn header_of(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
}

async fn serve(State(registry): State<Arc<Registry>>, headers: HeaderMap) -> Response {
    registry.hits.fetch_add(1, Ordering::SeqCst);
    let seen = Seen {
        authorization: header_of(&headers, header::AUTHORIZATION),
        accept: header_of(&headers, header::ACCEPT),
        if_none_match: header_of(&headers, header::IF_NONE_MATCH),
        if_modified_since: header_of(&headers, header::IF_MODIFIED_SINCE),
    };
    registry.journal.lock().unwrap().push(seen.clone());
    let (mode, delay) = {
        let script = registry.script.lock().unwrap();
        (script.mode.clone(), script.delay)
    };
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    match answer(&registry, &mode, &seen) {
        Some(response) => response,
        None => {
            std::future::pending::<()>().await;
            unreachable!()
        }
    }
}

/// The response for `mode`, or `None` to never answer. No `.await` while the script is locked.
fn answer(registry: &Registry, mode: &Mode, seen: &Seen) -> Option<Response> {
    let script = registry.script.lock().unwrap();
    if let Some(token) = &script.token
        && seen.authorization.as_deref() != Some(format!("Bearer {token}").as_str())
    {
        return Some((StatusCode::UNAUTHORIZED, "no").into_response());
    }
    Some(match mode {
        Mode::Hang => return None,
        Mode::Status(code) => StatusCode::from_u16(*code).unwrap().into_response(),
        Mode::Redirect => (
            StatusCode::FOUND,
            [(header::LOCATION, "http://127.0.0.1:1/elsewhere")],
        )
            .into_response(),
        Mode::Raw { content_type, body } => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, content_type.clone())],
            body.clone(),
        )
            .into_response(),
        Mode::Serve => {
            let mut response_headers: Vec<(HeaderName, String)> = Vec::new();
            if let Some(v) = &script.cache_control {
                response_headers.push((header::CACHE_CONTROL, v.clone()));
            }
            if let Some(v) = &script.age {
                response_headers.push((header::AGE, v.clone()));
            }
            if let Some(v) = &script.etag {
                response_headers.push((header::ETAG, v.clone()));
            }
            if let Some(v) = &script.last_modified {
                response_headers.push((header::LAST_MODIFIED, v.clone()));
            }
            let unchanged =
                match (&script.etag, &seen.if_none_match) {
                    (Some(etag), Some(asked)) => etag == asked,
                    _ => false,
                } || match (&script.last_modified, &seen.if_modified_since, &script.etag) {
                    (Some(date), Some(asked), None) => date == asked,
                    _ => false,
                };
            let mut response = if unchanged {
                StatusCode::NOT_MODIFIED.into_response()
            } else {
                let body = json!({"linkset": [{
                    "anchor": "https://platform.example.com/registry/v1/agents",
                    "profile": [{"href": PROFILE}],
                    "item": script.items,
                }]});
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, script.content_type.clone())],
                    body.to_string(),
                )
                    .into_response()
            };
            for (name, value) in response_headers {
                response
                    .headers_mut()
                    .insert(name, HeaderValue::from_str(&value).unwrap());
            }
            response
        }
    })
}

impl Server {
    pub async fn start() -> Server {
        let registry = Arc::new(Registry {
            script: Mutex::new(Script {
                items: Vec::new(),
                mode: Mode::Serve,
                cache_control: Some("no-store".to_owned()),
                etag: None,
                last_modified: None,
                age: None,
                content_type:
                    "application/linkset+json; profile=\"https://www.rfc-editor.org/info/rfc9727\""
                        .to_owned(),
                token: None,
                delay: Duration::ZERO,
            }),
            journal: Mutex::new(Vec::new()),
            hits: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}{PATH}", listener.local_addr().unwrap());
        let router = Router::new()
            .route(PATH, get(serve))
            .with_state(Arc::clone(&registry));
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Server {
            registry,
            url,
            task,
        }
    }

    pub fn script<T>(&self, f: impl FnOnce(&mut Script) -> T) -> T {
        f(&mut self.registry.script.lock().unwrap())
    }

    pub fn hits(&self) -> usize {
        self.registry.hits.load(Ordering::SeqCst)
    }

    pub fn journal(&self) -> Vec<Seen> {
        self.registry.journal.lock().unwrap().clone()
    }

    pub fn push(&self, item: Value) {
        self.script(|s| s.items.push(item));
    }

    pub fn set_items(&self, items: Vec<Value>) {
        self.script(|s| s.items = items);
    }
}

/// The item a registry lists for the agent `id`.
pub fn item(id: &str, title: &str) -> Value {
    json!({"href": format!("http://{id}.test/.well-known/agent-card.json"),
           "type": "application/json", "title": title, "service": [id]})
}

/// The item that stands for an entry made by the testkit.
pub fn item_of(entry: &RegistryEntry) -> Value {
    let AgentTransport::A2a { card_url, .. } = &entry.endpoint.transport else {
        panic!("the testkit makes A2A entries")
    };
    let mut item = json!({"href": card_url, "type": "application/json",
                          "title": entry.name, "service": [entry.endpoint.id.as_str()]});
    if !entry.tags.is_empty() {
        item["tags"] = json!(entry.tags);
    }
    item
}

pub fn ids(entries: &[RegistryEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.endpoint.id.as_str()).collect()
}

pub fn agent_id(id: &str) -> AgentId {
    AgentId::new(id)
}

pub fn registry_origin(entry: &RegistryEntry) -> bool {
    entry.origin == AgentSource::Registry
}

pub fn endpoint_of(entry: &RegistryEntry) -> &AgentEndpoint {
    &entry.endpoint
}

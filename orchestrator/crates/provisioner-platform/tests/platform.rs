//! The adapter against an in-process stand-in of the Platform API's owner routes, written from the
//! platform's OpenAPI document (`docs/api/platform-api-v1.openapi.json`): bearer required, `ETag`
//! on a read, `If-None-Match: *` on a create (`201`, or `409` for the second), `If-Match` on a
//! replace and a delete (`412` when stale, `428` when absent), `404` for an agent of another owner
//! and `422` for a form that names another owner. The stand-in keeps state, which a WireMock
//! stub cannot, and the conformance cases need it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use orch_ports::testkit::provisioner::ProvisionerFixture;
use orch_ports::{AgentKind, AgentOwner, AgentProvisioner, AgentState, ProvisionError};
use orch_provisioner_platform::{PlatformConfig, PlatformProvisioner};
use secrecy::SecretString;
use serde_json::{Value, json};

const TOKEN: &str = "orchestrator-token";

struct Agent {
    owner: String,
    uid: u64,
    generation: u64,
    suspend: bool,
    form: Value,
    ready_at: Instant,
    deleted_at: Option<Instant>,
}

impl Agent {
    fn state(&self) -> &'static str {
        if self.deleted_at.is_some() {
            "deleting"
        } else if self.suspend {
            "suspended"
        } else if Instant::now() >= self.ready_at {
            "ready"
        } else {
            "saving"
        }
    }
    fn etag(&self) -> String {
        format!("\"{}-{}\"", self.uid, self.generation)
    }
}

#[derive(Default)]
struct Platform {
    agents: Mutex<HashMap<String, Agent>>,
    delay_ms: AtomicU64,
    next_uid: AtomicU64,
    requests: Mutex<Vec<String>>,
}

type Shared = Arc<Platform>;

fn problem(status: StatusCode, kind: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/problem+json")],
        json!({"type": format!("urn:vymalo:agents:problem:{kind}"), "title": kind, "status": status.as_u16()}).to_string(),
    )
        .into_response()
}

fn authorised(headers: &HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"))
}

fn detail(name: &str, agent: &Agent) -> Response {
    (
        StatusCode::OK,
        [(header::ETAG, agent.etag())],
        json!({"name": name, "state": agent.state(), "suspend": agent.suspend}).to_string(),
    )
        .into_response()
}

async fn read(
    State(p): State<Shared>,
    Path((owner, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    p.requests
        .lock()
        .unwrap()
        .push(format!("GET {owner}/{name}"));
    if !authorised(&headers) {
        return problem(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    match p.agents.lock().unwrap().get(&name) {
        Some(a) if a.owner == owner => detail(&name, a),
        _ => problem(StatusCode::NOT_FOUND, "not-found"),
    }
}

async fn write(
    State(p): State<Shared>,
    Path((owner, name)): Path<(String, String)>,
    headers: HeaderMap,
    body: String,
) -> Response {
    p.requests
        .lock()
        .unwrap()
        .push(format!("PUT {owner}/{name}"));
    if !authorised(&headers) {
        return problem(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Ok(form) = serde_json::from_str::<Value>(&body) else {
        return problem(StatusCode::BAD_REQUEST, "invalid-request");
    };
    if form.get("name").and_then(Value::as_str) != Some(&name) {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "invalid");
    }
    if let Some(o) = form.get("owner").filter(|o| !o.is_null())
        && o.as_str() != Some(&owner)
    {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "invalid");
    }
    let suspend = form
        .get("suspend")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let delay = Duration::from_millis(p.delay_ms.load(Ordering::SeqCst));
    let mut agents = p.agents.lock().unwrap();
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    let if_none = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    match (if_match, if_none) {
        (None, Some("*")) => {
            if agents.contains_key(&name) {
                return problem(StatusCode::CONFLICT, "already-exists");
            }
            let uid = p.next_uid.fetch_add(1, Ordering::SeqCst);
            let agent = Agent {
                owner,
                uid,
                generation: 1,
                suspend,
                form,
                ready_at: Instant::now() + delay,
                deleted_at: None,
            };
            let etag = agent.etag();
            agents.insert(name, agent);
            (StatusCode::CREATED, [(header::ETAG, etag)], "{}").into_response()
        }
        (Some(tag), None) => match agents.get_mut(&name) {
            Some(a) if a.owner == owner => {
                if a.etag() != tag {
                    return problem(StatusCode::PRECONDITION_FAILED, "stale");
                }
                if a.suspend && !suspend {
                    a.ready_at = Instant::now() + delay;
                }
                a.suspend = suspend;
                a.form = form;
                a.generation += 1;
                (StatusCode::OK, [(header::ETAG, a.etag())], "{}").into_response()
            }
            _ => problem(StatusCode::NOT_FOUND, "not-found"),
        },
        _ => problem(StatusCode::PRECONDITION_REQUIRED, "precondition-required"),
    }
}

async fn remove(
    State(p): State<Shared>,
    Path((owner, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    p.requests
        .lock()
        .unwrap()
        .push(format!("DELETE {owner}/{name}"));
    if !authorised(&headers) {
        return problem(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    let Some(tag) = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok()) else {
        return problem(StatusCode::PRECONDITION_REQUIRED, "precondition-required");
    };
    let mut agents = p.agents.lock().unwrap();
    match agents.get(&name) {
        Some(a) if a.owner == owner => {
            if a.etag() != tag {
                return problem(StatusCode::PRECONDITION_FAILED, "stale");
            }
            agents.remove(&name);
            StatusCode::NO_CONTENT.into_response()
        }
        _ => problem(StatusCode::NOT_FOUND, "not-found"),
    }
}

struct Fixture {
    provisioner: PlatformProvisioner,
    platform: Shared,
}

async fn serve() -> (String, Shared) {
    let platform = Shared::default();
    let app = Router::new()
        .route(
            "/v1/owners/{owner}/agents/{name}",
            get(read).put(write).delete(remove),
        )
        .with_state(platform.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, platform)
}

fn config(url: &str, token: &str) -> PlatformConfig {
    PlatformConfig::new(url, SecretString::from(token.to_owned()))
        .with_form(
            AgentKind::Coder,
            json!({"agent": {"kind": "coder"}, "model": {"endpoint": "e", "model": "m"}}),
        )
        .with_form(
            AgentKind::Workspace,
            json!({"agent": {"kind": "folder"}, "model": {"endpoint": "e", "model": "m"}}),
        )
        .with_poll_interval(Duration::from_millis(20))
        .with_ready_timeout(Duration::from_secs(10))
        .without_system_proxy()
}

impl ProvisionerFixture for Fixture {
    type Provisioner = PlatformProvisioner;
    fn provisioner(&self) -> &PlatformProvisioner {
        &self.provisioner
    }
    fn agent_count(&self) -> usize {
        self.platform.agents.lock().unwrap().len()
    }
    fn set_ready_delay(&self, delay: Duration) {
        self.platform
            .delay_ms
            .store(u64::try_from(delay.as_millis()).unwrap(), Ordering::SeqCst);
    }
}

async fn make() -> Option<Fixture> {
    let (url, platform) = serve().await;
    let provisioner = PlatformProvisioner::new(config(&url, TOKEN)).unwrap();
    Some(Fixture {
        provisioner,
        platform,
    })
}

orch_ports::agent_provisioner_conformance!(make);

fn owner(key: &str) -> AgentOwner {
    AgentOwner::new(key).unwrap()
}

#[tokio::test]
async fn a_wrong_token_is_refused_and_makes_nothing() {
    let (url, platform) = serve().await;
    let p = PlatformProvisioner::new(config(&url, "wrong")).unwrap();
    let err = p
        .ensure_ready(&owner("a"), AgentKind::Coder)
        .await
        .unwrap_err();
    assert!(matches!(err, ProvisionError::Refused), "{err:?}");
    assert!(platform.agents.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_agent_of_another_owner_with_the_same_name_is_never_touched() {
    let (url, platform) = serve().await;
    platform.agents.lock().unwrap().insert(
        "coder-x".into(),
        Agent {
            owner: "mallory".into(),
            uid: 99,
            generation: 1,
            suspend: true,
            form: json!({}),
            ready_at: Instant::now(),
            deleted_at: None,
        },
    );
    let p = PlatformProvisioner::new(config(&url, TOKEN)).unwrap();
    let err = p
        .ensure_ready(&owner("x"), AgentKind::Coder)
        .await
        .unwrap_err();
    assert!(matches!(err, ProvisionError::Invalid), "{err:?}");
    p.suspend(&owner("x"), AgentKind::Coder).await.unwrap();
    p.delete(&owner("x"), AgentKind::Coder).await.unwrap();
    let agents = platform.agents.lock().unwrap();
    let theirs = &agents["coder-x"];
    assert!(theirs.suspend, "still suspended");
    assert_eq!(theirs.generation, 1, "never written");
}

#[tokio::test]
async fn the_platform_gets_the_form_of_the_kind_with_the_owner_and_no_suspend() {
    let (url, platform) = serve().await;
    let p = PlatformProvisioner::new(config(&url, TOKEN)).unwrap();
    p.ensure_ready(&owner("zed"), AgentKind::Workspace)
        .await
        .unwrap();
    let agents = platform.agents.lock().unwrap();
    let form = &agents["workspace-zed"].form;
    assert_eq!(form["owner"], "zed");
    assert_eq!(form["name"], "workspace-zed");
    assert_eq!(form["suspend"], false);
    assert_eq!(form["agent"]["kind"], "folder");
}

#[tokio::test]
async fn an_agent_that_never_becomes_ready_gives_not_ready() {
    let (url, platform) = serve().await;
    platform.delay_ms.store(60_000, Ordering::SeqCst);
    let p = PlatformProvisioner::new(
        config(&url, TOKEN).with_ready_timeout(Duration::from_millis(200)),
    )
    .unwrap();
    let err = p
        .ensure_ready(&owner("slow"), AgentKind::Coder)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ProvisionError::NotReady {
                last: AgentState::Saving
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn the_deployment_must_give_a_form_for_the_kind() {
    let (url, _) = serve().await;
    let cfg =
        PlatformConfig::new(&url, SecretString::from(TOKEN.to_owned())).without_system_proxy();
    let p = PlatformProvisioner::new(cfg).unwrap();
    let err = p
        .ensure_ready(&owner("a"), AgentKind::Coder)
        .await
        .unwrap_err();
    assert!(matches!(err, ProvisionError::Invalid), "{err:?}");
}

#[test]
fn a_form_that_names_what_the_adapter_owns_is_refused_at_build() {
    let cfg =
        config("http://localhost:1", TOKEN).with_form(AgentKind::Coder, json!({"owner": "x"}));
    assert!(PlatformProvisioner::new(cfg).is_err());
    assert!(PlatformProvisioner::new(config("ftp://x", TOKEN)).is_err());
    assert!(PlatformProvisioner::new(config("http://u:p@x", TOKEN)).is_err());
}

#[tokio::test]
async fn an_unreachable_platform_is_unavailable() {
    let p = PlatformProvisioner::new(config("http://127.0.0.1:1", TOKEN)).unwrap();
    let err = p.state(&owner("a"), AgentKind::Coder).await.unwrap_err();
    assert!(matches!(err, ProvisionError::Unavailable { .. }), "{err:?}");
}

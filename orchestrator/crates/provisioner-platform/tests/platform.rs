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
use orch_core::AccountId;
use orch_ports::testkit::provisioner::ProvisionerFixture;
use orch_ports::{
    AgentKind, AgentOwner, AgentProvisioner, AgentState, BlockedReason, ProvisionError,
};
use orch_provisioner_platform::{BuildError, PlatformConfig, PlatformProvisioner};
use secrecy::SecretString;
use serde_json::{Value, json};

const TOKEN: &str = "orchestrator-token";

/// The account of the person in these tests: the shape of an identity provider's `sub`.
const ACCOUNT: &str = "3b1d7e52-6a40-4c18-9f0e-a1c2d3e4f501";

fn account() -> AccountId {
    AccountId::parse(ACCOUNT).unwrap()
}

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
    /// When set, a create answers `409` with this problem slug whatever the agents are.
    conflict: Mutex<Option<&'static str>>,
    /// When set, a read answers this state and these `conditions` (the shape of the platform's
    /// detail: `{type, status, reason}`) for every agent that exists.
    forced_state: Mutex<Option<(&'static str, Vec<Value>)>>,
    /// When set, a write answers this status with a problem of this slug that **quotes the whole
    /// request body** (a platform that echoes what it was sent, the account included).
    echo: Mutex<Option<(StatusCode, &'static str)>>,
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

fn detail(name: &str, agent: &Agent, forced: Option<&(&'static str, Vec<Value>)>) -> Response {
    let (state, conditions) = match forced {
        Some((state, conditions)) => (*state, conditions.clone()),
        None => (agent.state(), Vec::new()),
    };
    (
        StatusCode::OK,
        [(header::ETAG, agent.etag())],
        json!({"name": name, "state": state, "conditions": conditions, "suspend": agent.suspend})
            .to_string(),
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
    let forced = p.forced_state.lock().unwrap().clone();
    match p.agents.lock().unwrap().get(&name) {
        Some(a) if a.owner == owner => detail(&name, a, forced.as_ref()),
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
    if let Some((status, slug)) = *p.echo.lock().unwrap() {
        return (
            status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            json!({"type": format!("urn:vymalo:agents:problem:{slug}"), "title": body, "detail": body, "status": status.as_u16()}).to_string(),
        )
            .into_response();
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
            if let Some(slug) = *p.conflict.lock().unwrap() {
                return problem(StatusCode::CONFLICT, slug);
            }
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
        .ensure_ready(&owner("a"), AgentKind::Coder, &account())
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
        .ensure_ready(&owner("x"), AgentKind::Coder, &account())
        .await
        .unwrap_err();
    assert!(matches!(err, ProvisionError::Invalid), "{err:?}");
    p.suspend(&owner("x"), AgentKind::Coder, &account())
        .await
        .unwrap();
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
    p.ensure_ready(&owner("zed"), AgentKind::Workspace, &account())
        .await
        .unwrap();
    let agents = platform.agents.lock().unwrap();
    let form = &agents["workspace-zed"].form;
    assert_eq!(form["owner"], "zed");
    assert_eq!(form["name"], "workspace-zed");
    assert_eq!(form["account"], ACCOUNT);
    assert_eq!(form["suspend"], false);
    assert_eq!(form["agent"]["kind"], "folder");
}

#[tokio::test]
async fn every_replace_carries_the_persons_account_beside_name_owner_and_suspend() {
    let (url, platform) = serve().await;
    let p = PlatformProvisioner::new(config(&url, TOKEN)).unwrap();
    p.ensure_ready(&owner("yan"), AgentKind::Coder, &account())
        .await
        .unwrap();
    // The stand-in keeps the last form written. It is emptied before each replace, so what is
    // read after it is what that replace sent and nothing of the create.
    let forget = || {
        platform
            .agents
            .lock()
            .unwrap()
            .get_mut("coder-yan")
            .unwrap()
            .form = json!({});
    };
    let sent = || platform.agents.lock().unwrap()["coder-yan"].form.clone();
    let check = |form: Value, suspend: bool| {
        assert_eq!(form["account"], ACCOUNT, "{suspend}");
        assert_eq!(form["name"], "coder-yan");
        assert_eq!(form["owner"], "yan");
        assert_eq!(form["suspend"], suspend);
    };

    forget();
    p.suspend(&owner("yan"), AgentKind::Coder, &account())
        .await
        .unwrap();
    check(sent(), true);

    forget();
    p.ensure_ready(&owner("yan"), AgentKind::Coder, &account())
        .await
        .unwrap();
    check(sent(), false);
}

#[test]
fn a_configured_form_that_carries_an_account_is_refused_at_build() {
    // a form of the deployment's cannot say who is charged: every person would be charged to it
    let cfg = config("http://localhost:1", TOKEN).with_form(
        AgentKind::Coder,
        json!({"agent": {"kind": "coder"}, "account": ACCOUNT}),
    );
    let err = PlatformProvisioner::new(cfg).unwrap_err();
    assert!(
        matches!(err, BuildError::BadForm(AgentKind::Coder)),
        "{err:?}"
    );
    assert!(!format!("{err} {err:?}").contains(ACCOUNT), "{err:?}");
}

/// No error, `Display` or `Debug`, its causes included, holds the account, whatever the platform
/// answers, even when it quotes the request that carried it. The crate has no logging dependency,
/// so it writes no log line: errors and `Debug` are all that can leave it.
#[tokio::test]
async fn no_error_or_debug_text_of_the_adapter_holds_the_account() {
    let clean = |what: &str, text: String| {
        assert!(!text.contains(ACCOUNT), "{what}: {text}");
    };
    let shown = |e: &ProvisionError| format!("{} {e:?}", orch_core::report(e));
    for (status, slug) in [
        (StatusCode::UNPROCESSABLE_ENTITY, "invalid"),
        (StatusCode::BAD_REQUEST, "invalid-request"),
        (StatusCode::FORBIDDEN, "forbidden"),
        (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        (StatusCode::CONFLICT, "managed-by-gitops"),
        (StatusCode::CONFLICT, "deleting"),
        (StatusCode::CONFLICT, "conflict"),
    ] {
        // a create
        let (url, platform) = serve().await;
        *platform.echo.lock().unwrap() = Some((status, slug));
        let p = PlatformProvisioner::new(config(&url, TOKEN)).unwrap();
        let err = p
            .ensure_ready(&owner("quiet"), AgentKind::Coder, &account())
            .await
            .unwrap_err();
        assert_eq!(put_count(&platform).await, 1, "{status}: the form was sent");
        clean(slug, shown(&err));
        clean(slug, shown(&err.duplicate()));
        clean(slug, format!("{p:?}"));

        // a replace
        *platform.echo.lock().unwrap() = None;
        p.ensure_ready(&owner("quiet"), AgentKind::Coder, &account())
            .await
            .unwrap();
        *platform.echo.lock().unwrap() = Some((status, slug));
        // a `409` on a replace is read again until the races run out; every status errs
        let err = p
            .suspend(&owner("quiet"), AgentKind::Coder, &account())
            .await
            .unwrap_err();
        clean(slug, shown(&err));
    }

    // a wait that runs out, after a create that carried the account
    let (url, platform) = serve().await;
    platform.delay_ms.store(60_000, Ordering::SeqCst);
    let cfg = config(&url, TOKEN).with_ready_timeout(Duration::from_millis(100));
    clean("config", format!("{cfg:?}"));
    let p = PlatformProvisioner::new(cfg).unwrap();
    let err = p
        .ensure_ready(&owner("slow"), AgentKind::Coder, &account())
        .await
        .unwrap_err();
    clean("not ready", shown(&err));
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
        .ensure_ready(&owner("slow"), AgentKind::Coder, &account())
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

/// A condition as the platform writes it.
fn condition(kind: &str, status: &str, reason: &str) -> Value {
    json!({"type": kind, "status": status, "reason": reason})
}

/// The platform reads every agent as `state`, with `conditions`, from the first poll.
async fn platform_reading(
    state: &'static str,
    conditions: Vec<Value>,
    timeout: Duration,
) -> (PlatformProvisioner, Shared) {
    let (url, platform) = serve().await;
    *platform.forced_state.lock().unwrap() = Some((state, conditions));
    let p = PlatformProvisioner::new(config(&url, TOKEN).with_ready_timeout(timeout)).unwrap();
    (p, platform)
}

fn gets(platform: &Shared) -> usize {
    platform
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("GET "))
        .count()
}

/// After `after`, the platform reads the agent as it really is (ready).
fn clear_after(platform: &Shared, after: Duration) {
    let platform = platform.clone();
    tokio::spawn(async move {
        tokio::time::sleep(after).await;
        *platform.forced_state.lock().unwrap() = None;
    });
}

#[tokio::test]
async fn a_blocked_agent_whose_store_is_not_ready_is_waited_for_until_it_is_ready() {
    // A fresh database cluster blocks the agent (`StoreReady` False, `ClusterNotReady`) and the
    // agent then goes ready: the first message of a person must not be refused. The earlier
    // "Blocked is refused at once" returned `Err` at the first poll and fails this test.
    let conditions = vec![condition("StoreReady", "False", "ClusterNotReady")];
    let (p, platform) = platform_reading("blocked", conditions, Duration::from_secs(10)).await;
    clear_after(&platform, Duration::from_millis(300));
    let agent = p
        .ensure_ready(&owner("fresh"), AgentKind::Coder, &account())
        .await
        .unwrap();
    assert_eq!(agent.state, AgentState::Ready);
    assert!(gets(&platform) > 2, "it polled while blocked");
}

async fn refused_at_once(conditions: Vec<Value>, reason: BlockedReason) {
    // The bound is 10 s; a reason that waiting cannot clear must not hold the request for it.
    let (p, platform) = platform_reading("blocked", conditions, Duration::from_secs(10)).await;
    let started = Instant::now();
    let err = p
        .ensure_ready(&owner("blocked"), AgentKind::Coder, &account())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ProvisionError::Blocked { reason: r } if *r == reason),
        "{err:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "waited {:?}",
        started.elapsed()
    );
    // One read after the create: no polling.
    assert_eq!(gets(&platform), 2);
}

#[tokio::test]
async fn a_blocked_agent_with_a_name_conflict_is_refused_at_once() {
    refused_at_once(
        vec![
            condition("ConfigResolved", "True", "Resolved"),
            condition("RuntimeReady", "False", "NameConflict"),
        ],
        BlockedReason::NameConflict,
    )
    .await;
}

#[tokio::test]
async fn a_blocked_agent_whose_database_operator_is_absent_is_refused_at_once() {
    refused_at_once(
        vec![condition("StoreReady", "False", "CNPGNotInstalled")],
        BlockedReason::StoreOperatorMissing,
    )
    .await;
}

#[tokio::test]
async fn a_blocked_agent_whose_condition_is_not_false_or_not_of_the_right_type_is_waited_for() {
    // The reason alone is not enough: `RuntimeReady` `True` with `NameConflict` (the status check)
    // and `Ready` `False` with `NameConflict` (the type check) are not the terminal condition.
    let conditions = vec![
        condition("RuntimeReady", "True", "NameConflict"),
        condition("Ready", "False", "NameConflict"),
    ];
    let (p, _platform) = platform_reading("blocked", conditions, Duration::from_millis(200)).await;
    let err = p
        .ensure_ready(&owner("lookalike"), AgentKind::Coder, &account())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ProvisionError::NotReady {
                last: AgentState::Blocked
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_blocked_agent_with_an_invalid_config_is_still_waited_for() {
    // `ConfigInvalid` also reports a referent that is not made yet, so it ends at the bound.
    let conditions = vec![condition("ConfigResolved", "False", "ConfigInvalid")];
    let (p, _platform) = platform_reading("blocked", conditions, Duration::from_millis(200)).await;
    let err = p
        .ensure_ready(&owner("invalid"), AgentKind::Coder, &account())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ProvisionError::NotReady {
                last: AgentState::Blocked
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_degraded_agent_is_waited_for_until_it_is_ready() {
    // The platform's lifecycle: `Degraded` is "applied, not ready" (a rollout, a crash loop) and
    // goes back to `Ready` by itself, so the wait continues.
    let (p, platform) = platform_reading("degraded", vec![], Duration::from_secs(10)).await;
    clear_after(&platform, Duration::from_millis(300));
    let agent = p
        .ensure_ready(&owner("degraded"), AgentKind::Coder, &account())
        .await
        .unwrap();
    assert_eq!(agent.state, AgentState::Ready);
    assert!(gets(&platform) > 2, "it polled while degraded");
}

#[tokio::test]
async fn a_degraded_agent_that_stays_so_gives_not_ready_at_the_bound() {
    let (p, _platform) = platform_reading("degraded", vec![], Duration::from_millis(200)).await;
    let err = p
        .ensure_ready(&owner("stuck"), AgentKind::Coder, &account())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ProvisionError::NotReady {
                last: AgentState::Degraded
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
        .ensure_ready(&owner("a"), AgentKind::Coder, &account())
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

async fn put_count(platform: &Shared) -> usize {
    platform
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.starts_with("PUT"))
        .count()
}

#[tokio::test]
async fn a_409_that_is_not_already_exists_is_not_made() {
    for (slug, expect) in [
        ("managed-by-gitops", "invalid"),
        ("deleting", "deleting"),
        ("conflict", "unavailable"),
        ("something-new", "unavailable"),
    ] {
        let (url, platform) = serve().await;
        *platform.conflict.lock().unwrap() = Some(slug);
        let p = PlatformProvisioner::new(config(&url, TOKEN)).unwrap();
        let err = p
            .ensure_ready(&owner("gita"), AgentKind::Coder, &account())
            .await
            .unwrap_err();
        let got = match err {
            ProvisionError::Invalid => "invalid",
            ProvisionError::Deleting => "deleting",
            ProvisionError::Unavailable { .. } => "unavailable",
            ref other => panic!("{slug}: unexpected {other:?}"),
        };
        assert_eq!(got, expect, "{slug}");
        assert_eq!(put_count(&platform).await, 1, "{slug}: no second create");
        assert!(platform.agents.lock().unwrap().is_empty(), "{slug}");
    }
}

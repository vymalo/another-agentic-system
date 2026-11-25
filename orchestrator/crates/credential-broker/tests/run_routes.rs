//! The broker's internal run routes (ADR 0084 decisions 1, 2, 5 and 6): who may bind, unbind and
//! lease, and the ruled refusals. The broker under the routes is the real [`RunLeaseBroker`] over
//! the memory stores, so the cap, the idempotence and the lease bound are the production ones.

#![allow(clippy::expect_used, reason = "a test that cannot set up should panic")]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use jiff::Timestamp;
use orch_core::AgentId;
use orch_credential_broker::{RunLeaseBroker, SharedTokens, router};
use orch_ports::memory::{MemoryModelCredentials, MemoryRunBindingStore};
use orch_ports::{RunId, RunLeases, ToolSecret};
use serde_json::{Value, json};
use tower::ServiceExt;

const ORCHESTRATOR: &str = "orchestrator-token";
const INJECTOR: &str = "injector-token";
const CODER: &str = "coder-token";
const CHAT: &str = "chat-token";
const MODEL: &str = "model-x";

fn tokens() -> SharedTokens {
    SharedTokens::new(
        ORCHESTRATOR,
        [
            (CODER.to_owned(), AgentId("coder".to_owned())),
            (CHAT.to_owned(), AgentId("chat".to_owned())),
        ],
    )
    .with_injector(INJECTOR)
}

fn broker() -> Arc<RunLeaseBroker<MemoryRunBindingStore, MemoryModelCredentials>> {
    let credentials = MemoryModelCredentials::new(BTreeMap::from([(
        MODEL.to_owned(),
        ToolSecret::new("model-bearer"),
    )]));
    Arc::new(RunLeaseBroker::new(
        MemoryRunBindingStore::default(),
        credentials,
    ))
}

fn app() -> (
    Router,
    Arc<RunLeaseBroker<MemoryRunBindingStore, MemoryModelCredentials>>,
) {
    let leases = broker();
    (router(leases.clone(), Arc::new(tokens())), leases)
}

fn at(offset: Duration) -> String {
    (Timestamp::now() + offset).to_string()
}

fn bind_body(run: &str, agent: &str, expires_at: &str) -> String {
    json!({
        "run": run,
        "agent": agent,
        "person": "person@example.com",
        "model": MODEL,
        "expiresAt": expires_at,
    })
    .to_string()
}

async fn call(app: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, String) {
    let response = app.clone().oneshot(request).await.expect("a response");
    let status = response.status();
    let headers = response.headers().clone();
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("a body");
    (status, headers, String::from_utf8_lossy(&body).into_owned())
}

fn request(method: &str, path: &str, token: Option<&str>, body: Option<String>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("a request"),
        None => builder.body(Body::empty()).expect("a request"),
    }
}

async fn bind(app: &Router, run: &str, agent: &str, expires_at: &str) -> StatusCode {
    bind_model(app, run, agent, MODEL, expires_at).await
}

/// Binds `run` naming `model` as its endpoint, as a caller that already knows it does.
async fn bind_model(
    app: &Router,
    run: &str,
    agent: &str,
    model: &str,
    expires_at: &str,
) -> StatusCode {
    let body = json!({
        "run": run,
        "agent": agent,
        "person": "person@example.com",
        "model": model,
        "expiresAt": expires_at,
    })
    .to_string();
    call(
        app,
        request("POST", "/internal/runs", Some(ORCHESTRATOR), Some(body)),
    )
    .await
    .0
}

async fn lease(app: &Router, run: &str, token: Option<&str>) -> (StatusCode, HeaderMap, String) {
    call(
        app,
        request("GET", &format!("/internal/runs/{run}/lease"), token, None),
    )
    .await
}

#[tokio::test]
async fn the_injector_gets_the_lease_and_no_store() {
    let (app, _) = app();
    assert_eq!(
        bind(&app, "run-1", "coder", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    let (status, headers, body) = lease(&app, "run-1", Some(INJECTOR)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        headers
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let body: Value = serde_json::from_str(&body).expect("JSON");
    assert_eq!(body["token"], "model-bearer");
    assert_eq!(
        body["accountId"], "person@example.com",
        "the lease names the account"
    );
    assert!(body["expiresAt"].is_string());
}

#[tokio::test]
async fn the_account_id_is_in_the_lease_and_in_no_refusal() {
    let (app, _) = app();
    bind(&app, "run-acct", "coder", &at(Duration::from_secs(60))).await;
    // Every refusal of the route is the slug and nothing else: no account id in it.
    for (token, run) in [(CODER, "run-acct"), (INJECTOR, "run-unknown")] {
        let (_, _, body) = lease(&app, run, Some(token)).await;
        assert!(
            !body.contains("person@example.com"),
            "refusal holds the account: {body}"
        );
    }
    // And the lease's Debug, which is what a log line or an error would use, holds neither.
    let broker = broker();
    broker
        .bind(orch_ports::RunBinding {
            run: orch_ports::RunId::new("run-dbg"),
            agent: orch_core::AgentId("coder".to_owned()),
            person: orch_core::UserId::new("acc-00000000deadbeef"),
            model: Some(MODEL.to_owned()),
            expires_at: Timestamp::now() + Duration::from_secs(60),
        })
        .await
        .expect("bind");
    let lease = broker
        .lease(
            &orch_ports::RunId::new("run-dbg"),
            &orch_ports::RunCaller::Injector,
        )
        .await
        .expect("lease");
    assert_eq!(lease.account().as_str(), "acc-00000000deadbeef");
    let shown = format!("{lease:?}");
    assert!(
        !shown.contains("acc-00000000deadbeef"),
        "Debug holds the account: {shown}"
    );
    assert!(
        !shown.contains("model-bearer"),
        "Debug holds the key: {shown}"
    );
}

#[tokio::test]
async fn a_wrong_caller_with_the_right_run_id_is_run_not_bound() {
    let (app, _) = app();
    assert_eq!(
        bind(&app, "run-2", "coder", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    // An agent holds a real token, and the id, and it is still refused: the run id is not a
    // credential.
    let (status, _, body) = lease(&app, "run-2", Some(CHAT)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["code"],
        "run_not_bound"
    );
    // The orchestrator leases: with the injector it is one of the two callers the lookup answers.
    let (status, _, _) = lease(&app, "run-2", Some(ORCHESTRATOR)).await;
    assert_eq!(status, StatusCode::OK);
    // No credential at all is the door's refusal, not the run's.
    let (status, headers, _) = lease(&app, "run-2", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        headers
            .get(header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok()),
        Some("Bearer")
    );
}

#[tokio::test]
async fn an_agent_is_refused_its_own_runs_lease_and_anothers() {
    let (app, _) = app();
    assert_eq!(
        bind(&app, "run-3a", "coder", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        bind(&app, "run-3b", "chat", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    // Its own run included: the binding names the agent, it does not authorise it.
    assert_eq!(
        lease(&app, "run-3a", Some(CODER)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        lease(&app, "run-3b", Some(CODER)).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        lease(&app, "run-3b", Some(CHAT)).await.0,
        StatusCode::NOT_FOUND
    );
    // The two callers the lookup answers do.
    assert_eq!(
        lease(&app, "run-3a", Some(INJECTOR)).await.0,
        StatusCode::OK
    );
    assert_eq!(
        lease(&app, "run-3b", Some(ORCHESTRATOR)).await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_deleted_binding_is_run_not_bound_and_an_expired_one_is_lease_expired() {
    let (app, leases) = app();
    assert_eq!(
        bind(&app, "run-4", "coder", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    let (status, _, _) = call(
        &app,
        request("DELETE", "/internal/runs/run-4", Some(ORCHESTRATOR), None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _, body) = lease(&app, "run-4", Some(INJECTOR)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["code"],
        "run_not_bound"
    );

    assert_eq!(
        bind(&app, "run-5", "coder", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    let (status, _, _) = call(
        &app,
        request("DELETE", "/internal/runs/run-5", Some(ORCHESTRATOR), None),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // A binding whose deadline has passed: bound directly, because the route takes a deadline and
    // the broker caps it but does not refuse a past one (ADR 0084 decision 1 leaves expiry to it).
    leases
        .bind(orch_ports::RunBinding {
            run: RunId::new("run-6"),
            agent: AgentId("coder".to_owned()),
            person: orch_core::UserId::new("person@example.com"),
            model: Some(MODEL.to_owned()),
            expires_at: Timestamp::now() - Duration::from_secs(5),
        })
        .await
        .expect("bind a past deadline");
    let (status, _, body) = lease(&app, "run-6", Some(INJECTOR)).await;
    assert_eq!(status, StatusCode::GONE);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["code"],
        "lease_expired"
    );
}

#[tokio::test]
async fn a_lease_never_passes_the_bindings_deadline() {
    let (app, _) = app();
    let deadline = at(Duration::from_secs(5));
    assert_eq!(
        bind(&app, "run-7", "coder", &deadline).await,
        StatusCode::NO_CONTENT
    );
    let (status, _, body) = lease(&app, "run-7", Some(INJECTOR)).await;
    assert_eq!(status, StatusCode::OK);
    let body: Value = serde_json::from_str(&body).expect("JSON");
    let expiry: Timestamp = body["expiresAt"]
        .as_str()
        .expect("a time")
        .parse()
        .expect("RFC 3339");
    let bound: Timestamp = deadline.parse().expect("RFC 3339");
    assert!(
        expiry <= bound,
        "the lease {expiry} passes the binding {bound}"
    );
}

#[tokio::test]
async fn the_same_binding_is_idempotent_and_a_different_one_is_a_conflict() {
    let (app, _) = app();
    let expires_at = at(Duration::from_secs(60));
    assert_eq!(
        bind(&app, "run-8", "coder", &expires_at).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        bind(&app, "run-8", "coder", &expires_at).await,
        StatusCode::NO_CONTENT
    );
    let (status, _, body) = call(
        &app,
        request(
            "POST",
            "/internal/runs",
            Some(ORCHESTRATOR),
            Some(bind_body("run-8", "chat", &expires_at)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["code"],
        "run_already_bound"
    );
}

#[tokio::test]
async fn a_run_bound_to_one_endpoint_is_not_re_pointed_to_another() {
    let (app, _) = app();
    let expires_at = at(Duration::from_secs(60));
    assert_eq!(
        bind_model(&app, "run-12", "coder", "model-x", &expires_at).await,
        StatusCode::NO_CONTENT
    );
    // The same run, agent, person and deadline; only the endpoint label differs (ADR 0084, the
    // status note of 2026-11-24: one bind, one endpoint).
    let (status, _, body) = call(
        &app,
        request(
            "POST",
            "/internal/runs",
            Some(ORCHESTRATOR),
            Some(
                json!({
                    "run": "run-12",
                    "agent": "coder",
                    "person": "person@example.com",
                    "model": "model-y",
                    "expiresAt": expires_at,
                })
                .to_string(),
            ),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["code"],
        "run_already_bound"
    );
    // The binding is still the first endpoint's.
    let (status, _, body) = lease(&app, "run-12", Some(INJECTOR)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["token"],
        "model-bearer"
    );
}

#[tokio::test]
async fn each_binding_leases_the_credential_of_the_endpoint_the_caller_named() {
    // A deployment with a credential per endpoint, as `models.endpoints.<name>.apiKey` gives it.
    let credentials = MemoryModelCredentials::new(BTreeMap::from([
        ("model-x".to_owned(), ToolSecret::new("bearer-x")),
        ("model-y".to_owned(), ToolSecret::new("bearer-y")),
    ]));
    let leases = Arc::new(RunLeaseBroker::new(
        MemoryRunBindingStore::default(),
        credentials,
    ));
    let app = router(leases, Arc::new(tokens()));
    let expires_at = at(Duration::from_secs(60));
    assert_eq!(
        bind_model(&app, "run-13", "coder", "model-x", &expires_at).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        bind_model(&app, "run-14", "coder", "model-y", &expires_at).await,
        StatusCode::NO_CONTENT
    );
    // The optional `model` of the route is the caller's choice and is kept as sent (ADR 0084
    // decision 1): nothing here re-resolves it.
    for (run, token) in [("run-13", "bearer-x"), ("run-14", "bearer-y")] {
        let (status, _, body) = lease(&app, run, Some(INJECTOR)).await;
        assert_eq!(status, StatusCode::OK, "{run}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).expect("JSON")["token"],
            token,
            "{run}"
        );
    }
}

#[tokio::test]
async fn an_agent_may_not_bind_or_unbind_and_a_bad_body_is_refused() {
    let (app, _) = app();
    let (status, _, body) = call(
        &app,
        request(
            "POST",
            "/internal/runs",
            Some(CODER),
            Some(bind_body("run-9", "coder", &at(Duration::from_secs(60)))),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("JSON")["code"],
        "not_allowed"
    );

    assert_eq!(
        bind(&app, "run-9", "coder", &at(Duration::from_secs(60))).await,
        StatusCode::NO_CONTENT
    );
    let (status, _, _) = call(
        &app,
        request("DELETE", "/internal/runs/run-9", Some(CODER), None),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // The binding is still there: an agent's refusal changed nothing.
    assert_eq!(lease(&app, "run-9", Some(INJECTOR)).await.0, StatusCode::OK);

    for bad in [
        json!({"run": "run-10"}).to_string(),
        json!({"run": "run-10", "agent": "coder", "person": "p", "expiresAt": "tomorrow"})
            .to_string(),
        String::new(),
    ] {
        let (status, _, _) = call(
            &app,
            request(
                "POST",
                "/internal/runs",
                Some(ORCHESTRATOR),
                Some(bad.clone()),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
}

#[tokio::test]
async fn the_routes_are_not_in_the_public_api_document() {
    let document = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/chat-api.yaml"),
    )
    .expect("the public API document");
    assert!(
        !document.contains("internal/runs"),
        "the run routes belong to the internal listener only (ADR 0084 decision 5)"
    );
}

#[test]
fn the_readme_lists_the_three_routes() {
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("the crate README");
    for route in [
        "POST /internal/runs",
        "DELETE /internal/runs/{run}",
        "GET /internal/runs/{run}/lease",
    ] {
        assert!(readme.contains(route), "the README does not list {route}");
    }
}

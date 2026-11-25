//! The minimum application version (`server.minClientVersion`): a client below it gets the stable
//! problem "update the application"; with the setting off the same request passes.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use orch_api::ApiConfig;
use orch_api::client_version::ClientVersion;
use orch_app::{AgentDirectory, App, AppConfig};
use orch_auth_jwt::testkit::TestIdp;
use orch_auth_jwt::{JwtAuth, JwtConfig};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{PortSet, SystemClock};
use reqwest::StatusCode;
use serde_json::Value;
use tokio::task::JoinHandle;

struct Server {
    base: String,
    client: reqwest::Client,
    _idp: TestIdp,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(min: Option<&str>) -> Server {
    let idp = TestIdp::start().await;
    let jwt = JwtConfig::new(idp.issuer(), ["another-agentic"]).without_system_proxy();
    let directory = AgentDirectory::new(Vec::new());
    let app = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: MemoryStore::new(),
                wakeup: MemoryWakeup::new(),
                agents: ScriptedAgent::new(),
                clock: SystemClock,
                ids: SeqIds::default(),
                model: orch_ports::NoModel,
                auth: JwtAuth::new(jwt).unwrap(),
                registry: directory.fixed_registry(),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory,
            AppConfig::default(),
        )
        .expect("a valid gate"),
    );
    let cfg = ApiConfig {
        min_client_version: min.map(|m| m.parse::<ClientVersion>().unwrap()),
        ..ApiConfig::default()
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = orch_api::router(app, cfg);
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        base: format!("http://{addr}"),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        _idp: idp,
        task,
    }
}

impl Server {
    async fn get(&self, path: &str, version: Option<&str>) -> reqwest::Response {
        let mut request = self.client.get(format!("{}{path}", self.base));
        if let Some(version) = version {
            request = request.header("X-Client-Version", version);
        }
        request.send().await.unwrap()
    }
}

#[tokio::test]
async fn client_below_minimum_gets_update_required_problem() {
    let s = serve(Some("2.1.0")).await;
    for stated in [
        Some("2.0.9"),
        Some("1.99"),
        Some("garbage"),
        Some("2.1.0.x"),
        None,
    ] {
        let response = s.get("/api/config", stated).await;
        assert_eq!(
            response.status(),
            StatusCode::UPGRADE_REQUIRED,
            "{stated:?}"
        );
        assert_eq!(
            response.headers()["content-type"],
            "application/problem+json"
        );
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["type"], "urn:vymalo:problem:client-update-required");
        assert_eq!(body["code"], "client_update_required");
        assert_eq!(body["status"], 426);
        assert_eq!(body["minClientVersion"], "2.1.0");
    }
}

#[tokio::test]
async fn a_client_at_or_above_the_minimum_reaches_the_identity_layer() {
    let s = serve(Some("2.1.0")).await;
    for stated in ["2.1.0", "2.1", "2.10.0", "3"] {
        let response = s.get("/api/config", Some(stated)).await;
        // no token: the request got past the version check and met the identity layer
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{stated}");
    }
}

#[tokio::test]
async fn with_the_setting_off_the_same_request_passes() {
    let s = serve(None).await;
    for stated in [None, Some("0.0.1"), Some("garbage")] {
        let response = s.get("/api/config", stated).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{stated:?}");
    }
}

#[tokio::test]
async fn health_and_public_routes_are_not_asked_for_a_version() {
    let s = serve(Some("2.1.0")).await;
    let health = s.get("/healthz", None).await;
    assert_ne!(health.status(), StatusCode::UPGRADE_REQUIRED);
    let public = s.get("/api/public/auth", None).await;
    assert_ne!(public.status(), StatusCode::UPGRADE_REQUIRED);
}

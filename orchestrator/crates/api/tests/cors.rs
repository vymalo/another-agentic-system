//! Calls from a page of another origin (CORS, ADR 0047): the desktop and mobile apps call the API
//! from `tauri://localhost` or `http://tauri.localhost`. An allow-list, never `*`, never
//! credentials; a preflight is answered before identity; a refusal is readable by an allowed page.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use orch_api::{ApiConfig, BrowserAuth};
use orch_app::{AgentDirectory, App, AppConfig};
use orch_auth_jwt::testkit::{DpopKey, TestIdp};
use orch_auth_jwt::{DpopConfig, JwtAuth, JwtConfig};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::{PortSet, SystemClock};
use reqwest::{Method, StatusCode};
use tokio::task::JoinHandle;

const AUDIENCE: &str = "another-agentic";
const ALICE: &str = "alice@example.com";
/// The API's public origin: what a proof's `htu` names, wherever the page is.
const API: &str = "https://chat.example.com";
const APP: &str = "tauri://localhost";

struct Server {
    base: String,
    client: reqwest::Client,
    idp: TestIdp,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(origins: &[&str]) -> Server {
    let idp = TestIdp::start().await;
    let jwt = JwtConfig::new(idp.issuer(), [AUDIENCE])
        .without_system_proxy()
        .with_dpop(DpopConfig::new([API]));
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
            },
            directory,
            AppConfig::default(),
        )
        .expect("a valid gate"),
    );
    let cfg = ApiConfig {
        browser_auth: Some(BrowserAuth {
            issuer: "https://idp.example/realms/main".to_owned(),
            client_id: "another-agentic-desktop".to_owned(),
            scope: "openid email profile offline_access".to_owned(),
        }),
        cors_allowed_origins: origins.iter().map(|o| (*o).to_owned()).collect(),
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
        idp,
        task,
    }
}

impl Server {
    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.client.request(method, format!("{}{path}", self.base))
    }

    /// The preflight a browser sends before a DPoP call from `origin`.
    async fn preflight(&self, origin: &str, path: &str) -> reqwest::Response {
        self.request(Method::OPTIONS, path)
            .header("Origin", origin)
            .header("Access-Control-Request-Method", "GET")
            .header("Access-Control-Request-Headers", "authorization,dpop")
            .send()
            .await
            .unwrap()
    }
}

fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_ascii_lowercase())
}

#[tokio::test]
async fn without_an_allow_list_no_page_of_another_origin_may_read_an_answer() {
    let s = serve(&[]).await;
    let preflight = s.preflight(APP, "/api/me").await;
    assert_eq!(header(&preflight, "access-control-allow-origin"), None);
    let public = s
        .request(Method::GET, "/api/public/auth")
        .header("Origin", APP)
        .send()
        .await
        .unwrap();
    assert_eq!(public.status(), StatusCode::OK);
    assert_eq!(header(&public, "access-control-allow-origin"), None);
}

#[tokio::test]
async fn a_preflight_of_an_allowed_origin_is_answered_before_identity_without_credentials() {
    let s = serve(&[APP, "http://tauri.localhost"]).await;
    for origin in [APP, "http://tauri.localhost"] {
        let preflight = s.preflight(origin, "/api/me").await;
        assert!(preflight.status().is_success(), "{}", preflight.status());
        assert_eq!(
            header(&preflight, "access-control-allow-origin").as_deref(),
            Some(origin)
        );
        let allowed = header(&preflight, "access-control-allow-headers").unwrap();
        for name in ["authorization", "dpop", "content-type", "last-event-id"] {
            assert!(allowed.contains(name), "{name} in {allowed}");
        }
        assert!(
            header(&preflight, "access-control-allow-methods")
                .unwrap()
                .contains("post")
        );
        assert_eq!(header(&preflight, "access-control-allow-credentials"), None);
        assert_eq!(
            header(&preflight, "access-control-max-age").as_deref(),
            Some("600")
        );
        assert!(header(&preflight, "vary").unwrap().contains("origin"));
    }
}

#[tokio::test]
async fn an_origin_that_is_not_listed_gets_no_allowance() {
    let s = serve(&[APP]).await;
    for origin in [
        "https://evil.example",
        "null",
        "tauri://localhost.evil",
        "TAURI://LOCALHOST",
    ] {
        let preflight = s.preflight(origin, "/api/me").await;
        assert_eq!(
            header(&preflight, "access-control-allow-origin"),
            None,
            "{origin}"
        );
    }
}

#[tokio::test]
async fn a_dpop_call_from_the_app_is_answered_and_its_headers_can_be_read() {
    let s = serve(&[APP]).await;
    let key = DpopKey::es256();
    let token = s.idp.bound_token(AUDIENCE, ALICE, &key);
    let ok = s
        .request(Method::GET, "/api/me")
        .header("Origin", APP)
        .header("Authorization", format!("DPoP {token}"))
        .header("DPoP", key.proof("GET", &format!("{API}/api/me"), &token))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(
        header(&ok, "access-control-allow-origin").as_deref(),
        Some(APP)
    );
    assert_eq!(header(&ok, "access-control-allow-credentials"), None);
    let exposed = header(&ok, "access-control-expose-headers").unwrap();
    for name in ["www-authenticate", "date", "content-disposition"] {
        assert!(exposed.contains(name), "{name} in {exposed}");
    }
}

#[tokio::test]
async fn a_refusal_is_readable_by_the_app_so_it_knows_to_refresh() {
    let s = serve(&[APP]).await;
    let refused = s
        .request(Method::GET, "/api/me")
        .header("Origin", APP)
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        header(&refused, "access-control-allow-origin").as_deref(),
        Some(APP)
    );
    assert!(header(&refused, "www-authenticate").is_some());
}

#[tokio::test]
async fn a_wildcard_or_null_in_the_list_allows_nothing() {
    let s = serve(&["*", "null"]).await;
    let preflight = s.preflight("https://anything.example", "/api/me").await;
    assert_eq!(header(&preflight, "access-control-allow-origin"), None);
    let preflight = s.preflight("null", "/api/me").await;
    assert_eq!(header(&preflight, "access-control-allow-origin"), None);
}

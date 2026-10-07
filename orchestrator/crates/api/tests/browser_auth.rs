//! A browser's sign-in at the edge (ADR 0054): `Authorization: DPoP` and the `DPoP` proof through
//! the identity layer, the challenges a refusal carries, the streams that end with the token, and
//! the public route that says where to sign in.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::Arc;

use axum::Extension;
use axum::Router;
use axum::routing::get;
use orch_api::{ApiConfig, BrowserAuth, PublicLimits, SurfaceRoutes};
use orch_app::{AgentDirectory, App, AppConfig};
use orch_auth_jwt::testkit::{DpopKey, TestIdp};
use orch_auth_jwt::{DpopConfig, JwtAuth, JwtConfig};
use orch_ports::memory::{MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
use orch_ports::testkit::bearer::{Alg, Signing};
use orch_ports::{PortSet, Principal, SystemClock};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio::task::JoinHandle;

const AUDIENCE: &str = "orchestrator-web";
const ALICE: &str = "alice@example.com";
const ORIGIN: &str = "https://chat.example.com";

struct Edge {
    base: String,
    client: reqwest::Client,
    idp: TestIdp,
    server: JoinHandle<()>,
}

impl Drop for Edge {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn browser() -> BrowserAuth {
    BrowserAuth {
        issuer: "https://idp.example/realms/main".to_owned(),
        client_id: "another-agentic-web".to_owned(),
        scope: "openid email profile offline_access".to_owned(),
    }
}

/// `/x/budget`: how many seconds a stream of the caller may stay open (`stream_budget`).
fn budget_surface() -> SurfaceRoutes {
    async fn budget(Extension(principal): Extension<Principal>) -> String {
        orch_api::sse::stream_budget(&principal, jiff::Timestamp::now())
            .map_or_else(|| "unbounded".to_owned(), |d| d.as_secs().to_string())
    }
    SurfaceRoutes::new().streaming(Router::new().route("/x/budget", get(budget)))
}

async fn edge(dpop: bool, cfg: ApiConfig) -> Edge {
    let idp = TestIdp::start().await;
    let mut jwt = JwtConfig::new(idp.issuer(), [AUDIENCE]).without_system_proxy();
    if dpop {
        jwt = jwt.with_dpop(DpopConfig::new([ORIGIN]));
    }
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = orch_api::router_with_surfaces(app, cfg, vec![budget_surface()]);
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Edge {
        base: format!("http://{addr}"),
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
        idp,
        server,
    }
}

impl Edge {
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(Method::GET, format!("{}{path}", self.base))
    }

    /// `GET path` with the DPoP scheme: `token` and one good proof of `key` for the public URL.
    fn dpop(&self, path: &str, key: &DpopKey, token: &str) -> reqwest::RequestBuilder {
        let htu = format!("{ORIGIN}{}", path.split('?').next().unwrap());
        self.get(path)
            .header("Authorization", format!("DPoP {token}"))
            .header("DPoP", key.proof("GET", &htu, token))
    }
}

fn challenges(response: &reqwest::Response) -> Vec<String> {
    response
        .headers()
        .get_all("www-authenticate")
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect()
}

const ALGS: &str = "algs=\"ES256 EdDSA\"";

#[tokio::test]
async fn a_dpop_request_is_authenticated_and_its_query_is_not_part_of_the_url() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let response = e.dpop("/api/me", &key, &token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let me: Value = response.json().await.unwrap();
    assert_eq!(me["user"], ALICE, "{me}");
    // htu is the URL without the query.
    let response = e
        .dpop("/api/agents?x=1", &key, &token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_eddsa_proof_is_authenticated_too() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::ed25519();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let response = e.dpop("/api/me", &key, &token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_replayed_proof_is_a_401_that_names_the_proof() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let proof = key.proof("GET", &format!("{ORIGIN}/api/me"), &token);
    let send = || {
        e.get("/api/me")
            .header("Authorization", format!("DPoP {token}"))
            .header("DPoP", &proof)
            .send()
    };
    assert_eq!(send().await.unwrap().status(), StatusCode::OK);
    let again = send().await.unwrap();
    assert_eq!(again.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&again),
        [format!("DPoP error=\"invalid_dpop_proof\", {ALGS}")]
    );
    let problem: Value = again.json().await.unwrap();
    assert_eq!(problem["status"], 401);
    assert!(!problem.to_string().contains(&token), "{problem}");
}

#[tokio::test]
async fn a_proof_for_another_method_path_or_origin_is_a_401_that_names_the_proof() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    for (method, htu) in [
        ("POST", format!("{ORIGIN}/api/me")),
        ("GET", format!("{ORIGIN}/api/agents")),
        ("GET", "https://evil.example/api/me".to_owned()),
    ] {
        let response = e
            .get("/api/me")
            .header("Authorization", format!("DPoP {token}"))
            .header("DPoP", key.proof(method, &htu, &token))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {htu}"
        );
        assert_eq!(
            challenges(&response),
            [format!("DPoP error=\"invalid_dpop_proof\", {ALGS}")],
            "{method} {htu}"
        );
    }
}

#[tokio::test]
async fn two_proofs_or_none_are_a_401_that_names_the_proof() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let url = format!("{ORIGIN}/api/me");
    let two = e
        .get("/api/me")
        .header("Authorization", format!("DPoP {token}"))
        .header("DPoP", key.proof("GET", &url, &token))
        .header("DPoP", key.proof("GET", &url, &token))
        .send()
        .await
        .unwrap();
    let none = e
        .get("/api/me")
        .header("Authorization", format!("DPoP {token}"))
        .send()
        .await
        .unwrap();
    for response in [two, none] {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            challenges(&response),
            [format!("DPoP error=\"invalid_dpop_proof\", {ALGS}")]
        );
    }
}

#[tokio::test]
async fn a_proof_of_another_key_than_the_tokens_is_a_401_that_names_the_token() {
    let e = edge(true, ApiConfig::default()).await;
    let (owner, thief) = (DpopKey::es256(), DpopKey::es256());
    let token = e.idp.bound_token(AUDIENCE, ALICE, &owner);
    let response = e.dpop("/api/me", &thief, &token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&response),
        [format!("DPoP error=\"invalid_token\", {ALGS}")]
    );
    // And a token with no binding, sent as DPoP.
    let plain = e.idp.token(AUDIENCE, ALICE);
    let response = e.dpop("/api/me", &thief, &plain).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&response),
        [format!("DPoP error=\"invalid_token\", {ALGS}")]
    );
}

#[tokio::test]
async fn a_bound_token_sent_as_a_bearer_is_a_401_that_names_the_token() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let response = e.get("/api/me").bearer_auth(&token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&response),
        [format!("DPoP error=\"invalid_token\", {ALGS}")]
    );
}

#[tokio::test]
async fn a_plain_bearer_is_unchanged_and_the_generic_challenge_names_both_schemes() {
    let e = edge(true, ApiConfig::default()).await;
    let ok = e
        .get("/api/me")
        .bearer_auth(e.idp.token(AUDIENCE, ALICE))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let nothing = e.get("/api/me").send().await.unwrap();
    assert_eq!(nothing.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&nothing),
        [
            "Bearer realm=\"orchestrator\"".to_owned(),
            format!("DPoP {ALGS}")
        ]
    );
    let bad = e
        .get("/api/me")
        .bearer_auth("garbage")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        challenges(&bad),
        [
            "Bearer realm=\"orchestrator\", error=\"invalid_token\"".to_owned(),
            format!("DPoP {ALGS}")
        ]
    );
}

#[tokio::test]
async fn without_dpop_the_scheme_is_refused_and_only_bearer_is_advertised() {
    let e = edge(false, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let token = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let response = e.dpop("/api/me", &key, &token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let shown = challenges(&response);
    assert_eq!(
        shown,
        ["Bearer realm=\"orchestrator\", error=\"invalid_token\""]
    );
    assert!(shown.iter().all(|c| !c.contains("DPoP")));
    // The same bound token as a bearer is what it always was.
    let response = e.get("/api/me").bearer_auth(&token).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let nothing = e.get("/api/me").send().await.unwrap();
    assert_eq!(challenges(&nothing), ["Bearer realm=\"orchestrator\""]);
}

#[tokio::test]
async fn a_stream_ends_with_the_token_whether_it_came_as_a_bearer_or_with_dpop() {
    let e = edge(true, ApiConfig::default()).await;
    let key = DpopKey::es256();
    let bound = e.idp.bound_token(AUDIENCE, ALICE, &key);
    let budget = |body: String| body.parse::<u64>().unwrap_or_else(|_| panic!("{body}"));
    let by_dpop = budget(
        e.dpop("/x/budget", &key, &bound)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    );
    let by_bearer = budget(
        e.get("/x/budget")
            .bearer_auth(e.idp.token(AUDIENCE, ALICE))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    );
    // The test token lives five minutes, and a stream may outlive it by the leeway (a minute).
    for seconds in [by_dpop, by_bearer] {
        assert!((355..=360).contains(&seconds), "{seconds}");
    }
    // A token that has run out of its leeway has no stream at all: the request is refused earlier.
    let mut claims = e.idp.bound_claims(AUDIENCE, ALICE, &key);
    claims.insert(
        "exp".into(),
        json!(jiff::Timestamp::now().as_second() - 3600),
    );
    let expired = e.idp.mint(&claims, Signing::Published(Alg::Rs256));
    let response = e.dpop("/x/budget", &key, &expired).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// --- GET /api/public/auth ---

#[tokio::test]
async fn the_public_route_says_where_to_sign_in_and_needs_no_identity() {
    let cfg = ApiConfig {
        browser_auth: Some(browser()),
        ..ApiConfig::default()
    };
    let e = edge(true, cfg).await;
    // No credential at all: the route is outside the identity layer.
    let response = e.get("/api/public/auth").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
    assert!(
        response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("application/json")
    );
    assert!(response.headers().get("www-authenticate").is_none());
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body,
        json!({
            "issuer": "https://idp.example/realms/main",
            "clientId": "another-agentic-web",
            "scope": "openid email profile offline_access",
        })
    );
    // A credential beside it counts for nothing, even a bad one.
    let response = e
        .get("/api/public/auth")
        .bearer_auth("garbage")
        .header("DPoP", "garbage")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    // Only GET.
    let response = e
        .client
        .post(format!("{}/api/public/auth", e.base))
        .send()
        .await
        .unwrap();
    assert!(
        response.status() == StatusCode::METHOD_NOT_ALLOWED
            || response.status() == StatusCode::NOT_FOUND,
        "{}",
        response.status()
    );
}

#[tokio::test]
async fn the_public_route_is_a_404_problem_where_browser_sign_in_is_not_configured() {
    let e = edge(true, ApiConfig::default()).await;
    let response = e.get("/api/public/auth").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(
        content_type.starts_with("application/problem+json"),
        "{content_type}"
    );
    let problem: Value = response.json().await.unwrap();
    assert_eq!(problem["status"], 404);
    // The same body as any other route that does not exist.
    let other = e
        .get("/api/nothing-here")
        .bearer_auth(e.idp.token(AUDIENCE, ALICE))
        .send()
        .await
        .unwrap();
    let other: Value = other.json().await.unwrap();
    assert_eq!(problem["detail"], other["detail"]);
}

#[tokio::test]
async fn the_public_route_is_behind_the_public_rate_limit() {
    let cfg = ApiConfig {
        browser_auth: Some(browser()),
        public_limits: Some(PublicLimits {
            per_link_per_second: 2,
            total_per_second: 100,
            streams_per_link: 1,
            streams_total: 1,
        }),
        ..ApiConfig::default()
    };
    let e = edge(true, cfg).await;
    let mut statuses = Vec::new();
    for _ in 0..8 {
        statuses.push(e.get("/api/public/auth").send().await.unwrap().status());
    }
    assert!(statuses.contains(&StatusCode::OK), "{statuses:?}");
    assert!(
        statuses.contains(&StatusCode::TOO_MANY_REQUESTS),
        "{statuses:?}"
    );
}

#[tokio::test]
async fn the_public_route_without_a_limiter_is_closed() {
    // Fail closed, as every public route: no limiter, no public route.
    let cfg = ApiConfig {
        browser_auth: Some(browser()),
        public_limits: None,
        ..ApiConfig::default()
    };
    let e = edge(true, cfg).await;
    let response = e.get("/api/public/auth").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn asking_where_to_sign_in_does_not_spend_the_budget_of_shared_links() {
    // A deployment without the route is asked by every page load; those 404s are no guesses at a link.
    let cfg = ApiConfig {
        public_limits: Some(PublicLimits {
            per_link_per_second: 100,
            total_per_second: 10,
            streams_per_link: 1,
            streams_total: 1,
        }),
        ..ApiConfig::default()
    };
    let e = edge(true, cfg).await;
    for _ in 0..3 {
        let response = e.get("/api/public/auth").send().await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    // 3 requests of a bucket of 10: had each 404 cost the failure extra, the bucket would be empty.
    let response = e.get("/api/public/auth").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "not 429");
}

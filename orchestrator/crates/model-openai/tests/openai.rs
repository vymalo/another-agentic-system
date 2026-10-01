//! The OpenAI-compatible adapter against an in-process stub of the endpoint, over real HTTP: the
//! `ChatModel` conformance suite, and what only this adapter has to say (the wire shape, the
//! bearer token, redirects, timeouts, size bounds).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use orch_core::{Classify, ErrorClass};
use orch_model_openai::{BuildError, OpenAiChat, OpenAiConfig};
use orch_ports::testkit::chat_model::ModelFixture;
use orch_ports::{ChatModel, ChatRequest, ModelError};
use secrecy::SecretString;
use serde_json::{Value, json};

const SECRET: &str = "sk-test-0123456789-very-secret";

#[derive(Clone)]
enum Mode {
    Answer(String),
    /// 200 with this body, as it is.
    Body(String),
    Down,
    RateLimit(Option<&'static str>),
    Reject(String),
    RefuseKey,
    Nonsense,
    Hang,
    Redirect,
    /// 200 with a body over the adapter's bound.
    Huge,
}

struct Stub {
    mode: Mutex<Mode>,
    body: Mutex<Option<Value>>,
    authorization: Mutex<Option<String>>,
    paths: Mutex<Vec<String>>,
    hits: AtomicUsize,
    /// What the stub accepts as the credential; `None` accepts any (or none).
    key: Option<&'static str>,
}

fn completion(text: &str) -> Value {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}]
    })
}

async fn chat(State(stub): State<Arc<Stub>>, headers: HeaderMap, body: String) -> Response {
    stub.hits.fetch_add(1, Ordering::SeqCst);
    *stub.authorization.lock().unwrap() = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    *stub.body.lock().unwrap() = serde_json::from_str(&body).ok();
    if let Some(key) = stub.key {
        let want = format!("Bearer {key}");
        let got = stub.authorization.lock().unwrap().clone();
        if got.as_deref() != Some(want.as_str()) {
            return (StatusCode::UNAUTHORIZED, "no key").into_response();
        }
    }
    let mode = stub.mode.lock().unwrap().clone();
    match mode {
        Mode::Answer(text) => axum::Json(completion(&text)).into_response(),
        Mode::Body(text) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            text,
        )
            .into_response(),
        Mode::Down => (StatusCode::SERVICE_UNAVAILABLE, "overloaded").into_response(),
        Mode::RateLimit(after) => {
            let mut response = (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
            if let Some(after) = after {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, after.parse().unwrap());
            }
            response
        }
        Mode::Reject(message) => (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({"error": {"message": message, "type": "invalid_request_error"}})),
        )
            .into_response(),
        Mode::RefuseKey => (StatusCode::UNAUTHORIZED, "bad key").into_response(),
        Mode::Nonsense => (StatusCode::OK, "<html>not a completion</html>").into_response(),
        Mode::Hang => {
            tokio::time::sleep(Duration::from_secs(30)).await;
            StatusCode::OK.into_response()
        }
        Mode::Redirect => (
            StatusCode::FOUND,
            [(header::LOCATION, "http://127.0.0.1:1/elsewhere")],
        )
            .into_response(),
        Mode::Huge => (StatusCode::OK, "x".repeat(300 * 1024)).into_response(),
    }
}

async fn serve(prefix: &str, key: Option<&'static str>) -> (Arc<Stub>, String) {
    let stub = Arc::new(Stub {
        mode: Mutex::new(Mode::Answer("A title".to_owned())),
        body: Mutex::new(None),
        authorization: Mutex::new(None),
        paths: Mutex::new(Vec::new()),
        hits: AtomicUsize::new(0),
        key,
    });
    let path = format!("{prefix}/chat/completions");
    let app = Router::new()
        .route(&path, post(chat))
        .with_state(Arc::clone(&stub));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    stub.paths.lock().unwrap().push(path);
    (stub, format!("http://{addr}{prefix}"))
}

fn model(base: &str, key: Option<&str>, timeout: Duration) -> OpenAiChat {
    let mut cfg = OpenAiConfig::new(base).with_timeout(timeout);
    if let Some(key) = key {
        cfg = cfg.with_api_key(SecretString::from(key.to_owned()));
    }
    OpenAiChat::new(cfg).unwrap()
}

fn question() -> ChatRequest {
    ChatRequest {
        model: "mock-title".into(),
        system: "Reply with a short title.".into(),
        user: "Title this: how do I fix the build?".into(),
        max_tokens: 24,
    }
}

// ---- the conformance suite -----------------------------------------------------------------

struct Fixture {
    model: OpenAiChat,
    stub: Arc<Stub>,
}

impl Fixture {
    fn set(&self, mode: Mode) {
        *self.stub.mode.lock().unwrap() = mode;
    }
}

impl ModelFixture for Fixture {
    type Model = OpenAiChat;

    fn model(&self) -> &OpenAiChat {
        &self.model
    }

    fn secret(&self) -> &str {
        SECRET
    }

    fn will_answer(&self, text: &str) {
        self.set(Mode::Answer(text.to_owned()));
    }

    fn will_be_unreachable(&self) {
        self.set(Mode::Down);
    }

    fn will_rate_limit(&self) {
        self.set(Mode::RateLimit(Some("2")));
    }

    fn will_reject(&self) {
        self.set(Mode::Reject("no such model: mock-title".to_owned()));
    }

    fn will_refuse_the_credential(&self) {
        self.set(Mode::RefuseKey);
    }

    fn will_answer_nonsense(&self) {
        self.set(Mode::Nonsense);
    }

    fn last_request(&self) -> Option<ChatRequest> {
        let body = self.stub.body.lock().unwrap().clone()?;
        let messages = body["messages"].as_array()?;
        let content = |role: &str| {
            messages
                .iter()
                .find(|m| m["role"] == role)
                .and_then(|m| m["content"].as_str())
                .map(str::to_owned)
        };
        Some(ChatRequest {
            model: body["model"].as_str()?.to_owned(),
            system: content("system")?,
            user: content("user")?,
            max_tokens: u32::try_from(body["max_tokens"].as_u64()?).ok()?,
        })
    }
}

async fn make() -> Option<Fixture> {
    let (stub, base) = serve("/v1", Some(SECRET)).await;
    Some(Fixture {
        model: model(&base, Some(SECRET), Duration::from_secs(5)),
        stub,
    })
}

orch_ports::chat_model_conformance!(make);

// ---- what only this adapter says -----------------------------------------------------------

#[tokio::test]
async fn the_request_is_a_non_streaming_chat_completion_with_the_bearer_token() {
    let (stub, base) = serve("/v1", Some(SECRET)).await;
    let m = model(&base, Some(SECRET), Duration::from_secs(5));
    assert_eq!(m.complete(&question()).await.unwrap(), "A title");
    assert_eq!(
        stub.body.lock().unwrap().clone().unwrap(),
        json!({
            "model": "mock-title",
            "messages": [
                {"role": "system", "content": "Reply with a short title."},
                {"role": "user", "content": "Title this: how do I fix the build?"}
            ],
            "max_tokens": 24,
            "stream": false
        })
    );
    assert_eq!(
        stub.authorization.lock().unwrap().as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );
}

#[tokio::test]
async fn an_endpoint_that_wants_no_key_is_sent_none() {
    let (stub, base) = serve("/v1", None).await;
    let m = model(&base, None, Duration::from_secs(5));
    m.complete(&question()).await.unwrap();
    assert_eq!(*stub.authorization.lock().unwrap(), None);
    // an empty key is no key
    let m = model(&base, Some(""), Duration::from_secs(5));
    m.complete(&question()).await.unwrap();
    assert_eq!(*stub.authorization.lock().unwrap(), None);
}

#[tokio::test]
async fn the_base_url_may_end_in_a_slash_and_have_a_path() {
    let (_, base) = serve("/openai/v1", None).await;
    for url in [base.clone(), format!("{base}/"), format!("  {base}  ")] {
        let m = model(&url, None, Duration::from_secs(5));
        assert_eq!(m.complete(&question()).await.unwrap(), "A title", "{url}");
    }
}

#[tokio::test]
async fn an_answer_without_text_is_a_protocol_error_that_can_be_retried() {
    let (stub, base) = serve("/v1", None).await;
    let m = model(&base, None, Duration::from_secs(5));
    for body in [
        r#"{"choices": []}"#,
        r#"{}"#,
        r#"{"choices": [{"message": {"role": "assistant", "content": null}}]}"#,
        r#"{"choices": [{"index": 0}]}"#,
        "[]",
        "",
    ] {
        *stub.mode.lock().unwrap() = Mode::Body(body.to_owned());
        let err = m.complete(&question()).await.unwrap_err();
        assert!(
            matches!(err, ModelError::Protocol { .. }),
            "{body:?}: {err:?}"
        );
        assert_eq!(err.class(), ErrorClass::Transient);
    }
}

#[tokio::test]
async fn the_wait_an_endpoint_asks_for_is_passed_on_and_bounded() {
    let (stub, base) = serve("/v1", None).await;
    let m = model(&base, None, Duration::from_secs(5));
    for (header, want) in [
        (Some("7"), Some(Duration::from_secs(7))),
        (Some("999999"), Some(Duration::from_secs(3600))),
        (Some("Wed, 21 Oct 2026 07:28:00 GMT"), None),
        (None, None),
    ] {
        *stub.mode.lock().unwrap() = Mode::RateLimit(header);
        let err = m.complete(&question()).await.unwrap_err();
        assert_eq!(err.class(), ErrorClass::RateLimited);
        assert_eq!(err.retry_after(), want, "{header:?}");
    }
}

#[tokio::test]
async fn a_refusal_carries_the_endpoints_own_words_cut_and_never_the_request() {
    let (stub, base) = serve("/v1", None).await;
    let m = model(&base, Some(SECRET), Duration::from_secs(5));
    *stub.mode.lock().unwrap() = Mode::Reject(format!("  no such   model\n{}", "x".repeat(1_000)));
    let err = m.complete(&question()).await.unwrap_err();
    let ModelError::Rejected(text) = &err else {
        panic!("{err:?}")
    };
    assert!(
        text.starts_with("400 Bad Request: no such model xxx"),
        "{text}"
    );
    assert!(text.chars().count() < 340, "cut: {}", text.chars().count());
    assert!(!text.contains("fix the build"), "{text}");
    assert!(!text.contains(SECRET));
}

#[tokio::test]
async fn a_redirect_is_never_followed_and_is_a_permanent_misconfiguration() {
    let (stub, base) = serve("/v1", None).await;
    *stub.mode.lock().unwrap() = Mode::Redirect;
    let m = model(&base, Some(SECRET), Duration::from_secs(5));
    let err = m.complete(&question()).await.unwrap_err();
    assert!(matches!(err, ModelError::Rejected(_)), "{err:?}");
    assert!(err.to_string().contains("redirected"), "{err}");
    assert_eq!(stub.hits.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_endpoint_that_does_not_answer_in_time_is_unreachable_and_says_so_without_the_url() {
    let (stub, base) = serve("/v1", Some(SECRET)).await;
    *stub.mode.lock().unwrap() = Mode::Hang;
    let m = model(&base, Some(SECRET), Duration::from_millis(200));
    let err = m.complete(&question()).await.unwrap_err();
    assert!(matches!(err, ModelError::Unreachable { .. }), "{err:?}");
    assert_eq!(err.class(), ErrorClass::Transient);
    let logged = format!("{err} {}", orch_core::report(&err));
    assert!(!logged.contains("127.0.0.1"), "{logged}");
    assert!(!format!("{err:?}").contains(SECRET));
}

#[tokio::test]
async fn an_endpoint_nobody_listens_on_is_unreachable() {
    // a port that was free a moment ago
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let m = model(
        &format!("http://127.0.0.1:{port}/v1"),
        Some(SECRET),
        Duration::from_secs(2),
    );
    let err = m.complete(&question()).await.unwrap_err();
    assert!(matches!(err, ModelError::Unreachable { .. }), "{err:?}");
    // what is logged (the message and the flattened chain) names no address and no key; the
    // transport error behind it, in `Debug`, may name the address and still never the key
    let logged = format!("{err} {}", orch_core::report(&err));
    assert!(!logged.contains("127.0.0.1"), "{logged}");
    assert!(!format!("{err:?}").contains(SECRET));
}

#[tokio::test]
async fn an_answer_over_the_bound_is_not_read() {
    let (stub, base) = serve("/v1", None).await;
    *stub.mode.lock().unwrap() = Mode::Huge;
    let m = model(&base, None, Duration::from_secs(5));
    let err = m.complete(&question()).await.unwrap_err();
    assert!(matches!(err, ModelError::Protocol { .. }), "{err:?}");
}

#[test]
fn a_base_url_that_is_not_http_is_refused_when_the_adapter_is_built() {
    for bad in [
        "",
        "   ",
        "ftp://example.com/v1",
        "localhost:8080/v1",
        "http://",
        "example.com",
    ] {
        let err = OpenAiChat::new(OpenAiConfig::new(bad)).unwrap_err();
        assert!(matches!(err, BuildError::BadBaseUrl), "{bad:?}: {err:?}");
    }
    let err = OpenAiChat::new(
        OpenAiConfig::new("http://example.com/v1")
            .with_api_key(SecretString::from("bad\nkey".to_owned())),
    )
    .unwrap_err();
    assert!(matches!(err, BuildError::BadApiKey), "{err:?}");
}

#[test]
fn nothing_the_adapter_prints_has_the_key_in_it() {
    let cfg = OpenAiConfig::new("https://api.example.com/v1")
        .with_api_key(SecretString::from(SECRET.to_owned()));
    let shown = format!("{cfg:?}");
    assert!(!shown.contains(SECRET), "{shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
    let m = OpenAiChat::new(cfg).unwrap();
    let shown = format!("{m:?}");
    assert!(!shown.contains(SECRET), "{shown}");
    assert!(shown.contains("/chat/completions"), "{shown}");
}

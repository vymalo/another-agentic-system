//! `ToolServerClient` conformance cases: what the relay of the thread-tools endpoint relies on from
//! any client of an MCP server (ADR 0024), stated once and run against the in-memory client and
//! against the MCP client (`orch-tools-mcp`) over real HTTP, against a server of the test support.
//!
//! An implementation supplies a [`ToolServerFixture`]: a client, and the endpoint of a server that
//! wants a bearer token and a header, offers four tools (below), and keeps a journal of what it
//! was asked. Each case is `async fn(fixture)` and gives up after 20 seconds.
//!
//! **The server a fixture stands for** offers:
//!
//! | Tool | Takes | Answers |
//! |---|---|---|
//! | `echo` | any arguments, `text` a string | its arguments as one text block and as the structured result |
//! | `fail` | nothing | `isError: true` with the text [`FAIL_TEXT`] |
//! | `slow` | nothing | nothing, until the caller gives up |
//! | `big` | `bytes`, an integer | one text block of that many `x` |
//!
//! What is deliberately not asserted: the wording of any error, the wire format, how the client
//! connects (a session or none), and the text of a tool's description.

use std::future::Future;
use std::time::{Duration, Instant};

use orch_core::{Classify, ErrorClass};
use serde_json::{Map, Value, json};

use crate::{
    MAX_RESULT_BYTES, ToolCall, ToolSecret, ToolServerClient, ToolServerEndpoint, ToolServerError,
};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(20);

/// The limit of the endpoint a case uses to see a call time out.
pub const SHORT_TIMEOUT: Duration = Duration::from_secs(1);

/// What the `fail` tool says.
pub const FAIL_TEXT: &str = "the tool failed on purpose";

/// A bearer token no fixture's server accepts.
const WRONG_BEARER: &str = "wrong-bearer-0123456789";

/// One request the fixture's server received, as its journal keeps it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SeenRequest {
    /// `tools/list` or `tools/call`.
    pub method: String,
    /// The tool called (`tools/call`).
    pub name: Option<String>,
    /// The arguments of the call.
    pub arguments: Option<Map<String, Value>>,
    /// The request's `_meta`, without the keys an MCP client adds itself (a progress token).
    pub meta: Option<Map<String, Value>>,
    /// The token of `Authorization: Bearer <token>`, when that header was sent.
    pub bearer: Option<String>,
    /// Every header of the request, name in lower case, in the order received.
    pub headers: Vec<(String, String)>,
}

impl SeenRequest {
    /// The value of the header `name` (any case), when the request had it.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A client under test, with a server behind it.
pub trait ToolServerFixture: Send + Sync + 'static {
    /// The client under test.
    type Client: ToolServerClient;

    /// The client.
    fn client(&self) -> &Self::Client;

    /// The bearer token the server wants. No error and no debug text may show it.
    fn bearer(&self) -> &str;

    /// The name and the value of the header the server wants beside the bearer token. The value is
    /// a secret as well.
    fn header(&self) -> (&str, &str);

    /// The endpoint of the server, with the bearer token and the header it wants, and a timeout
    /// that is long enough for any case but the one that waits for a call to time out.
    fn endpoint(&self) -> ToolServerEndpoint;

    /// An endpoint where nothing listens, with the same credentials as [`endpoint`](Self::endpoint).
    fn unreachable_endpoint(&self) -> ToolServerEndpoint;

    /// Every request the server received so far, oldest first.
    fn seen(&self) -> Vec<SeenRequest>;
}

async fn within<T>(case: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, case)
        .await
        .expect("the case timed out")
}

fn args(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("arguments are an object, not {other}"),
    }
}

fn call(name: &str, arguments: Value) -> ToolCall {
    ToolCall::new(name).with_arguments(args(arguments))
}

/// The error text a person could be shown or a log could hold, as far as this side can say: the
/// `Display` and `Debug` of the error and of every error under it.
fn everything_said_by(error: &ToolServerError) -> String {
    let mut said = format!("{error} {error:?}");
    let mut source = std::error::Error::source(error);
    while let Some(e) = source {
        said.push_str(&format!(" {e} {e:?}"));
        source = e.source();
    }
    said.push_str(&error.public_detail());
    said
}

/// The server offers its tools, each with the schema of its arguments.
pub async fn lists_the_tools_with_their_schemas<F: ToolServerFixture>(f: F) {
    within(async {
        let tools = f
            .client()
            .list_tools(&f.endpoint())
            .await
            .expect("a listing");
        let mut names: Vec<_> = tools.iter().map(|t| t.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, ["big", "echo", "fail", "slow"]);
        for tool in &tools {
            assert_eq!(
                tool.input_schema.get("type"),
                Some(&json!("object")),
                "{} takes an object",
                tool.name
            );
        }
        let echo = tools.iter().find(|t| t.name == "echo").unwrap();
        assert_eq!(
            echo.input_schema["properties"]["text"]["type"],
            json!("string"),
            "the schema is the server's own"
        );
        assert!(
            echo.description.as_deref().is_some_and(|d| !d.is_empty()),
            "the description is passed on"
        );
    })
    .await;
}

/// The server sees the bearer token and the header it asked for on a listing and on a call.
pub async fn the_credentials_reach_the_server<F: ToolServerFixture>(f: F) {
    within(async {
        let endpoint = f.endpoint();
        f.client().list_tools(&endpoint).await.expect("a listing");
        f.client()
            .call_tool(&endpoint, &call("echo", json!({"text": "hi"})))
            .await
            .expect("a result");
        let seen = f.seen();
        let listed = seen
            .iter()
            .find(|r| r.method == "tools/list")
            .expect("the listing was seen");
        let called = seen
            .iter()
            .find(|r| r.method == "tools/call")
            .expect("the call was seen");
        let (name, value) = f.header();
        for request in [listed, called] {
            assert_eq!(
                request.bearer.as_deref(),
                Some(f.bearer()),
                "{}",
                request.method
            );
            assert_eq!(request.header(name), Some(value), "{}", request.method);
        }
    })
    .await;
}

/// A call returns what the server answered, and the server was given the name, the arguments and
/// the request's `_meta` as they were put.
pub async fn an_echo_round_trip<F: ToolServerFixture>(f: F) {
    within(async {
        let arguments = json!({"text": "hello", "n": 3, "nested": {"a": [1, 2]}});
        let meta = args(json!({"thread-tools/v1": {"callId": "call-1", "parentStepId": "step-9"}}));
        let output = f
            .client()
            .call_tool(
                &f.endpoint(),
                &call("echo", arguments.clone()).with_meta(meta.clone()),
            )
            .await
            .expect("a result");
        assert!(!output.is_error && !output.truncated, "{output:?}");
        assert_eq!(output.structured, Some(arguments.clone()));
        let text: Value =
            serde_json::from_str(&output.text_of()).expect("the text is the arguments");
        assert_eq!(text, arguments);
        let seen = f.seen();
        let request = seen
            .iter()
            .rev()
            .find(|r| r.method == "tools/call")
            .expect("the call was seen");
        assert_eq!(request.name.as_deref(), Some("echo"));
        assert_eq!(request.arguments.as_ref(), arguments.as_object());
        let got = request.meta.as_ref().expect("the _meta reached the server");
        assert_eq!(got.get("thread-tools/v1"), meta.get("thread-tools/v1"));
    })
    .await;
}

/// A tool that says it failed is a result, passed through with its text: the caller gives it to
/// the agent as it is.
pub async fn is_error_is_passed_through<F: ToolServerFixture>(f: F) {
    within(async {
        let output = f
            .client()
            .call_tool(&f.endpoint(), &ToolCall::new("fail"))
            .await
            .expect("a failed tool is still a result");
        assert!(output.is_error);
        assert_eq!(output.text_of(), FAIL_TEXT);
        assert!(!output.truncated);
    })
    .await;
}

/// A server that refuses the credentials, or wants some it was not given, is `Unauthenticated`,
/// for a listing and for a call.
pub async fn a_wrong_bearer_is_unauthenticated<F: ToolServerFixture>(f: F) {
    within(async {
        let wrong = ToolServerEndpoint {
            bearer: Some(ToolSecret::new(WRONG_BEARER)),
            ..f.endpoint()
        };
        let none = ToolServerEndpoint {
            bearer: None,
            ..f.endpoint()
        };
        for endpoint in [wrong, none] {
            let err = f
                .client()
                .list_tools(&endpoint)
                .await
                .expect_err("a refusal");
            assert!(matches!(err, ToolServerError::Unauthenticated), "{err:?}");
            assert_eq!(err.class(), ErrorClass::Unauthenticated);
            let err = f
                .client()
                .call_tool(&endpoint, &call("echo", json!({"text": "hi"})))
                .await
                .expect_err("a refusal");
            assert!(matches!(err, ToolServerError::Unauthenticated), "{err:?}");
        }
    })
    .await;
}

/// A server nobody answers for is `Unreachable`, a transient failure, for a listing and for a call.
pub async fn an_unreachable_server_is_unreachable<F: ToolServerFixture>(f: F) {
    within(async {
        let endpoint = f.unreachable_endpoint();
        let err = f
            .client()
            .list_tools(&endpoint)
            .await
            .expect_err("no server");
        assert!(
            matches!(err, ToolServerError::Unreachable { .. }),
            "{err:?}"
        );
        assert_eq!(err.class(), ErrorClass::Transient);
        assert!(err.is_retryable());
        let err = f
            .client()
            .call_tool(&endpoint, &ToolCall::new("echo"))
            .await
            .expect_err("no server");
        assert!(
            matches!(err, ToolServerError::Unreachable { .. }),
            "{err:?}"
        );
        assert!(
            !err.public_detail().contains(&endpoint.url),
            "what a person may see does not name the URL"
        );
    })
    .await;
}

/// A call the server never answers is `TimedOut` after the endpoint's timeout, and no later than a
/// second after it.
pub async fn a_slow_call_times_out_within_the_limit<F: ToolServerFixture>(f: F) {
    within(async {
        let endpoint = f.endpoint().with_timeout(SHORT_TIMEOUT);
        let started = Instant::now();
        let err = f
            .client()
            .call_tool(&endpoint, &ToolCall::new("slow"))
            .await
            .expect_err("no answer");
        let took = started.elapsed();
        match err {
            ToolServerError::TimedOut { after } => assert_eq!(after, SHORT_TIMEOUT),
            other => panic!("expected a timeout, got {other:?}"),
        }
        assert!(
            took >= SHORT_TIMEOUT.mul_f32(0.9),
            "it gave up early: {took:?}"
        );
        assert!(
            took <= SHORT_TIMEOUT + Duration::from_secs(1),
            "it took {took:?} to give up"
        );
    })
    .await;
}

/// Dropping a call that has not answered abandons it, and the client serves the next one.
pub async fn a_dropped_call_leaves_the_client_usable<F: ToolServerFixture>(f: F) {
    within(async {
        let endpoint = f.endpoint();
        let abandoned = tokio::time::timeout(
            Duration::from_millis(300),
            f.client().call_tool(&endpoint, &ToolCall::new("slow")),
        )
        .await;
        assert!(abandoned.is_err(), "the slow tool answered");
        let output = f
            .client()
            .call_tool(&endpoint, &call("echo", json!({"text": "again"})))
            .await
            .expect("the next call is served");
        assert!(!output.is_error);
    })
    .await;
}

/// A tool the server does not have is the server's own JSON-RPC error, with its message.
pub async fn an_unknown_tool_is_a_remote_error<F: ToolServerFixture>(f: F) {
    within(async {
        let err = f
            .client()
            .call_tool(&f.endpoint(), &ToolCall::new("no-such-tool"))
            .await
            .expect_err("an error");
        match &err {
            ToolServerError::Remote { message, .. } => assert!(!message.is_empty()),
            other => panic!("expected the server's error, got {other:?}"),
        }
        assert_eq!(err.class(), ErrorClass::Invalid);
    })
    .await;
}

/// A result over [`MAX_RESULT_BYTES`] is cut and says so; one under it is not touched.
pub async fn a_result_over_the_bound_is_cut<F: ToolServerFixture>(f: F) {
    within(async {
        let endpoint = f.endpoint();
        let small = f
            .client()
            .call_tool(&endpoint, &call("big", json!({"bytes": 1000})))
            .await
            .expect("a result");
        assert!(!small.truncated);
        assert_eq!(small.text_of().len(), 1000);

        let big = f
            .client()
            .call_tool(
                &endpoint,
                &call("big", json!({"bytes": MAX_RESULT_BYTES + 10_000})),
            )
            .await
            .expect("a result");
        assert!(big.truncated, "the result was over the bound");
        assert!(!big.is_error);
        assert_eq!(big.text_of().len(), MAX_RESULT_BYTES);
        assert!(big.text_of().bytes().all(|b| b == b'x'));
    })
    .await;
}

/// No error, and no debug text of the endpoint, shows the bearer token, the value of a header or a
/// refused token: not when the server refuses, when nobody listens, nor when the call times out.
pub async fn no_error_or_debug_text_shows_a_secret<F: ToolServerFixture>(f: F) {
    within(async {
        let (_, header_value) = f.header();
        let secrets = [
            f.bearer().to_owned(),
            header_value.to_owned(),
            WRONG_BEARER.to_owned(),
        ];
        let wrong = ToolServerEndpoint {
            bearer: Some(ToolSecret::new(WRONG_BEARER)),
            ..f.endpoint()
        };
        let slow = f.endpoint().with_timeout(Duration::from_millis(300));
        let mut said = vec![
            format!("{:?}", f.endpoint()),
            format!("{:?}", f.endpoint().bearer),
            format!("{:?}", wrong),
        ];
        for (endpoint, call) in [
            (wrong, ToolCall::new("echo")),
            (f.unreachable_endpoint(), ToolCall::new("echo")),
            (slow, ToolCall::new("slow")),
        ] {
            said.push(everything_said_by(
                &f.client()
                    .call_tool(&endpoint, &call)
                    .await
                    .expect_err("a failure"),
            ));
            said.push(everything_said_by(
                &f.client()
                    .list_tools(&endpoint)
                    .await
                    .err()
                    .unwrap_or(ToolServerError::unreachable("listed")),
            ));
        }
        for text in said {
            for secret in &secrets {
                assert!(!text.contains(secret.as_str()), "{secret:?} is in: {text}");
            }
        }
    })
    .await;
}

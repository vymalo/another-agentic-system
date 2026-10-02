//! The tool-server port: the orchestrator as a client of an MCP server (ADR 0024, ADR 0009).
//!
//! A deployment lists the MCP servers a person may attach to a conversation (`toolServers`), and the
//! relay of the thread-tools endpoint (`thread-tools/v1`) calls them for the agent. This port is
//! what the relay asks of them: **list the tools** and **call one**. It speaks MCP's shapes (a tool
//! with an input schema, a result of content blocks that may say `isError`) as plain JSON values, so
//! no MCP library type appears in a signature and a test can stand in for the server without one.
//!
//! What a server answers is **untrusted text** (ADR 0024, `thread-tools/v1`): a tool's description,
//! the content of a result and the message of an error are written by someone else and go back into
//! an agent's model, so they are data to show and to pass on, never instructions to follow here.
//! The credentials are the opposite: they are the orchestrator's own, they appear on its requests
//! to the server and nowhere else. They are [`ToolSecret`]s, which print as `<redacted>`, and no
//! error of this module holds one.
//!
//! One server, one request: a client holds no session between calls (the orchestrator is stateless,
//! ADR 0001), and a call is **cancelled by dropping its future**.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use orch_core::{BoxError, Classify, ErrorClass};
use serde_json::{Map, Value};

/// The most content a call returns: 256 KiB, counting the text of a text block and the JSON of any
/// other block. A result over it is cut ([`ToolCallOutput::bounded`]) and says so
/// ([`ToolCallOutput::truncated`]).
pub const MAX_RESULT_BYTES: usize = 256 * 1024;

/// The most bytes of the message of a server's own error ([`ToolServerError::Remote`]): 1 KiB.
pub const MAX_REMOTE_MESSAGE_BYTES: usize = 1024;

/// A value that must not be printed: a bearer token, or the value of a header that carries a
/// credential. `Debug` shows `<redacted>`; there is no `Display`; [`expose`](ToolSecret::expose) is
/// the one way to read it, for the request that needs it.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolSecret(String);

impl ToolSecret {
    /// Wraps `value`.
    pub fn new(value: impl Into<String>) -> Self {
        ToolSecret(value.into())
    }

    /// The value, for the one request that sends it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ToolSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// Where a tool server is and how to talk to it: one entry of `toolServers`, resolved. `Debug` shows
/// the id, the URL, the names of the headers and the timeout, never a value.
///
/// A request to the server is `POST url` (MCP over streamable HTTP) with `Authorization: Bearer
/// <bearer>` when there is a bearer, and each of `headers` as given.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolServerEndpoint {
    /// The server's id in the configuration (`toolServers[].id`), for logs and for the names of
    /// its tools. Not part of any request.
    pub id: String,
    /// The server's MCP endpoint, `http` or `https`. It holds no credential (the configuration
    /// refuses one in it), and no error of this module repeats it.
    pub url: String,
    /// The bearer token the server wants, sent as `Authorization: Bearer <value>`.
    pub bearer: Option<ToolSecret>,
    /// Further headers the server wants: name and value, sent on every request.
    pub headers: Vec<(String, ToolSecret)>,
    /// How long one request to the server may take, **connecting included**: a call that has not
    /// answered by then is [`ToolServerError::TimedOut`] (`toolServers[].timeoutSecs`). A caller
    /// that wants another limit for one call clones the endpoint with
    /// [`with_timeout`](ToolServerEndpoint::with_timeout).
    pub timeout: Duration,
}

impl ToolServerEndpoint {
    /// An endpoint with no credential.
    pub fn new(id: impl Into<String>, url: impl Into<String>, timeout: Duration) -> Self {
        ToolServerEndpoint {
            id: id.into(),
            url: url.into(),
            bearer: None,
            headers: Vec::new(),
            timeout,
        }
    }

    /// The same endpoint with `bearer` as its bearer token.
    #[must_use]
    pub fn with_bearer(mut self, bearer: ToolSecret) -> Self {
        self.bearer = Some(bearer);
        self
    }

    /// The same endpoint that also sends the header `name: value`.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: ToolSecret) -> Self {
        self.headers.push((name.into(), value));
        self
    }

    /// The same endpoint with another timeout.
    #[must_use]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl fmt::Debug for ToolServerEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolServerEndpoint")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("bearer", &self.bearer)
            .field(
                "headers",
                &self
                    .headers
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// A tool a server offers, as it says it. All of it is untrusted text. The icons a server lists
/// are not here: only an icon of the configuration is drawn (open question 38).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDef {
    /// The server's own name for the tool.
    pub name: String,
    /// A title for people, when the server gives one.
    pub title: Option<String>,
    /// What the tool does, when the server says.
    pub description: Option<String>,
    /// The JSON Schema of the arguments, an object schema.
    pub input_schema: Map<String, Value>,
    /// The JSON Schema of the structured result, when the tool has one.
    pub output_schema: Option<Map<String, Value>>,
    /// The tool's annotations as the server wrote them (hints, never a guarantee).
    pub annotations: Option<Value>,
}

/// One call of a tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    /// The server's own name of the tool (not the relayed `<server>__<tool>`).
    pub name: String,
    /// The arguments, an object (empty when the tool takes none).
    pub arguments: Map<String, Value>,
    /// The request's `_meta` (MCP's metadata of a request): what the caller wants the server to see
    /// beside the arguments, for instance a call id for the server's own logs. Sent as given.
    pub meta: Option<Map<String, Value>>,
}

impl ToolCall {
    /// A call of `name` with no arguments and no metadata.
    pub fn new(name: impl Into<String>) -> Self {
        ToolCall {
            name: name.into(),
            arguments: Map::new(),
            meta: None,
        }
    }

    /// The same call with `arguments`.
    #[must_use]
    pub fn with_arguments(mut self, arguments: Map<String, Value>) -> Self {
        self.arguments = arguments;
        self
    }

    /// The same call with `meta` as its `_meta`.
    #[must_use]
    pub fn with_meta(mut self, meta: Map<String, Value>) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// What a call returned: the server's result, passed through. A tool that *failed* is an `Ok`
/// result with `is_error` (MCP's `isError`: the tool ran and says it went wrong), which the caller
/// passes on to the agent as it is; a call that could not be made at all is a [`ToolServerError`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCallOutput {
    /// MCP content blocks as the server sent them (`{"type": "text", "text": ...}` and the
    /// others), untrusted. At most [`MAX_RESULT_BYTES`] of them.
    pub content: Vec<Value>,
    /// The structured result, when the tool gave one.
    pub structured: Option<Value>,
    /// The server said the tool failed.
    pub is_error: bool,
    /// The content was cut at [`MAX_RESULT_BYTES`]: blocks after the cut are gone, a text block was
    /// shortened, and a structured result that did not fit was dropped.
    pub truncated: bool,
}

impl ToolCallOutput {
    /// A result of one text block.
    pub fn text(text: impl Into<String>) -> Self {
        ToolCallOutput {
            content: vec![serde_json::json!({"type": "text", "text": text.into()})],
            ..ToolCallOutput::default()
        }
    }

    /// The same result with `is_error` set: the tool failed, and said `text`.
    pub fn failed(text: impl Into<String>) -> Self {
        ToolCallOutput {
            is_error: true,
            ..ToolCallOutput::text(text)
        }
    }

    /// The text blocks of the content, in order, one per line.
    pub fn text_of(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| match block {
                Value::Object(block)
                    if block.get("type").and_then(Value::as_str) == Some("text") =>
                {
                    block.get("text").and_then(Value::as_str)
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The same result cut to `max_bytes` of content, counting the text of a text block and the
    /// JSON of any other block, and the structured result as its JSON. Blocks are kept whole while
    /// they fit; the first that does not is cut (a text block at a character boundary, any other
    /// dropped) and nothing after it is kept; a structured result that does not fit what is left
    /// is dropped. `truncated` says whether anything was cut. Every implementation applies this
    /// with [`MAX_RESULT_BYTES`] to what it returns.
    #[must_use]
    pub fn bounded(mut self, max_bytes: usize) -> Self {
        let mut left = max_bytes;
        let mut kept = Vec::with_capacity(self.content.len());
        let mut cut = false;
        for block in std::mem::take(&mut self.content) {
            if cut {
                continue;
            }
            let size = block_bytes(&block);
            if size <= left {
                left -= size;
                kept.push(block);
                continue;
            }
            cut = true;
            if let Value::Object(mut object) = block
                && object.get("type").and_then(Value::as_str) == Some("text")
                && let Some(Value::String(text)) = object.get("text")
            {
                let end = floor_char_boundary(text, left);
                let shorter = text[..end].to_owned();
                left -= shorter.len();
                object.insert("text".to_owned(), Value::String(shorter));
                kept.push(Value::Object(object));
            }
        }
        self.content = kept;
        if let Some(structured) = &self.structured {
            let size = structured.to_string().len();
            if cut || size > left {
                self.structured = None;
                cut = true;
            }
        }
        self.truncated |= cut;
        self
    }
}

/// What a block counts as against the bound: its text, or its JSON.
fn block_bytes(block: &Value) -> usize {
    match block {
        Value::Object(object) if object.get("type").and_then(Value::as_str) == Some("text") => {
            object
                .get("text")
                .and_then(Value::as_str)
                .map_or_else(|| block.to_string().len(), str::len)
        }
        other => other.to_string().len(),
    }
}

/// The largest index at most `index` that is a character boundary of `text`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut end = index.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

/// Why a server did not give a result.
///
/// `Unreachable` and `Protocol` describe this side: their `detail` is plain text without URLs or
/// secrets, and the transport error is their `source` (adapters box theirs: ADR 0009), which may
/// hold a URL and never reaches the chat log. None of them holds a credential.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ToolServerError {
    /// The server could not be reached or the connection broke (refused, unknown host, reset, a 5xx,
    /// no answer to the handshake in time). The call did not run, as far as this side knows.
    #[error("the tool server could not be reached: {detail}")]
    Unreachable {
        /// What went wrong, without URLs or secrets.
        detail: String,
        /// The transport error.
        #[source]
        source: Option<BoxError>,
    },
    /// The server refused the credentials (401 or 403), or wants some it was not given.
    #[error("the tool server refused the credentials")]
    Unauthenticated,
    /// The server had not answered within the endpoint's timeout. The call may still be running
    /// there.
    #[error("the tool server gave no answer within {after:?}")]
    TimedOut {
        /// The limit that was reached.
        after: Duration,
    },
    /// The server answered with a JSON-RPC error: an unknown tool, arguments it could not read,
    /// a failure of its own. Not a tool's `isError` (that is an `Ok` result).
    #[error("the tool server answered with an error ({code}): {message}")]
    Remote {
        /// The JSON-RPC error code.
        code: i32,
        /// The server's message, untrusted, at most [`MAX_REMOTE_MESSAGE_BYTES`].
        message: String,
    },
    /// The server answered something that is not MCP (an HTML page, a malformed message).
    #[error("the tool server broke the protocol: {detail}")]
    Protocol {
        /// What was unexpected, without URLs or secrets.
        detail: String,
        /// The lower error.
        #[source]
        source: Option<BoxError>,
    },
    /// The endpoint cannot be used as given (a URL that is not `http` or `https`, a header a request
    /// cannot carry). Nothing was sent.
    #[error("the tool server endpoint is not usable: {0}")]
    Misconfigured(String),
}

impl ToolServerError {
    /// The server could not be reached.
    pub fn unreachable(detail: impl Into<String>) -> Self {
        ToolServerError::Unreachable {
            detail: detail.into(),
            source: None,
        }
    }

    /// The server answered something unexpected.
    pub fn protocol(detail: impl Into<String>) -> Self {
        ToolServerError::Protocol {
            detail: detail.into(),
            source: None,
        }
    }

    /// A JSON-RPC error of the server's, its message cut to [`MAX_REMOTE_MESSAGE_BYTES`].
    pub fn remote(code: i32, message: &str) -> Self {
        let end = floor_char_boundary(message, MAX_REMOTE_MESSAGE_BYTES);
        ToolServerError::Remote {
            code,
            message: message[..end].to_owned(),
        }
    }

    /// Keeps `source` as the cause of an `Unreachable` or `Protocol` error; the other variants have
    /// no source and are returned unchanged.
    #[must_use]
    pub fn with_source(self, source: impl Into<BoxError>) -> Self {
        match self {
            ToolServerError::Unreachable { detail, .. } => ToolServerError::Unreachable {
                detail,
                source: Some(source.into()),
            },
            ToolServerError::Protocol { detail, .. } => ToolServerError::Protocol {
                detail,
                source: Some(source.into()),
            },
            other @ (ToolServerError::Unauthenticated
            | ToolServerError::TimedOut { .. }
            | ToolServerError::Remote { .. }
            | ToolServerError::Misconfigured(_)) => other,
        }
    }

    /// What may be shown to a person or put in a step: no URL, no credential, no transport error.
    /// For [`Remote`](ToolServerError::Remote) it is the server's own message, untrusted text.
    pub fn public_detail(&self) -> String {
        match self {
            ToolServerError::Unreachable { .. } => "the server could not be reached".to_owned(),
            ToolServerError::Unauthenticated => "the server refused the credentials".to_owned(),
            ToolServerError::TimedOut { after } => {
                format!("no answer within {} s", after.as_secs().max(1))
            }
            ToolServerError::Remote { message, .. } => message.clone(),
            ToolServerError::Protocol { .. } => "the server did not answer as MCP".to_owned(),
            ToolServerError::Misconfigured(_) => "the server is not set up for use".to_owned(),
        }
    }
}

impl Classify for ToolServerError {
    fn class(&self) -> ErrorClass {
        match self {
            ToolServerError::Unreachable { .. }
            | ToolServerError::TimedOut { .. }
            | ToolServerError::Protocol { .. } => ErrorClass::Transient,
            ToolServerError::Unauthenticated => ErrorClass::Unauthenticated,
            ToolServerError::Remote { .. } | ToolServerError::Misconfigured(_) => {
                ErrorClass::Invalid
            }
        }
    }
}

/// An MCP server as the orchestrator calls it.
///
/// An implementation holds nothing between calls that a replica could not rebuild, never retries (the
/// caller decides, by [`Classify::class`]), never caches a listing (a server's tools change, and
/// each request lists again) and never puts a credential into an error or a log line. Every
/// request is bounded by the endpoint's [`timeout`](ToolServerEndpoint::timeout).
pub trait ToolServerClient: Send + Sync + 'static {
    /// The tools the server offers now, all pages of them.
    ///
    /// # Errors
    /// [`ToolServerError`]; its class says whether asking again can help.
    fn list_tools(
        &self,
        server: &ToolServerEndpoint,
    ) -> impl Future<Output = Result<Vec<ToolDef>, ToolServerError>> + Send;

    /// Calls a tool. The result is the server's, passed through (`is_error` included) and cut at
    /// [`MAX_RESULT_BYTES`]; a server that answers with a JSON-RPC error is
    /// [`ToolServerError::Remote`], including for a tool it does not have. **Dropping the future
    /// cancels the call**: the request is abandoned, and the server may still run it to the end.
    ///
    /// # Errors
    /// [`ToolServerError`]; its class says whether asking again can help, and whether the call may
    /// have run is the tool's to say (`TimedOut`, and a connection that broke after the request
    /// was sent, are at least once on a retry).
    fn call_tool(
        &self,
        server: &ToolServerEndpoint,
        call: &ToolCall,
    ) -> impl Future<Output = Result<ToolCallOutput, ToolServerError>> + Send;
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn debug_shows_neither_the_bearer_nor_a_header_value() {
        let endpoint =
            ToolServerEndpoint::new("web", "https://mcp.example.com/mcp", Duration::from_secs(5))
                .with_bearer(ToolSecret::new("tok-very-secret"))
                .with_header("X-Api-Key", ToolSecret::new("key-very-secret"));
        let text = format!("{endpoint:?} {:?}", endpoint.bearer);
        assert!(!text.contains("very-secret"), "{text}");
        assert!(
            text.contains("X-Api-Key") && text.contains("<redacted>"),
            "{text}"
        );
        assert_eq!(
            endpoint.bearer.as_ref().unwrap().expose(),
            "tok-very-secret"
        );
    }

    #[test]
    fn errors_have_the_class_that_says_what_to_do() {
        let cases: [(ToolServerError, ErrorClass); 6] = [
            (
                ToolServerError::unreachable("refused"),
                ErrorClass::Transient,
            ),
            (
                ToolServerError::TimedOut {
                    after: Duration::from_secs(1),
                },
                ErrorClass::Transient,
            ),
            (ToolServerError::protocol("html"), ErrorClass::Transient),
            (
                ToolServerError::Unauthenticated,
                ErrorClass::Unauthenticated,
            ),
            (
                ToolServerError::remote(-32602, "unknown tool"),
                ErrorClass::Invalid,
            ),
            (
                ToolServerError::Misconfigured("url".into()),
                ErrorClass::Invalid,
            ),
        ];
        for (err, class) in cases {
            assert_eq!(err.class(), class, "{err}");
        }
    }

    #[test]
    fn a_remote_message_is_cut_at_a_character_boundary() {
        let long = "é".repeat(MAX_REMOTE_MESSAGE_BYTES);
        let ToolServerError::Remote { message, .. } = ToolServerError::remote(-1, &long) else {
            panic!("a remote error");
        };
        assert!(message.len() <= MAX_REMOTE_MESSAGE_BYTES);
        assert!(message.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_result_within_the_bound_is_untouched() {
        let output = ToolCallOutput::text("hello").bounded(5);
        assert_eq!(output, ToolCallOutput::text("hello"));
        assert!(!output.truncated);
    }

    #[test]
    fn a_text_block_is_cut_at_a_character_boundary_and_what_follows_is_dropped() {
        let output = ToolCallOutput {
            content: vec![
                json!({"type": "text", "text": "ééé"}),
                json!({"type": "text", "text": "never"}),
            ],
            structured: Some(json!({"a": 1})),
            ..ToolCallOutput::default()
        }
        .bounded(5);
        assert_eq!(output.text_of(), "éé");
        assert!(output.truncated);
        assert_eq!(output.content.len(), 1);
        assert_eq!(output.structured, None);
    }

    #[test]
    fn a_block_that_is_not_text_is_dropped_whole() {
        let image = json!({"type": "image", "data": "x".repeat(100), "mimeType": "image/png"});
        let output = ToolCallOutput {
            content: vec![json!({"type": "text", "text": "ok"}), image],
            ..ToolCallOutput::default()
        }
        .bounded(50);
        assert_eq!(output.content.len(), 1);
        assert!(output.truncated);
    }

    #[test]
    fn a_structured_result_that_does_not_fit_is_dropped() {
        let output = ToolCallOutput {
            content: vec![json!({"type": "text", "text": "ab"})],
            structured: Some(json!({"key": "a long enough value"})),
            ..ToolCallOutput::default()
        }
        .bounded(10);
        assert_eq!(output.text_of(), "ab");
        assert!(output.structured.is_none() && output.truncated);
    }
}

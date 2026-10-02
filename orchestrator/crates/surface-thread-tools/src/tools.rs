//! The built-in tools, a closed enum (ADR 0004): their names, schemas and what they do. Today
//! two, `get_ui_catalog` and `turn_output`; the tools of later slices come from providers
//! ([`ThreadToolProvider`](crate::ThreadToolProvider)).

use std::sync::Arc;

use orch_app::{App, AppError, ApplyOutcome};
use orch_core::{AnswerError, Classify, ErrorClass, MAX_ANSWER_BYTES, check_answer, report};
use orch_ports::Ports;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::ToolCtx;

/// What `get_ui_catalog` says of a thread that has none.
const NO_CATALOG: &str = "this thread has no UI catalog; answer in text";

/// What `turn_output` says when the answer cannot be announced any more: the thread is not
/// working, a later message started another turn, or the token belongs to an earlier job.
const TURN_OVER: &str = "this turn is over";

/// Every built-in tool. A new one is a new variant, so the compiler finds every place that must
/// know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadTool {
    /// The thread's current UI catalog (ADR 0023): the refetch seam.
    GetUiCatalog,
    /// The agent announces its answer for the turn (ADR 0031): shown to the person as the answer,
    /// whatever else the agent says in the turn.
    TurnOutput,
}

impl ThreadTool {
    /// Every built-in tool, in the order `tools/list` gives them (before any provider's).
    pub const ALL: &'static [ThreadTool] = &[ThreadTool::GetUiCatalog, ThreadTool::TurnOutput];

    /// The name on the wire.
    pub const fn name(self) -> &'static str {
        match self {
            ThreadTool::GetUiCatalog => "get_ui_catalog",
            ThreadTool::TurnOutput => "turn_output",
        }
    }

    /// The tool a wire name stands for.
    pub fn parse(name: &str) -> Option<ThreadTool> {
        Self::ALL.iter().copied().find(|t| t.name() == name)
    }

    const fn description(self) -> &'static str {
        match self {
            ThreadTool::GetUiCatalog => {
                "The UI catalog of this conversation: the components the person's screen can draw, \
                 in the newest version this thread has seen. Call it when your copy may be stale \
                 (a message names a digest you do not hold, you restarted) or when you were told \
                 the catalog was not sent inline. Pass knownDigest, the digest you hold, to learn \
                 cheaply whether it is still the newest: the answer then says unchanged and \
                 leaves the catalog out. A thread whose screen sent no catalog answers with an \
                 error; answer in text."
            }
            ThreadTool::TurnOutput => {
                "Announce your answer for this turn. Call it once the answer is ready, with the \
                 whole answer as Markdown in text: the person is shown it as your answer, and \
                 everything else you said in this turn (the sentences before a tool call, your \
                 closing words) is kept as working notes, not shown as the answer. It must be \
                 complete on its own, with the result first. Then finish with one short line. A \
                 later call in the same turn replaces the answer. text is 1 to 65536 bytes. A \
                 call after the turn is over is an error."
            }
        }
    }

    fn input_schema(self) -> Arc<JsonObject> {
        let schema = match self {
            ThreadTool::GetUiCatalog => json!({
                "type": "object",
                "properties": {
                    "knownDigest": {
                        "type": "string",
                        "pattern": "^sha256:[0-9a-f]{64}$",
                        "description": "The digest of the catalog you already hold."
                    }
                },
                "additionalProperties": false
            }),
            ThreadTool::TurnOutput => json!({
                "type": "object",
                "properties": {
                    "text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Your answer for this turn, as Markdown: 1 to 65536 bytes."
                    }
                },
                "required": ["text"],
                "additionalProperties": false
            }),
        };
        object(schema)
    }

    fn output_schema(self) -> Arc<JsonObject> {
        let schema = match self {
            ThreadTool::GetUiCatalog => json!({
                "type": "object",
                "properties": {
                    "catalogId": {"type": "string"},
                    "version": {"type": "integer", "minimum": 1},
                    "digest": {"type": "string", "pattern": "^sha256:[0-9a-f]{64}$"},
                    "unchanged": {"type": "boolean"},
                    "catalog": {"type": "object"}
                },
                "required": ["catalogId", "version", "digest", "unchanged"],
                "additionalProperties": false
            }),
            ThreadTool::TurnOutput => json!({
                "type": "object",
                "properties": {"delivered": {"type": "boolean", "const": true}},
                "required": ["delivered"],
                "additionalProperties": false
            }),
        };
        object(schema)
    }

    fn annotations(self) -> ToolAnnotations {
        match self {
            ThreadTool::GetUiCatalog => ToolAnnotations::new()
                .read_only(true)
                .idempotent(true)
                .open_world(false),
            // A second call replaces the answer: it changes what the person is shown, and is not
            // idempotent.
            ThreadTool::TurnOutput => ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(false),
        }
    }

    /// The definition `tools/list` gives.
    pub fn definition(self) -> Tool {
        Tool::new(self.name(), self.description(), self.input_schema())
            .with_raw_output_schema(self.output_schema())
            .annotate(self.annotations())
    }

    /// Runs the tool for the call `ctx` describes.
    ///
    /// # Errors
    ///
    /// A JSON-RPC error for arguments that do not parse (`-32602`) and for a fault of the server;
    /// a failure the caller can read is a result with `is_error` instead.
    pub(crate) async fn call<P: Ports>(
        self,
        app: &App<P>,
        ctx: &ToolCtx,
        args: Option<JsonObject>,
    ) -> Result<CallToolResult, ErrorData> {
        match self {
            ThreadTool::GetUiCatalog => get_ui_catalog(app, ctx, args).await,
            ThreadTool::TurnOutput => turn_output(app, ctx, args).await,
        }
    }
}

fn object(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        // The schemas above are written as objects; a test of `tools/list` reads each of them.
        _ => Arc::new(JsonObject::new()),
    }
}

/// The arguments of `get_ui_catalog`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GetUiCatalogArgs {
    known_digest: Option<String>,
}

/// `sha256:` and 64 lowercase hexadecimal characters.
fn is_digest(text: &str) -> bool {
    text.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

fn parse_args(arguments: Option<JsonObject>) -> Result<GetUiCatalogArgs, ErrorData> {
    let args: GetUiCatalogArgs =
        serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
            .map_err(|e| ErrorData::invalid_params(format!("invalid arguments: {e}"), None))?;
    if args.known_digest.as_deref().is_some_and(|d| !is_digest(d)) {
        return Err(ErrorData::invalid_params(
            "invalid arguments: knownDigest must be sha256: and 64 lowercase hexadecimal characters",
            None,
        ));
    }
    Ok(args)
}

/// A failure of the tool itself, for the caller to read.
fn refused(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

async fn get_ui_catalog<P: Ports>(
    app: &App<P>,
    ctx: &ToolCtx,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, ErrorData> {
    let args = parse_args(arguments)?;
    let thread = ctx.claims.thread;
    let catalog = match app.thread_ui_catalog(thread).await {
        Ok(Some(catalog)) => catalog,
        Ok(None) => return Ok(refused(NO_CATALOG)),
        Err(e) => {
            return match e.class() {
                ErrorClass::Conflict | ErrorClass::Transient | ErrorClass::RateLimited => {
                    tracing::warn!(%thread, error = %report(&e), "get_ui_catalog failed temporarily");
                    Ok(refused("temporarily unavailable; try again"))
                }
                _ => {
                    tracing::error!(%thread, error = %report(&e), class = ?e.class(), "get_ui_catalog failed");
                    Err(ErrorData::internal_error("internal error", None))
                }
            };
        }
    };
    let unchanged = args.known_digest.as_deref() == Some(catalog.digest.as_str());
    let mut result = json!({
        "catalogId": catalog.catalog_id,
        "version": catalog.version,
        "digest": catalog.digest,
        "unchanged": unchanged,
    });
    if !unchanged && let Some(object) = result.as_object_mut() {
        object.insert("catalog".to_owned(), catalog.catalog);
    }
    // The same JSON as structured content and as one text content, as MCP asks.
    Ok(CallToolResult::structured(result))
}

/// The arguments of `turn_output`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TurnOutputArgs {
    text: String,
}

/// Why a text is not an answer, worded for the model that wrote it.
fn answer_problem(error: AnswerError) -> String {
    match error {
        AnswerError::Empty => "text must not be empty".to_owned(),
        AnswerError::TooLong => format!("text must be at most {MAX_ANSWER_BYTES} bytes"),
        // `AnswerError` is non-exhaustive.
        other => other.to_string(),
    }
}

async fn turn_output<P: Ports>(
    app: &App<P>,
    ctx: &ToolCtx,
    arguments: Option<JsonObject>,
) -> Result<CallToolResult, ErrorData> {
    let args: TurnOutputArgs = serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|e| ErrorData::invalid_params(format!("invalid arguments: {e}"), None))?;
    if let Err(e) = check_answer(&args.text) {
        return Ok(refused(answer_problem(e)));
    }
    let claims = &ctx.claims;
    // An asked agent works for the agent that asked it: the answer to the person is the
    // addressed agent's to announce.
    if !claims.caller.is_main() {
        return Ok(refused(
            "only the agent the conversation is addressed to announces its answer",
        ));
    }
    let thread = claims.thread;
    match app
        .record_answer(
            thread,
            &claims.agent,
            claims.job,
            &claims.message_id,
            args.text,
        )
        .await
    {
        Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => {
            Ok(CallToolResult::structured(json!({"delivered": true})))
        }
        Ok(ApplyOutcome::Fenced) => {
            tracing::error!(%thread, "turn_output was fenced, which only a claimed row can be");
            Err(ErrorData::internal_error("internal error", None))
        }
        Err(AppError::Transition(_) | AppError::NotFound) => Ok(refused(TURN_OVER)),
        Err(AppError::Invalid(message)) => Ok(refused(message)),
        Err(e) => match e.class() {
            ErrorClass::Conflict | ErrorClass::Transient | ErrorClass::RateLimited => {
                tracing::warn!(%thread, error = %report(&e), "turn_output failed temporarily");
                Ok(refused("temporarily unavailable; try again"))
            }
            _ => {
                tracing::error!(%thread, error = %report(&e), class = ?e.class(), "turn_output failed");
                Err(ErrorData::internal_error("internal error", None))
            }
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn every_tool_has_a_name_that_parses_back_and_object_schemas() {
        for tool in ThreadTool::ALL {
            assert_eq!(ThreadTool::parse(tool.name()), Some(*tool));
            let definition = tool.definition();
            assert_eq!(definition.name, tool.name());
            assert_eq!(definition.input_schema.get("type"), Some(&json!("object")));
            let output = definition.output_schema.expect("an output schema");
            assert_eq!(output.get("type"), Some(&json!("object")));
        }
        assert_eq!(ThreadTool::parse("get_ui_catalogs"), None);
        assert_eq!(
            ThreadTool::ALL.iter().map(|t| t.name()).collect::<Vec<_>>(),
            ["get_ui_catalog", "turn_output"]
        );
        assert_eq!(ThreadTool::parse(""), None);
    }

    #[test]
    fn a_digest_is_sha256_and_lowercase_hex() {
        let good = format!("sha256:{}", "0123456789abcdef".repeat(4));
        assert!(is_digest(&good));
        for bad in [
            "",
            "sha256:",
            &good.to_uppercase(),
            &format!("sha256:{}", "a".repeat(63)),
            &format!("sha256:{}", "a".repeat(65)),
            &format!("sha512:{}", "a".repeat(64)),
            &format!("sha256:{}g", "a".repeat(63)),
        ] {
            assert!(!is_digest(bad), "{bad:?}");
        }
    }

    #[test]
    fn arguments_are_the_one_optional_digest_and_nothing_else() {
        assert!(parse_args(None).is_ok());
        assert!(parse_args(Some(JsonObject::new())).is_ok());
        let digest = format!("sha256:{}", "a".repeat(64));
        let with = |v: Value| match v {
            Value::Object(m) => Some(m),
            _ => None,
        };
        assert!(parse_args(with(json!({"knownDigest": digest}))).is_ok());
        for bad in [
            json!({"knownDigest": "sha256:abc"}),
            json!({"knownDigest": 7}),
            json!({"known_digest": digest}),
            json!({"knownDigest": digest, "extra": true}),
        ] {
            assert!(parse_args(with(bad.clone())).is_err(), "{bad}");
        }
    }
}

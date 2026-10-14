//! `request_upload`: the addressed agent asks for a presigned `PUT` for a file too large for the
//! message ([ADR 0058](../../../../docs/decisions/0058-large-files-go-up-through-a-presigned-url-the-orchestrator-gives-the-agent.md),
//! [`docs/api/uploads-v1.md`](../../../../docs/api/uploads-v1.md#3-the-tool-request_upload)).
//!
//! A [`ThreadToolProvider`] like `ask_agent`: whether the tool exists depends on the deployment (a
//! limit configured, a store that can presign) and on the agent (its live card lists `uploads/v1`
//! and `thread-tools/v1`), not on the endpoint, so it is not one of the closed built-ins. The rules
//! are the application's ([`App::request_upload`]); this is the tool's face:
//!
//! * **`tools/list`** offers it to the thread's addressed agent only while the deployment offers
//!   uploads and the agent's **live card** lists both extensions (read for this request, never
//!   cached: ADR 0008). An asked agent is offered nothing.
//! * **`tools/call`** answers with a result. Every failure is a **tool error**
//!   (`isError: true`), never a protocol error, whose text starts with the contract's code
//!   (`not_activated`, `too_large`, `over_limit`, `unsupported`, `bad_request`,
//!   `caller_not_allowed`, then the tool's own `task_over` and `unavailable`): a model reads the code
//!   and the reason. A call is bounded to [`CALL_TIMEOUT`].
//! * **The answer is a secret for its minutes.** The URL opens one key for one `PUT`; nothing here
//!   logs it, and the result goes to the agent alone (`structuredContent` and the same JSON as one
//!   text content).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use orch_app::{App, UploadCaller, UploadRefusal, UploadRequest};
use orch_ports::Ports;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde_json::{Value, json};

use crate::{ThreadToolProvider, ToolCtx};

/// The name of the tool.
pub const UPLOAD_TOOL_NAME: &str = "request_upload";

/// How long a call may take before it is cut off: a thread read, a card read and a local signature.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The provider of `request_upload`. Built by the composition root; asked for each request, it keeps
/// nothing between calls.
pub struct UploadTools<P: Ports> {
    app: Arc<App<P>>,
}

impl<P: Ports> fmt::Debug for UploadTools<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UploadTools").finish_non_exhaustive()
    }
}

impl<P: Ports> UploadTools<P> {
    /// The tool over `app`, whose configuration says the limits ([`App::upload_offer`]).
    pub fn new(app: Arc<App<P>>) -> Self {
        UploadTools { app }
    }
}

/// The definition `tools/list` gives.
fn definition() -> Tool {
    let input = json!({
        "type": "object",
        "properties": {
            "filename": {
                "type": "string",
                "minLength": 1,
                "maxLength": 255,
                "description": "The file's name, one name: no path."
            },
            "mediaType": {
                "type": "string",
                "description": "The file's media type, type/subtype, like application/pdf."
            },
            "size": {
                "type": "integer",
                "minimum": 1,
                "description": "The file's size in bytes, from 1 to the most the message said (maxBytes)."
            },
            "sha256": {
                "type": "string",
                "pattern": "^[0-9a-f]{64}$",
                "description": "The SHA-256 of the file's content, 64 lower-case hexadecimal digits."
            }
        },
        "required": ["filename", "mediaType", "size", "sha256"],
        "additionalProperties": false
    });
    let output = json!({
        "type": "object",
        "properties": {
            "uploadId": {"type": "string"},
            "url": {"type": "string"},
            "method": {"type": "string", "const": "PUT"},
            "headers": {"type": "object", "additionalProperties": {"type": "string"}},
            "expiresAt": {"type": "string"}
        },
        "required": ["uploadId", "url", "method", "headers", "expiresAt"],
        "additionalProperties": false
    });
    Tool::new(
        UPLOAD_TOOL_NAME,
        "Ask for a one-time upload slot for a file too large to send in the message. Give the \
         file's name, media type, size and SHA-256. The answer is a presigned PUT: send exactly the \
         method, the url, every header listed (with exactly those values) and the file's bytes, \
         before expiresAt, then hand the uploadId back in the artifact that names the file (a part \
         whose metadata under the uploads/v1 URI holds the uploadId). The url is a credential for \
         a few minutes: keep it out of your prompts and logs. Use this only when the message \
         activated uploads/v1, and only for a file larger than the message can carry. The answer \
         can be a refusal with a code: too_large (over maxBytes), over_limit (the conversation's \
         limit of files or bytes), not_activated, unsupported, bad_request, or unavailable (the \
         thread, the store or your card could not be read just now: try again).",
        object(&input),
    )
    .with_raw_output_schema(object(&output))
    .annotate(
        ToolAnnotations::new()
            .read_only(false)
            .destructive(false)
            .idempotent(true)
            .open_world(false),
    )
}

fn object(value: &Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map.clone()),
        // the schemas above are written as objects; a test of `tools/list` reads each of them
        _ => Arc::new(JsonObject::new()),
    }
}

/// A failure of the tool for the model to read: `<code>: <reason>`.
fn refusal(refusal: &UploadRefusal) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!(
        "{}: {refusal}",
        refusal.code()
    ))])
}

/// The arguments of the call, read as the schema says; a missing or mistyped member, an unknown one,
/// a name, a type, a size or a hash the contract refuses are all `bad_request`, a tool error.
fn parse(arguments: &JsonObject) -> Result<UploadRequest, UploadRefusal> {
    let bad = |what: &str| Err(UploadRefusal::BadRequest(what.to_owned()));
    if let Some(unknown) = arguments
        .keys()
        .find(|k| !matches!(k.as_str(), "filename" | "mediaType" | "size" | "sha256"))
    {
        return Err(UploadRefusal::BadRequest(format!(
            "unknown member {unknown:?}: the members are filename, mediaType, size and sha256"
        )));
    }
    let Some(filename) = arguments.get("filename").and_then(Value::as_str) else {
        return bad("filename is required and is a string");
    };
    let Some(media_type) = arguments.get("mediaType").and_then(Value::as_str) else {
        return bad("mediaType is required and is a string");
    };
    let Some(size) = arguments.get("size").and_then(Value::as_u64) else {
        return bad("size is required and is a whole number of bytes");
    };
    let Some(sha256) = arguments.get("sha256").and_then(Value::as_str) else {
        return bad("sha256 is required and is a string");
    };
    UploadRequest::parse(filename, media_type, size, sha256)
}

impl<P: Ports> ThreadToolProvider for UploadTools<P> {
    async fn list(&self, ctx: &ToolCtx) -> Vec<Tool> {
        // The tool is for the thread's addressed agent, while the deployment offers uploads and the
        // agent's live card lists the extension it rides on.
        if !ctx.claims.caller.is_main()
            || self.app.upload_offer().is_none()
            || !self.app.uploads_listed(&ctx.claims.agent).await
        {
            return Vec::new();
        }
        vec![definition()]
    }

    async fn call(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: JsonObject,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        if name != UPLOAD_TOOL_NAME {
            return None;
        }
        let caller = UploadCaller {
            thread: ctx.claims.thread,
            job: ctx.claims.job,
            agent: ctx.claims.agent.clone(),
            caller: ctx.claims.caller,
        };
        let outcome = async {
            if !caller.caller.is_main() {
                return Err(UploadRefusal::CallerNotAllowed);
            }
            let request = parse(&args)?;
            self.app.request_upload(&caller, &request).await
        };
        let outcome = match tokio::time::timeout(CALL_TIMEOUT, outcome).await {
            Ok(outcome) => outcome,
            Err(_) => {
                tracing::warn!(thread = %ctx.claims.thread, "request_upload took too long and was cut off");
                Err(UploadRefusal::Unavailable)
            }
        };
        Some(Ok(match outcome {
            Ok(slot) => CallToolResult::structured(json!({
                "uploadId": slot.upload_id,
                "url": slot.url,
                "method": slot.method,
                "headers": slot.headers,
                "expiresAt": slot.expires_at.to_string(),
            })),
            Err(why) => refusal(&why),
        }))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn args(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    const HASH: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn the_definition_is_an_object_schema_that_names_the_four_members() {
        let tool = definition();
        assert_eq!(tool.name, UPLOAD_TOOL_NAME);
        assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
        assert_eq!(
            tool.input_schema.get("required"),
            Some(&json!(["filename", "mediaType", "size", "sha256"]))
        );
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false))
        );
        let output = tool.output_schema.expect("an output schema");
        assert_eq!(output.get("type"), Some(&json!("object")));
    }

    #[test]
    fn arguments_are_the_four_members_and_every_other_shape_is_a_bad_request() {
        let ok = parse(&args(json!({
            "filename": "a.pdf", "mediaType": "application/pdf", "size": 5, "sha256": HASH
        })))
        .unwrap();
        assert_eq!((ok.filename.as_str(), ok.size), ("a.pdf", 5));
        for bad in [
            json!({}),
            json!({"filename": "a.pdf", "mediaType": "application/pdf", "size": 5}),
            json!({"filename": 7, "mediaType": "application/pdf", "size": 5, "sha256": HASH}),
            json!({"filename": "a", "mediaType": "application/pdf", "size": "5", "sha256": HASH}),
            json!({"filename": "a", "mediaType": "application/pdf", "size": -1, "sha256": HASH}),
            json!({"filename": "a", "mediaType": "application/pdf", "size": 5.5, "sha256": HASH}),
            json!({"filename": "a", "mediaType": "application/pdf", "size": 0, "sha256": HASH}),
            json!({"filename": "a", "mediaType": "pdf", "size": 5, "sha256": HASH}),
            json!({"filename": "a", "mediaType": "application/pdf", "size": 5, "sha256": "x"}),
            json!({"filename": "a", "mediaType": "application/pdf", "size": 5, "sha256": HASH, "extra": 1}),
        ] {
            let err = parse(&args(bad.clone())).unwrap_err();
            assert!(matches!(err, UploadRefusal::BadRequest(_)), "{bad}: {err}");
        }
    }

    #[test]
    fn a_refusal_is_its_code_then_its_reason() {
        let result = refusal(&UploadRefusal::TooLarge { max_bytes: 1024 });
        assert_eq!(result.is_error, Some(true));
        let ContentBlock::Text(text) = &result.content[0] else {
            panic!("{:?}", result.content)
        };
        assert!(text.text.starts_with("too_large: "), "{}", text.text);
        assert!(text.text.contains("1024"), "{}", text.text);
    }
}

//! The window tools: `window_open`, `window_look`, `window_act` and `window_close`
//! ([ADR 0092](../../../../docs/decisions/0092-a-window-is-a-port-on-the-runs-side-and-the-orchestrator-serves-its-tools.md),
//! [`docs/api/windows-v1.md`](../../../../docs/api/windows-v1.md)), over the
//! [`Window`](orch_window::Window) port.
//!
//! **A provider, not a variant of [`ThreadTool`](crate::ThreadTool).** ADR 0092 decision 2 says the
//! tools "join the closed enum". They are a [`ThreadToolProvider`] instead, like `request_upload`:
//! the built-in enum's `call` takes only the application, and the tools need the adapter, a type
//! the composition root picks. The provider is part of this surface, mounted by the binary only
//! where windows are served, and a thread's endpoint lists the tools only then, which is the
//! "windows off" of the contract (so `windows_off` is never answered from here: with no provider
//! the name is unknown, `-32602`).
//!
//! * **`tools/list`** offers the four tools to the thread's addressed agent only. An asked agent is
//!   offered none.
//! * **`tools/call`** answers every failure as a tool error (`isError: true`) whose text starts
//!   with the contract's code, then a reason: `not_main_caller`, `window_unknown`, `url_refused`,
//!   `too_many_windows`, `invalid`, and the tool's own `unavailable` (a call cut off after
//!   [`CALL_TIMEOUT`]). The caller is checked **before the arguments are read and before any
//!   adapter call**: an asked agent learns nothing about URLs or windows.
//! * **What is not here.** The tier of the message (access tier, network tier) is not read: the
//!   thread token carries neither and the orchestrator holds no network tier at all (ADR 0062 is
//!   accepted and unbuilt). The call is not recorded as a step of the thread by the orchestrator
//!   (ADR 0092 decision 2 asks for it); see `docs/api/windows-v1.md`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use orch_core::ThreadId;
use orch_window::{
    Action, Frame, MAX_ACTIONS_PER_CALL, MAX_URL_BYTES, Window, WindowError, WindowId,
};
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{ThreadToolProvider, ToolCtx};

/// The name of `window_open`.
pub const WINDOW_OPEN: &str = "window_open";
/// The name of `window_look`.
pub const WINDOW_LOOK: &str = "window_look";
/// The name of `window_act`.
pub const WINDOW_ACT: &str = "window_act";
/// The name of `window_close`.
pub const WINDOW_CLOSE: &str = "window_close";

/// How long a call may take before it is cut off. The adapter is the run's browser, which may be
/// slow on a page; a frame is one picture, so half a minute is the bound the built-ins have too.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// What a failure's reason is cut to when it quotes a parser, in characters: the model wrote the
/// arguments, so it is told what was wrong, not handed a megabyte back.
const MAX_REASON_CHARS: usize = 200;

/// The four tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Open,
    Look,
    Act,
    Close,
}

impl Which {
    const ALL: [Which; 4] = [Which::Open, Which::Look, Which::Act, Which::Close];

    const fn name(self) -> &'static str {
        match self {
            Which::Open => WINDOW_OPEN,
            Which::Look => WINDOW_LOOK,
            Which::Act => WINDOW_ACT,
            Which::Close => WINDOW_CLOSE,
        }
    }

    fn parse(name: &str) -> Option<Which> {
        Which::ALL.into_iter().find(|w| w.name() == name)
    }

    const fn description(self) -> &'static str {
        match self {
            Which::Open => {
                "Open a window on a web page and get its first frame. Give the page's http(s) URL. \
                 A page that is private, on loopback or link-local, or not http(s), is refused \
                 (url_refused). A conversation holds at most 4 windows (too_many_windows). The \
                 answer is the window's id and a frame: a picture handed over by reference \
                 (frame.url is artifact:<id>), never the bytes. A window closes itself after 15 \
                 minutes alone."
            }
            Which::Look => {
                "Look at a window: one frame, the picture of the page as it is now. A window that \
                 was closed or has expired answers frame null and state closed."
            }
            Which::Act => {
                "Act on a window with a short script of at most 8 actions, in order, and get the \
                 frame after them. An action is exactly one of {\"click\": {\"x\", \"y\"}}, \
                 {\"type\": {\"text\"}}, {\"scroll\": {\"dy\"}} (negative is up) or \
                 {\"key\": {\"name\"}}. More than 8 is refused (invalid), never cut short. A \
                 window this conversation has not got, or that expired, is window_unknown."
            }
            Which::Close => {
                "Close a window. It is safe to call again: an id that is not open answers \
                 closed false and is not an error."
            }
        }
    }

    fn input_schema(self) -> Value {
        match self {
            Which::Open => json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "minLength": 1,
                        "maxLength": MAX_URL_BYTES,
                        "description": "The page's http or https URL, at most 2048 bytes."
                    },
                    "label": {
                        "type": "string",
                        "description": "What the window is for, in a few words (for the person reading the log)."
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            }),
            Which::Look | Which::Close => json!({
                "type": "object",
                "properties": {
                    "window": {"type": "string", "minLength": 1, "description": "The window's id, as window_open answered it."}
                },
                "required": ["window"],
                "additionalProperties": false
            }),
            Which::Act => json!({
                "type": "object",
                "properties": {
                    "window": {"type": "string", "minLength": 1, "description": "The window's id, as window_open answered it."},
                    "actions": {
                        "type": "array",
                        "maxItems": MAX_ACTIONS_PER_CALL,
                        "description": "The script, in order: each item is exactly one of click, type, scroll, key.",
                        "items": {
                            "type": "object",
                            "minProperties": 1,
                            "maxProperties": 1,
                            "properties": {
                                "click": {
                                    "type": "object",
                                    "properties": {
                                        "x": {"type": "integer", "minimum": 0},
                                        "y": {"type": "integer", "minimum": 0}
                                    },
                                    "required": ["x", "y"]
                                },
                                "type": {
                                    "type": "object",
                                    "properties": {"text": {"type": "string"}},
                                    "required": ["text"]
                                },
                                "scroll": {
                                    "type": "object",
                                    "properties": {"dy": {"type": "integer"}},
                                    "required": ["dy"]
                                },
                                "key": {
                                    "type": "object",
                                    "properties": {"name": {"type": "string"}},
                                    "required": ["name"]
                                }
                            },
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["window", "actions"],
                "additionalProperties": false
            }),
        }
    }

    fn output_schema(self) -> Value {
        let frame = json!({
            "type": "object",
            "properties": {
                "mediaType": {"type": "string"},
                "width": {"type": "integer"},
                "height": {"type": "integer"},
                "scale": {"type": "integer"},
                "url": {"type": "string"},
                "at": {"type": "string"}
            },
            "required": ["mediaType", "width", "height", "scale", "url", "at"]
        });
        match self {
            Which::Open => json!({
                "type": "object",
                "properties": {"window": {"type": "string"}, "frame": frame},
                "required": ["window", "frame"],
                "additionalProperties": false
            }),
            Which::Look => json!({
                "type": "object",
                "properties": {
                    "frame": {"oneOf": [frame, {"type": "null"}]},
                    "state": {"type": "string", "const": "closed"}
                },
                "required": ["frame"],
                "additionalProperties": false
            }),
            Which::Act => json!({
                "type": "object",
                "properties": {"frame": frame},
                "required": ["frame"],
                "additionalProperties": false
            }),
            Which::Close => json!({
                "type": "object",
                "properties": {"closed": {"type": "boolean"}},
                "required": ["closed"],
                "additionalProperties": false
            }),
        }
    }

    fn annotations(self) -> ToolAnnotations {
        match self {
            Which::Open => ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(true),
            Which::Look => ToolAnnotations::new()
                .read_only(true)
                .idempotent(true)
                .open_world(false),
            // A click or a key can submit a form: the page decides what it does.
            Which::Act => ToolAnnotations::new()
                .read_only(false)
                .destructive(true)
                .idempotent(false)
                .open_world(true),
            Which::Close => ToolAnnotations::new()
                .read_only(false)
                .destructive(false)
                .idempotent(true)
                .open_world(false),
        }
    }

    fn definition(self) -> Tool {
        Tool::new(
            self.name(),
            self.description(),
            object(&self.input_schema()),
        )
        .with_raw_output_schema(object(&self.output_schema()))
        .annotate(self.annotations())
    }
}

fn object(value: &Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map.clone()),
        // the schemas above are written as objects; a test of `tools/list` reads each of them
        _ => Arc::new(JsonObject::new()),
    }
}

/// A failure of the tool for the model to read: `<code>: <reason>`.
fn refused(code: &str, reason: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(format!("{code}: {reason}"))])
}

/// A refusal by the adapter or the guard, in the contract's code and a reason for the model.
fn failure(error: &WindowError) -> CallToolResult {
    let reason = match error {
        WindowError::WindowsOff => "this deployment serves no windows",
        WindowError::NotMainCaller => {
            "only the agent the conversation is addressed to may use windows"
        }
        WindowError::WindowUnknown => {
            "this conversation has no such window (it was closed, or it expired)"
        }
        WindowError::UrlRefused => {
            "the URL is not one a window may open: it must be http or https, at most 2048 bytes, and name a public address"
        }
        WindowError::TooManyWindows => {
            "this conversation already has 4 windows open; close one first"
        }
        WindowError::Invalid => "the call is outside what a window takes",
    };
    refused(&error.to_string(), reason)
}

/// The arguments of the call, read as the schema says; a missing or mistyped member and an
/// unknown one are `invalid`, a tool error.
fn parse<T: DeserializeOwned>(args: JsonObject) -> Result<T, CallToolResult> {
    serde_json::from_value(Value::Object(args)).map_err(|e| {
        let detail: String = e.to_string().chars().take(MAX_REASON_CHARS).collect();
        refused("invalid", &format!("arguments: {detail}"))
    })
}

/// The arguments of `window_open`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenArgs {
    url: String,
    #[serde(default)]
    label: Option<String>,
}

/// The arguments of `window_look` and `window_close`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowArgs {
    window: String,
}

/// The arguments of `window_act`. An action outside the closed set does not parse: `invalid`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActArgs {
    window: String,
    actions: Vec<Action>,
}

/// A frame as the contract writes it. The bytes are never in it: the frame is an artifact reference.
fn frame_json(frame: &Frame) -> Value {
    serde_json::to_value(frame).unwrap_or(Value::Null)
}

/// The provider of the four window tools, over one adapter. Built by the composition root; asked
/// for each request, it keeps nothing between calls: a window's state is the adapter's (the run's,
/// ADR 0092 decision 1).
pub struct WindowTools<W: Window> {
    window: Arc<W>,
}

impl<W: Window> fmt::Debug for WindowTools<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WindowTools").finish_non_exhaustive()
    }
}

impl<W: Window> WindowTools<W> {
    /// The tools over `window`. The adapter owes the URL guard on the resolved addresses
    /// ([`orch_window::check_url`]); this layer only bounds what it passes on.
    pub fn new(window: Arc<W>) -> Self {
        WindowTools { window }
    }

    async fn run(&self, thread: ThreadId, which: Which, args: JsonObject) -> CallToolResult {
        match which {
            Which::Open => {
                let args: OpenArgs = match parse(args) {
                    Ok(args) => args,
                    Err(refusal) => return refusal,
                };
                // The label is for the person reading the log; the port has no place for it.
                if args
                    .label
                    .as_deref()
                    .is_some_and(|l| l.len() > MAX_REASON_CHARS)
                {
                    return refused("invalid", "label is at most 200 bytes");
                }
                // Bounded here so that an enormous string never reaches an adapter; the guard on
                // the resolved addresses is the adapter's.
                if args.url.len() > MAX_URL_BYTES {
                    return failure(&WindowError::UrlRefused);
                }
                match self.window.open(thread, &args.url).await {
                    Ok(opened) => CallToolResult::structured(json!({
                        "window": opened.id.0,
                        "frame": frame_json(&opened.frame),
                    })),
                    Err(error) => failure(&error),
                }
            }
            Which::Look => {
                let args: WindowArgs = match parse(args) {
                    Ok(args) => args,
                    Err(refusal) => return refusal,
                };
                match self.window.look(thread, &WindowId(args.window)).await {
                    Ok(Some(frame)) => CallToolResult::structured(json!({
                        "frame": frame_json(&frame),
                    })),
                    // closed or expired: an answer, not an error
                    Ok(None) => CallToolResult::structured(json!({
                        "frame": null,
                        "state": "closed",
                    })),
                    Err(error) => failure(&error),
                }
            }
            Which::Act => {
                let args: ActArgs = match parse(args) {
                    Ok(args) => args,
                    Err(refusal) => return refusal,
                };
                // over the bound is `invalid`, never a truncation (windows-v1)
                if args.actions.len() > MAX_ACTIONS_PER_CALL {
                    return refused(
                        "invalid",
                        &format!("at most {MAX_ACTIONS_PER_CALL} actions in one call"),
                    );
                }
                match self
                    .window
                    .act(thread, &WindowId(args.window), &args.actions)
                    .await
                {
                    Ok(frame) => CallToolResult::structured(json!({
                        "frame": frame_json(&frame),
                    })),
                    Err(error) => failure(&error),
                }
            }
            Which::Close => {
                let args: WindowArgs = match parse(args) {
                    Ok(args) => args,
                    Err(refusal) => return refusal,
                };
                match self.window.close(thread, &WindowId(args.window)).await {
                    Ok(closed) => CallToolResult::structured(json!({"closed": closed})),
                    Err(error) => failure(&error),
                }
            }
        }
    }
}

impl<W: Window> ThreadToolProvider for WindowTools<W> {
    async fn list(&self, ctx: &ToolCtx) -> Vec<Tool> {
        // The tools are the addressed agent's: an asked agent works for the agent that asked it.
        if !ctx.claims.caller.is_main() {
            return Vec::new();
        }
        Which::ALL.into_iter().map(Which::definition).collect()
    }

    async fn call(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: JsonObject,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        let which = Which::parse(name)?;
        // First of everything, and before the arguments are read: a caller that is not the main
        // one learns nothing about URLs or windows, and no adapter is called.
        if !ctx.claims.caller.is_main() {
            return Some(Ok(failure(&WindowError::NotMainCaller)));
        }
        let thread = ctx.claims.thread;
        let result = match tokio::time::timeout(CALL_TIMEOUT, self.run(thread, which, args)).await {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!(%thread, tool = name, "a window tool took too long and was cut off");
                refused(
                    "unavailable",
                    "the window did not answer in time; try again",
                )
            }
        };
        Some(Ok(result))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn args(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn every_tool_has_a_name_that_parses_back_and_object_schemas() {
        for which in Which::ALL {
            assert_eq!(Which::parse(which.name()), Some(which));
            let definition = which.definition();
            assert_eq!(definition.name, which.name());
            assert_eq!(definition.input_schema.get("type"), Some(&json!("object")));
            let output = definition.output_schema.expect("an output schema");
            assert_eq!(output.get("type"), Some(&json!("object")));
        }
        assert_eq!(Which::parse("window_opens"), None);
        assert_eq!(Which::parse(""), None);
        assert_eq!(
            Which::ALL.map(Which::name),
            ["window_open", "window_look", "window_act", "window_close"]
        );
    }

    #[test]
    fn the_arguments_are_the_contracts_and_every_other_shape_is_invalid() {
        assert!(parse::<OpenArgs>(args(json!({"url": "http://93.184.216.34/"}))).is_ok());
        assert!(parse::<OpenArgs>(args(json!({"url": "http://a/", "label": "x"}))).is_ok());
        for bad in [
            json!({}),
            json!({"url": 7}),
            json!({"url": "http://a/", "extra": 1}),
        ] {
            assert!(parse::<OpenArgs>(args(bad.clone())).is_err(), "{bad}");
        }
        let act = parse::<ActArgs>(args(json!({
            "window": "w-1",
            "actions": [
                {"click": {"x": 1, "y": 2}},
                {"type": {"text": "hi"}},
                {"scroll": {"dy": -3}},
                {"key": {"name": "Enter"}}
            ]
        })))
        .unwrap();
        assert_eq!(act.actions.len(), 4);
        for bad in [
            json!({"window": "w-1", "actions": [{"swipe": {}}]}),
            json!({"window": "w-1", "actions": [{"click": {"x": -1, "y": 2}}]}),
            json!({"window": "w-1", "actions": "click"}),
            json!({"actions": []}),
        ] {
            assert!(parse::<ActArgs>(args(bad.clone())).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_refusal_is_its_code_then_its_reason() {
        for (error, code) in [
            (WindowError::WindowsOff, "windows_off"),
            (WindowError::NotMainCaller, "not_main_caller"),
            (WindowError::WindowUnknown, "window_unknown"),
            (WindowError::UrlRefused, "url_refused"),
            (WindowError::TooManyWindows, "too_many_windows"),
            (WindowError::Invalid, "invalid"),
        ] {
            let result = failure(&error);
            assert_eq!(result.is_error, Some(true));
            let ContentBlock::Text(text) = &result.content[0] else {
                panic!("{:?}", result.content)
            };
            assert!(text.text.starts_with(&format!("{code}: ")), "{}", text.text);
        }
    }
}

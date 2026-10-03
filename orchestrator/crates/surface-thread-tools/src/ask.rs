//! `ask_agent`: the agent a thread is addressed to asks one of the agents the person mentioned to
//! do part of the work, and waits for its answer
//! ([ADR 0026](../../../../docs/decisions/0026-agent-mentions-as-structured-references.md),
//! [`docs/api/thread-tools-v1.md`](../../../../docs/api/thread-tools-v1.md#ask_agent)).
//!
//! A [`ThreadToolProvider`] like the relay. The rules are the core's and the checks the
//! application's ([`App::ask`]); this is the tool's face and its wait:
//!
//! * **`tools/list`** offers the tool only while it can be used: the thread runs, the token's job
//!   is the thread's current one, the person mentioned someone in it, and the caller's depth is
//!   below the limit. A call that arrives when it is not offered (a list that went stale during a
//!   turn) is a **result with `isError`** that says why, never `-32602`, so the model reads the
//!   reason.
//! * **`tools/call`** validates the arguments (`-32602`), asks ([`App::ask`]) and **waits**: it
//!   follows the thread's log from the ask's own event until `ask_finished` for this ask, and gives
//!   the asked agent's answer as the result. While it waits it tells a client that sent a
//!   `progressToken` of each step under the ask and, in any case, that it is still waiting, every
//!   [`HEARTBEAT`].
//! * **A dropped call does not cancel the ask.** A client that goes away, or cancels the request,
//!   ends the wait and nothing else: the ask runs on, and the agent **re-attaches** by calling
//!   again with the same `callId` (the call key the core deduplicates by). A call that arrives
//!   after the ask ended is answered at once with its recorded result.
//! * **The wait is bounded** by the ask's deadline and [`WAIT_MARGIN`]. The deadline is the
//!   core's (a timer): the ask ends `timed_out`, the asked agent is told to stop, and the call
//!   gives that. A call that outlives even that (the timer is the inbox worker's and it is not
//!   running) says the ask is still going and to call again.
//!
//! What the asked agent said is **untrusted text**: it goes to the asker's model and to the log as
//! text and is never interpreted here.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_app::{App, AppError, AskCall};
use orch_core::{
    AskFinishedData, AskOutcome, AskRefusal, Caller, Classify, ErrorClass, Event, EventBody,
    TransitionError, ask_step_id, report,
};
use orch_ports::Ports;
use rmcp::ErrorData;
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, MetaObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};
use tokio::time::{Instant, MissedTickBehavior};

use crate::relay::{CallMeta, META_KEY};
use crate::{ThreadToolProvider, ToolCtx};

/// The name of the tool.
pub const TOOL_NAME: &str = "ask_agent";

/// How often a call that is waiting says so to a client that sent a `progressToken`.
pub const HEARTBEAT: Duration = Duration::from_secs(30);

/// What the call waits beyond the ask's deadline: time for the core's timer to end the ask and for
/// the end to reach the call.
pub const WAIT_MARGIN: Duration = Duration::from_secs(30);

/// The most characters of `message` (the contract's `maxLength`).
pub const MAX_MESSAGE_CHARS: usize = 16_000;

/// The shortest and longest `timeout_secs` a call may ask for.
const TIMEOUT_RANGE: (u64, u64) = (10, 7200);

/// What the agent is told when the call was cancelled (nobody reads it: the caller is gone).
const CANCELLED: &str = "the call was cancelled; the ask goes on, and a call with the same callId \
    waits for it again";

/// What the agent is told when the orchestrator stops while the call waits.
const STOPPING: &str = "the orchestrator is shutting down; call again with the same callId to \
    wait for the ask";

/// What the agent is told when the store cannot say whether the ask may be made.
const TEMPORARILY_UNAVAILABLE: &str = "temporarily unavailable; try again";

/// What the agent is told when its task is over.
const TASK_OVER: &str = "this task is over";

/// The provider of `ask_agent`. Built by the composition root; asked for each request, it keeps
/// nothing between calls.
pub struct AskTools<P: Ports> {
    app: Arc<App<P>>,
    heartbeat: Duration,
}

impl<P: Ports> fmt::Debug for AskTools<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AskTools")
            .field("heartbeat", &self.heartbeat)
            .finish_non_exhaustive()
    }
}

impl<P: Ports> AskTools<P> {
    /// The tool over `app`, whose configuration says the limits ([`App::ask_limits`]).
    pub fn new(app: Arc<App<P>>) -> Self {
        AskTools {
            app,
            heartbeat: HEARTBEAT,
        }
    }

    /// How often a waiting call tells a client that wants progress that it still waits (default
    /// [`HEARTBEAT`]).
    #[must_use]
    pub fn with_heartbeat(mut self, heartbeat: Duration) -> Self {
        self.heartbeat = heartbeat.max(Duration::from_millis(1));
        self
    }
}

/// The arguments of the call, as the schema says.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    agent: String,
    message: String,
    timeout: Option<Duration>,
}

fn invalid(detail: impl fmt::Display) -> ErrorData {
    ErrorData::invalid_params(format!("invalid arguments: {detail}"), None)
}

impl Args {
    /// `{agent, message, timeout_secs?}` and nothing else; a missing or mistyped member, an empty
    /// or long message and a timeout out of range are `-32602`.
    fn parse(arguments: JsonObject) -> Result<Args, ErrorData> {
        let mut agent = None;
        let mut message = None;
        let mut timeout = None;
        for (key, value) in arguments {
            match key.as_str() {
                "agent" => match value {
                    Value::String(text) => agent = Some(text),
                    _ => return Err(invalid("agent must be a string")),
                },
                "message" => match value {
                    Value::String(text) => message = Some(text),
                    _ => return Err(invalid("message must be a string")),
                },
                "timeout_secs" => match value.as_u64() {
                    Some(secs) if (TIMEOUT_RANGE.0..=TIMEOUT_RANGE.1).contains(&secs) => {
                        timeout = Some(Duration::from_secs(secs));
                    }
                    _ => {
                        return Err(invalid(format!(
                            "timeout_secs must be an integer from {} to {}",
                            TIMEOUT_RANGE.0, TIMEOUT_RANGE.1
                        )));
                    }
                },
                other => {
                    return Err(invalid(format!(
                        "{} is not an argument of {TOOL_NAME}",
                        other.chars().take(40).collect::<String>()
                    )));
                }
            }
        }
        let agent = agent.ok_or_else(|| invalid("agent is required"))?;
        let message = message.ok_or_else(|| invalid("message is required"))?;
        if message.trim().is_empty() {
            return Err(invalid("message must not be empty"));
        }
        if message.chars().count() > MAX_MESSAGE_CHARS {
            return Err(invalid(format!(
                "message must be at most {MAX_MESSAGE_CHARS} characters"
            )));
        }
        Ok(Args {
            agent,
            message,
            timeout,
        })
    }
}

fn object(value: Value) -> Arc<JsonObject> {
    match value {
        Value::Object(map) => Arc::new(map),
        // the schemas below are written as objects; a test of `tools/list` reads each of them
        _ => Arc::new(JsonObject::new()),
    }
}

/// The definition of the tool: the contract's, with the agents that may be asked named in the
/// description and the longest the call may take in `_meta`.
fn definition(mentioned: &[String], timeout: Duration) -> Tool {
    let description = format!(
        "Ask one of the agents the person mentioned to do part of the work and wait for its \
         answer. The agent does not see this conversation: put everything it needs in message. \
         Asking the same agent again continues its conversation, and answers its question if it \
         asked one. Agents you can ask: {}.",
        mentioned.join(", ")
    );
    let input = object(json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["agent", "message"],
        "properties": {
            "agent": {"type": "string", "description": "the agentId of a mention"},
            "message": {"type": "string", "minLength": 1, "maxLength": MAX_MESSAGE_CHARS},
            "timeout_secs": {
                "type": "integer",
                "minimum": TIMEOUT_RANGE.0,
                "maximum": TIMEOUT_RANGE.1,
                "description": "Lowers the ask's timeout; it never raises the deployment's"
            }
        }
    }));
    let output = object(json!({
        "type": "object",
        "required": ["ask", "agent", "state"],
        "properties": {
            "ask": {"type": "integer", "minimum": 1},
            "agent": {"type": "string"},
            "state": {"enum": [
                "completed", "input_required", "auth_required", "failed", "rejected",
                "canceled", "timed_out"
            ]},
            "text": {"type": "string"},
            "artifacts": {"type": "array", "items": {
                "type": "object",
                "required": ["name"],
                "properties": {
                    "name": {"type": "string"},
                    "uri": {"type": "string"},
                    "mimeType": {"type": "string"}
                }
            }},
            "question": {"type": "string"},
            "error": {"type": "string"}
        }
    }));
    let mut meta = Map::new();
    meta.insert(
        META_KEY.to_owned(),
        json!({"reportsStep": true, "timeoutSecs": (timeout + WAIT_MARGIN).as_secs()}),
    );
    Tool::new(TOOL_NAME, description, input)
        .with_title("Ask a mentioned agent")
        .with_raw_output_schema(output)
        .with_annotations(
            ToolAnnotations::default()
                .read_only(false)
                .destructive(false)
                .idempotent(false)
                .open_world(true),
        )
        .with_meta(MetaObject(meta))
}

/// A refusal of the call: nothing was written, and the model reads why.
fn refused(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

/// What the core's refusal says, in the words of the contract.
fn refusal_text(why: &AskRefusal) -> String {
    match why {
        AskRefusal::TaskOver | AskRefusal::UnknownCaller { .. } => TASK_OVER.to_owned(),
        AskRefusal::NotMentioned { mentioned, .. } if mentioned.is_empty() => {
            "the person mentioned no agent in this job, so there is none to ask".to_owned()
        }
        AskRefusal::NotMentioned { mentioned, .. } => format!(
            "you can ask only the agents the person mentioned: {}",
            mentioned
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        AskRefusal::Cycle { .. } => {
            "an agent cannot ask itself or an agent already in its chain".to_owned()
        }
        AskRefusal::DepthReached { max } => format!("asks are nested at most {max} deep"),
        AskRefusal::TooManyInJob { max } => format!("this job has used its {max} asks"),
        AskRefusal::TooManyRunning { max } => {
            format!("{max} asks are already running; wait for one to finish")
        }
        AskRefusal::EmptyText => "message must not be empty".to_owned(),
        AskRefusal::TextTooLong { max } => {
            format!("message may hold {max} bytes at most")
        }
        AskRefusal::CallKeyTooLong { .. } => "the callId is too long".to_owned(),
        AskRefusal::CallKeyReused => "this callId was used for another ask".to_owned(),
    }
}

/// What an error of [`App::ask`] is to the model: a result that says why, or, for a fault of the
/// server, an internal error.
fn failure(error: &AppError) -> Result<CallToolResult, ErrorData> {
    let text = match error {
        AppError::Transition(TransitionError::AskRefused(why)) => refusal_text(why),
        AppError::Transition(_) | AppError::NotFound => TASK_OVER.to_owned(),
        AppError::Forbidden { .. } => {
            // the agent is the one the model named; the person's roles are not told
            "the person may not use that agent".to_owned()
        }
        AppError::Unprocessable(detail) | AppError::Invalid(detail) => detail.clone(),
        AppError::RegistryUnavailable { .. } => {
            "the agent list is unavailable; try again".to_owned()
        }
        other => match other.class() {
            ErrorClass::Conflict | ErrorClass::Transient | ErrorClass::RateLimited => {
                tracing::warn!(error = %report(other), "ask_agent failed temporarily");
                TEMPORARILY_UNAVAILABLE.to_owned()
            }
            _ => {
                tracing::error!(error = %report(other), class = ?other.class(), "ask_agent failed");
                return Err(ErrorData::internal_error("internal error", None));
            }
        },
    };
    Ok(refused(text))
}

/// The result of an ask that ended: `{ask, agent, state, text?, artifacts?, question?, error?}` as
/// structured content and as the same JSON text, an error unless the asked agent answered or
/// asked something back.
fn result_of(finished: &AskFinishedData, agent: &str) -> CallToolResult {
    let mut value = Map::new();
    value.insert("ask".to_owned(), json!(finished.ask));
    value.insert("agent".to_owned(), json!(agent));
    value.insert("state".to_owned(), json!(finished.state.as_str()));
    if let Some(text) = &finished.text {
        value.insert("text".to_owned(), json!(text));
    }
    if !finished.artifacts.is_empty() {
        value.insert("artifacts".to_owned(), json!(finished.artifacts));
    }
    if let Some(question) = &finished.question {
        value.insert("question".to_owned(), json!(question));
    }
    if let Some(error) = &finished.error {
        value.insert("error".to_owned(), json!(error));
    }
    let value = Value::Object(value);
    match finished.state {
        AskOutcome::Completed | AskOutcome::InputRequired | AskOutcome::AuthRequired => {
            CallToolResult::structured(value)
        }
        AskOutcome::Failed | AskOutcome::Rejected | AskOutcome::Canceled | AskOutcome::TimedOut => {
            CallToolResult::structured_error(value)
        }
    }
}

/// What a call reports of an event it follows, for a client that wants progress: a step that runs
/// under the ask, or an ask that was started under it.
fn progress_of(event: &Event, ask: u32) -> Option<String> {
    let under = ask_step_id(ask);
    match &event.body {
        EventBody::AgentStep(step) if step.path.contains(&under) => {
            Some(format!("{}: {}", event.actor.name, step.label))
        }
        EventBody::AskStarted(started) if started.by == Caller::Ask(ask) => {
            Some(format!("asking {}", started.agent))
        }
        _ => None,
    }
}

impl<P: Ports> AskTools<P> {
    /// The call: ask, then wait for the end.
    async fn ask(&self, ctx: &ToolCtx, arguments: JsonObject) -> Result<CallToolResult, ErrorData> {
        let args = Args::parse(arguments)?;
        let claims = &ctx.claims;
        let meta = CallMeta::of(ctx.meta.as_ref());
        let agent = args.agent.clone();
        let call = AskCall {
            thread: claims.thread,
            job: claims.job,
            caller: claims.caller,
            asker: claims.agent.clone(),
            agent: orch_core::AgentId::new(args.agent),
            text: args.message,
            call_id: meta.call_id,
            parent_step: meta.parent_step_id,
            timeout: args.timeout,
        };
        let handle = match self.app.ask(call).await {
            Ok(handle) => handle,
            Err(error) => return failure(&error),
        };
        tracing::debug!(
            thread = %claims.thread,
            ask = handle.ask,
            %agent,
            reattached = handle.reattached,
            "ask_agent waits for the asked agent"
        );
        self.wait(ctx, handle.ask, &agent, handle.after, handle.timeout)
            .await
    }

    /// Follows the log until the ask ends, the client goes, or the wait runs out.
    async fn wait(
        &self,
        ctx: &ToolCtx,
        ask: u32,
        agent: &str,
        after: i64,
        timeout: Duration,
    ) -> Result<CallToolResult, ErrorData> {
        let thread = ctx.claims.thread;
        let mut events = match self.app.thread_events_for_tools(thread, after).await {
            Ok(events) => events,
            Err(error) => return failure(&error),
        };
        let bound = Instant::now() + timeout + WAIT_MARGIN;
        let mut beat = tokio::time::interval_at(Instant::now() + self.heartbeat, self.heartbeat);
        beat.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut progress = 0_u64;
        loop {
            tokio::select! {
                // The client is gone: the ask is not its to cancel.
                () = ctx.cancel.cancelled() => return Ok(refused(CANCELLED)),
                () = tokio::time::sleep_until(bound) => {
                    return Ok(refused(format!(
                        "ask {ask} is still running after {} s; call again with the same \\
                         callId to wait for it",
                        (timeout + WAIT_MARGIN).as_secs()
                    )));
                }
                _ = beat.tick() => {
                    progress += 1;
                    if !ctx.progress.send(progress, format!("waiting for {agent}")).await {
                        return Ok(refused(CANCELLED));
                    }
                }
                next = events.next() => {
                    let Some(event) = next else {
                        return Ok(refused(STOPPING));
                    };
                    if let EventBody::AskFinished(finished) = &event.body
                        && finished.ask == ask
                    {
                        return Ok(result_of(finished, agent));
                    }
                    if let Some(message) = progress_of(&event, ask) {
                        progress += 1;
                        if !ctx.progress.send(progress, message).await {
                            return Ok(refused(CANCELLED));
                        }
                    }
                }
            }
        }
    }
}

impl<P: Ports> ThreadToolProvider for AskTools<P> {
    async fn list(&self, ctx: &ToolCtx) -> Vec<Tool> {
        let claims = &ctx.claims;
        let thread = match self.app.thread_for_tools(claims.thread).await {
            Ok(Some(thread)) => thread,
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::warn!(thread = %claims.thread, error = %report(&error), "ask_agent could not read the thread; it is not listed");
                return Vec::new();
            }
        };
        let limits = self.app.ask_limits();
        let offered = !thread.state.is_terminal()
            && thread.job.number == claims.job
            && !thread.job.mentioned.is_empty()
            && claims.depth < limits.depth;
        if !offered {
            return Vec::new();
        }
        let mentioned: Vec<String> = thread
            .job
            .mentioned
            .iter()
            .map(ToString::to_string)
            .collect();
        let timeout = Duration::from_secs(limits.timeout.as_secs().unsigned_abs());
        vec![definition(&mentioned, timeout)]
    }

    async fn call(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: JsonObject,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        if name != TOOL_NAME {
            return None;
        }
        Some(self.ask(ctx, args).await)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::{AgentId, AskArtifact};

    use super::*;

    fn args(value: Value) -> JsonObject {
        let Value::Object(map) = value else {
            panic!("an object")
        };
        map
    }

    #[test]
    fn the_arguments_are_the_schema_and_nothing_else() {
        let ok = Args::parse(args(json!({"agent": "a", "message": "do it"}))).unwrap();
        assert_eq!(
            ok,
            Args {
                agent: "a".into(),
                message: "do it".into(),
                timeout: None
            }
        );
        let with = Args::parse(args(
            json!({"agent": "a", "message": "x", "timeout_secs": 10}),
        ));
        assert_eq!(with.unwrap().timeout, Some(Duration::from_secs(10)));
        assert!(
            Args::parse(args(
                json!({"agent": "a", "message": "x", "timeout_secs": 7200})
            ))
            .is_ok()
        );
        for bad in [
            json!({}),
            json!({"agent": "a"}),
            json!({"message": "x"}),
            json!({"agent": 1, "message": "x"}),
            json!({"agent": "a", "message": 1}),
            json!({"agent": "a", "message": ""}),
            json!({"agent": "a", "message": "  \n"}),
            json!({"agent": "a", "message": "x".repeat(MAX_MESSAGE_CHARS + 1)}),
            json!({"agent": "a", "message": "x", "timeout_secs": 9}),
            json!({"agent": "a", "message": "x", "timeout_secs": 7201}),
            json!({"agent": "a", "message": "x", "timeout_secs": "60"}),
            json!({"agent": "a", "message": "x", "timeout_secs": 60.5}),
            json!({"agent": "a", "message": "x", "timeout_secs": -1}),
            json!({"agent": "a", "message": "x", "extra": true}),
        ] {
            let error = Args::parse(args(bad.clone())).unwrap_err();
            assert_eq!(error.code.0, -32602, "{bad}: {error:?}");
        }
        // the longest message is as long as the contract says, in characters
        let long = "é".repeat(MAX_MESSAGE_CHARS);
        assert!(Args::parse(args(json!({"agent": "a", "message": long}))).is_ok());
    }

    #[test]
    fn the_definition_is_the_contracts_with_the_agents_named() {
        let tool = definition(&["a".to_owned(), "b".to_owned()], Duration::from_secs(1800));
        assert_eq!(tool.name, TOOL_NAME);
        assert!(
            tool.description
                .as_deref()
                .unwrap()
                .ends_with("Agents you can ask: a, b.")
        );
        let schema = &tool.input_schema;
        assert_eq!(schema["required"], json!(["agent", "message"]));
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(schema["properties"]["timeout_secs"]["maximum"], json!(7200));
        let meta = tool.meta.unwrap().0;
        assert_eq!(
            meta[META_KEY],
            json!({"reportsStep": true, "timeoutSecs": 1830})
        );
        let annotations = tool.annotations.unwrap();
        assert_eq!(annotations.read_only_hint, Some(false));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(false));
        assert_eq!(annotations.open_world_hint, Some(true));
    }

    #[test]
    fn every_refusal_has_the_words_of_the_contract() {
        let a = AgentId::new("a");
        for (why, text) in [
            (AskRefusal::TaskOver, "this task is over"),
            (AskRefusal::UnknownCaller { n: 3 }, "this task is over"),
            (
                AskRefusal::NotMentioned {
                    agent: AgentId::new("x"),
                    mentioned: vec![a.clone(), AgentId::new("b")],
                },
                "you can ask only the agents the person mentioned: a, b",
            ),
            (
                AskRefusal::Cycle { agent: a },
                "an agent cannot ask itself or an agent already in its chain",
            ),
            (
                AskRefusal::DepthReached { max: 2 },
                "asks are nested at most 2 deep",
            ),
            (
                AskRefusal::TooManyInJob { max: 16 },
                "this job has used its 16 asks",
            ),
            (
                AskRefusal::TooManyRunning { max: 4 },
                "4 asks are already running; wait for one to finish",
            ),
            (
                AskRefusal::CallKeyReused,
                "this callId was used for another ask",
            ),
        ] {
            assert_eq!(refusal_text(&why), text);
        }
    }

    #[test]
    fn what_the_application_refuses_is_a_result_that_says_why_and_a_fault_is_an_error() {
        use orch_app::{AppError, Permission};
        let text = |error: AppError| match failure(&error).unwrap() {
            result if result.is_error == Some(true) => match &result.content[0] {
                ContentBlock::Text(t) => t.text.clone(),
                other => panic!("{other:?}"),
            },
            other => panic!("not an error result: {other:?}"),
        };
        assert_eq!(
            text(AppError::agent_not_allowed(
                Permission::AgentInvoke,
                &AgentId::new("coder")
            )),
            "the person may not use that agent"
        );
        assert_eq!(
            text(AppError::Unprocessable(
                "agent 'coder' is no longer listed".to_owned()
            )),
            "agent 'coder' is no longer listed"
        );
        assert_eq!(
            text(AppError::RegistryUnavailable {
                source: orch_ports::RegistryError::unavailable("platform", "down"),
            }),
            "the agent list is unavailable; try again"
        );
        assert_eq!(text(AppError::NotFound), "this task is over");
        assert_eq!(
            text(AppError::Contended),
            "temporarily unavailable; try again"
        );
        assert_eq!(
            text(AppError::Transition(TransitionError::AskRefused(
                AskRefusal::CallKeyReused
            ))),
            "this callId was used for another ask"
        );
        let fault = failure(&AppError::internal("broken")).unwrap_err();
        assert_eq!(fault.code.0, -32603);
    }

    #[test]
    fn an_ended_ask_is_a_result_that_is_an_error_unless_the_agent_answered_or_asked_back() {
        let finished = |state, text: Option<&str>| AskFinishedData {
            ask: 2,
            state,
            text: text.map(str::to_owned),
            question: (state == AskOutcome::InputRequired).then(|| "which branch?".to_owned()),
            artifacts: vec![AskArtifact {
                name: "pitch.png".into(),
                uri: None,
                mime_type: Some("image/png".into()),
            }],
            error: (state == AskOutcome::Failed).then(|| "it broke".to_owned()),
        };
        for (state, error) in [
            (AskOutcome::Completed, false),
            (AskOutcome::InputRequired, false),
            (AskOutcome::AuthRequired, false),
            (AskOutcome::Failed, true),
            (AskOutcome::Rejected, true),
            (AskOutcome::Canceled, true),
            (AskOutcome::TimedOut, true),
        ] {
            let result = result_of(&finished(state, Some("Pictures")), "mock-browser");
            assert_eq!(result.is_error, Some(error), "{state:?}");
            let value = result.structured_content.unwrap();
            assert_eq!(value["ask"], 2);
            assert_eq!(value["agent"], "mock-browser");
            assert_eq!(value["state"], state.as_str());
            assert_eq!(value["text"], "Pictures");
            assert_eq!(value["artifacts"][0]["name"], "pitch.png");
            assert_eq!(value["artifacts"][0]["mimeType"], "image/png");
            // the same JSON as one text content
            let text = match &result.content[0] {
                ContentBlock::Text(t) => t.text.clone(),
                other => panic!("{other:?}"),
            };
            assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
        }
        // absent members are absent
        let bare = result_of(&finished(AskOutcome::Canceled, None), "a")
            .structured_content
            .unwrap();
        assert!(bare.get("text").is_none() && bare.get("question").is_none());
    }
}

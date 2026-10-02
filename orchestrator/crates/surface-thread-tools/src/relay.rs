//! The relay: the tools of the MCP servers attached to a thread, on the thread's own endpoint
//! (`thread-tools/v1`, [ADR 0024](../../../../docs/decisions/0024-mcp-tools-attached-per-conversation.md)).
//!
//! A [`ThreadToolProvider`] like any other. For a request from a caller it serves the servers
//! that are **attached to the thread** and **offered for the caller's agent**:
//!
//! - `tools/list`: for each such server the upstream `tools/list` (nothing is cached: each request
//!   lists again), the server's allow-list intersected with what it says, each tool named
//!   `<server id>__<tool>` and marked in its `_meta["thread-tools/v1"]` with `reportsStep: true`
//!   and `timeoutSecs`. A server that cannot be listed is left out and logged.
//! - `tools/call`: the call is made upstream with the server's credentials, which this type holds
//!   and no other part of the application sees (a [`ToolServerEndpoint`] prints no secret), and is
//!   **one step** of the thread: `running` when it starts, then `completed`, `failed` or
//!   `canceled`, recorded through [`App::record_step`] with the server's icon
//!   (`mcp-server:<id>`), its arguments as the step's `input` and the result's text as its
//!   `output`, under the bounds of ADR 0030.
//!
//! What the upstream answers is untrusted text: it goes to the agent's model and to the step as
//! text, and is never interpreted here. The agent gets the whole result (the client cuts it at
//! 256 KiB, and a note says so); the step keeps what ADR 0030 allows.
//!
//! The error table of the contract is [`ERRORS`]; the tests walk it row by row.
//!
//! # A call that is dropped
//!
//! A client that goes away drops the call's future (an HTTP request whose connection closed) or
//! cancels it ([`ToolCtx::cancel`]). Both end the upstream call and the step as `canceled`; the
//! first one from a guard that records the end from a task of its own, because a dropped future
//! cannot await.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures::future::join_all;
use orch_app::{App, AppError, ToolServerInfo};
use orch_core::{
    Actor, AgentId, MAX_STEP_ID_BYTES, StepKind, StepOutput, StepReport, StepState, ThreadId,
    ThreadRecord,
};
use orch_ports::{
    IdGen, MAX_RESULT_BYTES, Ports, ThreadStore, ToolCall, ToolCallOutput, ToolDef,
    ToolServerClient, ToolServerEndpoint, ToolServerError,
};
use rmcp::ErrorData;
use rmcp::model::{
    CallToolResult, ContentBlock, Icon, JsonObject, MetaObject, Tool, ToolAnnotations,
};
use serde_json::{Map, Value, json};

use crate::{ThreadToolProvider, ToolCtx};

/// The key of this extension in the `_meta` of a tool and of a call (the full URI of the extension
/// is not a key MCP allows, see the contract).
pub const META_KEY: &str = "thread-tools/v1";

/// The longest name a relayed tool may have, `<server id>__<tool>` included.
pub const MAX_TOOL_NAME_BYTES: usize = 64;

/// What a tool's `description` is cut to, in bytes.
const MAX_DESCRIPTION_BYTES: usize = 8 * 1024;

/// What the agent's `callId` and `parentStepId` may be, in bytes.
const MAX_META_ID_BYTES: usize = 256;

/// What a step's `detail` is cut to, in characters.
const MAX_DETAIL_CHARS: usize = 200;

/// What the orchestrator adds to a server's timeout in a tool's `timeoutSecs`: time for the answer
/// to get back to the agent.
pub const TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

/// What the call waits beyond the endpoint's timeout for a client that did not keep its promise to
/// be bounded by it.
const CLIENT_GRACE: Duration = Duration::from_secs(5);

/// What the agent is told when the call is not made because its task is over.
pub const TASK_OVER: &str = "this task is over";

/// What the agent is told when the store cannot say whether the call may be made.
const TEMPORARILY_UNAVAILABLE: &str = "temporarily unavailable; try again";

/// What the agent is told when its call was cancelled (nobody reads it: the caller is gone).
const CANCELLED: &str = "the call was cancelled";

/// The error table of the contract (`docs/api/thread-tools-v1.md`, "Errors of a call"): what the
/// agent gets and the step's state, by situation. A summary for readers and for the tests, which
/// make each row happen.
pub const ERRORS: [(&str, &str, &str); 9] = [
    ("the name is not on the endpoint", "-32602", "none"),
    ("the caller's task is over", "isError", "none"),
    ("the server cannot be reached", "isError", "failed"),
    ("no answer within the timeout", "isError", "failed"),
    ("the server answers 401 or 403", "isError", "failed"),
    ("the server answers a JSON-RPC error", "isError", "failed"),
    ("the result has isError", "passed through", "failed"),
    (
        "the result is over 256 KiB",
        "cut, with a note",
        "completed",
    ),
    ("the agent cancels or drops the call", "dropped", "canceled"),
];

/// Why a [`RelayTools`] cannot be built.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum RelayError {
    /// A server id that is not one (`^[a-z0-9][a-z0-9-]{0,30}$`): with a `_` in it, the first `__`
    /// of a tool's name would not be the split.
    #[error("the server id {0:?} is not a valid id")]
    BadServerId(String),
    /// Two servers with one id.
    #[error("the server id {0:?} is listed twice")]
    DuplicateServer(String),
    /// A server whose endpoint is another server's (the ids differ).
    #[error("the endpoint of the server {0:?} names another id")]
    EndpointMismatch(String),
}

/// One server: what the application tells of it, and where it is and how to get in.
struct Server {
    info: ToolServerInfo,
    endpoint: ToolServerEndpoint,
}

/// The provider of the relayed tools. Built by the composition root from the deployment's list;
/// asked for each request, it keeps nothing between calls.
pub struct RelayTools<P: Ports, C: ToolServerClient> {
    app: Arc<App<P>>,
    client: Arc<C>,
    servers: Vec<Server>,
}

impl<P: Ports, C: ToolServerClient> fmt::Debug for RelayTools<P, C> {
    /// The ids of the servers and nothing else.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayTools")
            .field(
                "servers",
                &self
                    .servers
                    .iter()
                    .map(|s| s.info.id.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl<P: Ports, C: ToolServerClient> RelayTools<P, C> {
    /// A relay of `servers` (the public part of each and its endpoint, with its credentials), which
    /// it calls through `client`, recording its steps through `app`.
    ///
    /// # Errors
    ///
    /// [`RelayError`] for an id that is not a server id, one listed twice, or an endpoint that names
    /// another id than its server.
    pub fn new(
        app: Arc<App<P>>,
        client: C,
        servers: impl IntoIterator<Item = (ToolServerInfo, ToolServerEndpoint)>,
    ) -> Result<Self, RelayError> {
        let mut kept: Vec<Server> = Vec::new();
        for (info, endpoint) in servers {
            if !orch_core::is_valid_server_id(&info.id) {
                return Err(RelayError::BadServerId(info.id));
            }
            if kept.iter().any(|s| s.info.id == info.id) {
                return Err(RelayError::DuplicateServer(info.id));
            }
            if endpoint.id != info.id {
                return Err(RelayError::EndpointMismatch(info.id));
            }
            kept.push(Server { info, endpoint });
        }
        Ok(RelayTools {
            app,
            client: Arc::new(client),
            servers: kept,
        })
    }

    /// The thread, or why not: `Ok(None)` when nothing has the id.
    async fn thread(&self, id: ThreadId) -> Result<Option<ThreadRecord>, AppError> {
        self.app.thread_for_tools(id).await
    }

    /// The servers attached to the thread that are offered for `agent`, in the order of the
    /// deployment's list.
    fn served<'a>(&'a self, record: &ThreadRecord, agent: &AgentId) -> Vec<&'a Server> {
        self.servers
            .iter()
            .filter(|s| record.job.tools.contains(&s.info.id) && s.info.allows(agent))
            .collect()
    }

    /// The step's actor: the agent whose call it is, with the revision that serves the task.
    async fn actor_and_task(&self, ctx: &ToolCtx) -> (Actor, Option<String>) {
        let binding = match self
            .app
            .ports()
            .store()
            .get_binding(ctx.claims.thread)
            .await
        {
            Ok(binding) => binding,
            Err(error) => {
                tracing::warn!(thread = %ctx.claims.thread, %error, "the binding of a relayed call's thread could not be read");
                None
            }
        };
        let revision = binding.as_ref().and_then(|b| b.revision.clone());
        let task = binding.and_then(|b| b.task_id);
        (Actor::agent(&ctx.claims.agent, revision), task)
    }
}

/// The tool `tool` of the server `server`, as the endpoint names it, when that name is one a model
/// API takes: `^[A-Za-z0-9_-]{1,64}$`, and the tool's own name does not start with `_` (the first
/// `__` is then the split).
fn relayed_name(server: &str, tool: &str) -> Option<String> {
    if tool.is_empty() || tool.starts_with('_') {
        return None;
    }
    let name = format!("{server}__{tool}");
    (name.len() <= MAX_TOOL_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(name)
}

/// Whether the deployment's allow-list of `server` (none: every tool) names `tool`.
fn allowed(server: &ToolServerInfo, tool: &str) -> bool {
    server
        .tools
        .as_ref()
        .is_none_or(|tools| tools.iter().any(|t| t == tool))
}

/// `text` cut to at most `max` bytes at a character boundary.
fn cut_bytes(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `text` as one line of at most [`MAX_DETAIL_CHARS`] characters, ending in `…` when cut: its
/// first line that has something in it.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    cut_chars(line, MAX_DETAIL_CHARS)
}

fn cut_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// A relayed tool's definition, from what the server says of it. `None` for a tool whose name does
/// not fit.
fn definition(server: &Server, tool: &ToolDef) -> Option<Tool> {
    let name = relayed_name(&server.info.id, &tool.name)?;
    let mut schema = tool.input_schema.clone();
    schema
        .entry("type")
        .or_insert_with(|| Value::String("object".to_owned()));
    let description = tool
        .description
        .as_deref()
        .map(|d| cut_bytes(d, MAX_DESCRIPTION_BYTES).to_owned());
    let mut relayed = Tool::new_with_raw(name, description.map(Into::into), Arc::new(schema))
        .with_title(
            tool.title
                .clone()
                .unwrap_or_else(|| format!("{}: {}", server.info.name, tool.name)),
        );
    if let Some(output) = &tool.output_schema {
        relayed = relayed.with_raw_output_schema(Arc::new(output.clone()));
    }
    if let Some(annotations) = tool
        .annotations
        .clone()
        .and_then(|a| serde_json::from_value::<ToolAnnotations>(a).ok())
    {
        relayed = relayed.with_annotations(annotations);
    }
    // Only the icon of the configuration: an upstream's icons are dropped (open question 38).
    if let Some(icon) = &server.info.icon {
        relayed = relayed.with_icons(vec![Icon::new(icon.clone())]);
    }
    let timeout = server.endpoint.timeout + TIMEOUT_MARGIN;
    let mut meta = Map::new();
    meta.insert(
        META_KEY.to_owned(),
        json!({"reportsStep": true, "timeoutSecs": timeout.as_secs()}),
    );
    Some(relayed.with_meta(MetaObject(meta)))
}

/// What the agent said of its call in the request's `_meta[thread-tools/v1]`.
#[derive(Debug, Default, PartialEq, Eq)]
struct CallMeta {
    call_id: Option<String>,
    parent_step_id: Option<String>,
}

impl CallMeta {
    /// Both members are optional; one that is not a string, is empty, is longer than 256 bytes or
    /// has a control character is as good as absent.
    fn of(meta: Option<&Map<String, Value>>) -> Self {
        let entry = meta
            .and_then(|m| m.get(META_KEY))
            .and_then(Value::as_object);
        let text = |key: &str| {
            entry
                .and_then(|e| e.get(key))
                .and_then(Value::as_str)
                .filter(|s| {
                    !s.is_empty()
                        && s.len() <= MAX_META_ID_BYTES
                        && !s.chars().any(char::is_control)
                })
                .map(str::to_owned)
        };
        CallMeta {
            call_id: text("callId"),
            parent_step_id: text("parentStepId"),
        }
    }
}

/// How a call ended, for the step and for the agent.
struct Ended {
    result: CallToolResult,
    state: StepState,
    detail: Option<String>,
    output: Option<StepOutput>,
}

fn error_result(text: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

/// What a call that could not be made, or did not finish, ends as.
fn failure(server: &ToolServerInfo, error: &ToolServerError) -> Ended {
    let name = &server.name;
    let message = match error {
        ToolServerError::Unreachable { .. } => {
            format!("the MCP server '{name}' could not be reached; the call did not run")
        }
        ToolServerError::TimedOut { after } => format!(
            "no answer within {} s; it may still be running on the server",
            after.as_secs().max(1)
        ),
        ToolServerError::Unauthenticated => {
            "the MCP server refused the orchestrator's credentials".to_owned()
        }
        ToolServerError::Remote { message, .. } => message.clone(),
        ToolServerError::Protocol { .. } => {
            format!("the MCP server '{name}' did not answer as MCP")
        }
        ToolServerError::Misconfigured(_) => {
            format!("the MCP server '{name}' is not set up for use")
        }
        // `ToolServerError` is non-exhaustive.
        _ => format!("the MCP server '{name}' failed"),
    };
    let output = matches!(error, ToolServerError::Remote { .. }).then(|| StepOutput {
        text: message.clone(),
        error: true,
        ..StepOutput::default()
    });
    Ended {
        detail: Some(first_line(&message)),
        result: error_result(message),
        state: StepState::Failed,
        output,
    }
}

/// What a call that the server answered ends as: its result passed through, and the step says
/// whether the tool failed.
fn answered(output: ToolCallOutput) -> Ended {
    let mut text = output.text_of();
    if text.is_empty()
        && let Some(structured) = &output.structured
    {
        text = structured.to_string();
    }
    let blocks: Vec<ContentBlock> = output
        .content
        .iter()
        .filter_map(|block| serde_json::from_value(block.clone()).ok())
        .collect();
    let mut content = blocks;
    if output.truncated {
        content.push(ContentBlock::text(format!(
            "[the result is longer than {} KiB and was cut here]",
            MAX_RESULT_BYTES / 1024
        )));
    }
    let mut result = if output.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    result.structured_content = output.structured;
    let step_output = StepOutput {
        truncated: output.truncated,
        error: output.is_error,
        text: text.clone(),
        bytes: None,
    };
    if output.is_error {
        Ended {
            result,
            state: StepState::Failed,
            detail: Some(match first_line(&text) {
                line if line.is_empty() => "the tool reported an error".to_owned(),
                line => line,
            }),
            output: Some(step_output),
        }
    } else {
        Ended {
            result,
            state: StepState::Completed,
            detail: None,
            output: Some(step_output),
        }
    }
}

/// One call's step. It records the start; the end is recorded by [`CallStep::end`], or, when the
/// call's future is dropped first, by the guard's `Drop` from a task of its own, as `canceled`.
struct CallStep<P: Ports> {
    app: Arc<App<P>>,
    thread: ThreadId,
    actor: Actor,
    /// The report every moment of the step is made from: id, parent, kind, label, icon.
    template: StepReport,
    ended: bool,
}

/// Records one report of a step; a step that cannot be kept is logged and never fails a call.
async fn record<P: Ports>(app: &App<P>, thread: ThreadId, actor: Actor, report: StepReport) {
    if let Err(error) = app.record_step(thread, actor, report, None).await {
        match error {
            // The thread ended meanwhile: nothing to report to.
            AppError::NotFound | AppError::Transition(_) => {
                tracing::debug!(%thread, %error, "a relayed call's step was not recorded: the thread is over");
            }
            other => {
                tracing::warn!(%thread, error = %other, "a relayed call's step was not recorded");
            }
        }
    }
}

impl<P: Ports> CallStep<P> {
    async fn start(
        app: Arc<App<P>>,
        thread: ThreadId,
        actor: Actor,
        template: StepReport,
        input: Map<String, Value>,
    ) -> Self {
        let mut report = template.clone();
        report.state = StepState::Running;
        report.input = Some(input);
        record(&app, thread, actor.clone(), report).await;
        CallStep {
            app,
            thread,
            actor,
            template,
            ended: false,
        }
    }

    fn end_report(
        &self,
        state: StepState,
        detail: Option<String>,
        output: Option<StepOutput>,
    ) -> StepReport {
        StepReport {
            state,
            detail,
            output,
            ..self.template.clone()
        }
    }

    async fn end(mut self, state: StepState, detail: Option<String>, output: Option<StepOutput>) {
        self.ended = true;
        let report = self.end_report(state, detail, output);
        record(&self.app, self.thread, self.actor.clone(), report).await;
    }
}

impl<P: Ports> Drop for CallStep<P> {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let report = self.end_report(StepState::Canceled, Some(CANCELLED.to_owned()), None);
        let (app, thread, actor) = (Arc::clone(&self.app), self.thread, self.actor.clone());
        runtime.spawn(async move { record(&app, thread, actor, report).await });
    }
}

impl<P: Ports, C: ToolServerClient> ThreadToolProvider for RelayTools<P, C> {
    async fn list(&self, ctx: &ToolCtx) -> Vec<Tool> {
        let record = match self.thread(ctx.claims.thread).await {
            Ok(Some(record)) => record,
            Ok(None) => return Vec::new(),
            Err(error) => {
                tracing::warn!(thread = %ctx.claims.thread, %error, "the relay could not read the thread; no tools are listed");
                return Vec::new();
            }
        };
        let servers = self.served(&record, &ctx.claims.agent);
        let listings = join_all(servers.iter().map(|server| async move {
            (*server, self.client.list_tools(&server.endpoint).await)
        }))
        .await;
        let mut tools: Vec<Tool> = Vec::new();
        for (server, listing) in listings {
            let upstream = match listing {
                Ok(upstream) => upstream,
                Err(error) => {
                    // The public detail: no URL, no credential, no transport error.
                    tracing::warn!(
                        server = %server.info.id,
                        reason = %error.public_detail(),
                        "an attached MCP server could not be listed; its tools are left out"
                    );
                    continue;
                }
            };
            let mut seen: Vec<&str> = Vec::new();
            for tool in &upstream {
                if !allowed(&server.info, &tool.name) || seen.contains(&tool.name.as_str()) {
                    continue;
                }
                match definition(server, tool) {
                    Some(relayed) => {
                        seen.push(&tool.name);
                        tools.push(relayed);
                    }
                    None => tracing::warn!(
                        server = %server.info.id,
                        tool = %tool.name,
                        "a tool whose relayed name does not fit is left out"
                    ),
                }
            }
        }
        tools
    }

    async fn call(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: JsonObject,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        // Not ours: no `<server>__<tool>`, a server nobody listed, a tool left out by the
        // allow-list, a name no model API would take, or a server not offered for the agent. The
        // endpoint answers `-32602` when no provider owns the name.
        let (server_id, tool) = name.split_once("__")?;
        let server = self.servers.iter().find(|s| s.info.id == server_id)?;
        if relayed_name(server_id, tool).as_deref() != Some(name)
            || !allowed(&server.info, tool)
            || !server.info.allows(&ctx.claims.agent)
        {
            return None;
        }
        let thread = ctx.claims.thread;
        let record = match self.thread(thread).await {
            Ok(Some(record)) => record,
            Ok(None) => return Some(Ok(error_result(TASK_OVER))),
            Err(error) => {
                tracing::warn!(%thread, %error, "the relay could not read the thread");
                return Some(Ok(error_result(TEMPORARILY_UNAVAILABLE)));
            }
        };
        // A detached server is not on the endpoint any more.
        if !record.job.tools.contains(&server.info.id) {
            return None;
        }
        if record.state.is_terminal() || record.job.number != ctx.claims.job {
            return Some(Ok(error_result(TASK_OVER)));
        }
        Some(Ok(self.relay(ctx, server, tool, args).await))
    }
}

impl<P: Ports, C: ToolServerClient> RelayTools<P, C> {
    /// The call, as one step.
    async fn relay(
        &self,
        ctx: &ToolCtx,
        server: &Server,
        tool: &str,
        args: JsonObject,
    ) -> CallToolResult {
        let thread = ctx.claims.thread;
        let meta = CallMeta::of(ctx.meta.as_ref());
        let (actor, task) = self.actor_and_task(ctx).await;
        // The agent's call id makes the step's id, so a retry reports the same step again. Without
        // one (or one too long for an id) the step has an id of its own.
        let id = meta
            .call_id
            .as_deref()
            .map(|call| format!("tool-{call}"))
            .filter(|id| id.len() <= MAX_STEP_ID_BYTES)
            .unwrap_or_else(|| format!("tool-{}", self.app.ports().ids().new_id()));
        // The agent's step ids are the task's, prefixed by the adapter (`<task>/<id>`).
        let parent = match (&task, &meta.parent_step_id) {
            (Some(task), Some(parent)) => Some(format!("{task}/{parent}")),
            _ => None,
        };
        let template = StepReport {
            id,
            parent,
            kind: StepKind::Tool,
            label: format!("{} \u{b7} {tool}", server.info.name),
            state: StepState::Running,
            icon: Some(format!(
                "{}{}",
                orch_core::MCP_SERVER_ICON_PREFIX,
                server.info.id
            )),
            detail: None,
            input: None,
            output: None,
        };
        let step =
            CallStep::start(Arc::clone(&self.app), thread, actor, template, args.clone()).await;

        let call = ToolCall::new(tool).with_arguments(args);
        let bound = server.endpoint.timeout + CLIENT_GRACE;
        let upstream = async {
            match tokio::time::timeout(bound, self.client.call_tool(&server.endpoint, &call)).await
            {
                Ok(answer) => answer,
                Err(_) => Err(ToolServerError::TimedOut {
                    after: server.endpoint.timeout,
                }),
            }
        };
        let ended = tokio::select! {
            () = ctx.cancel.cancelled() => Ended {
                result: error_result(CANCELLED),
                state: StepState::Canceled,
                detail: Some(CANCELLED.to_owned()),
                output: None,
            },
            answer = upstream => match answer {
                Ok(output) => answered(output),
                Err(error) => {
                    if matches!(error, ToolServerError::Unauthenticated) {
                        // For the operator: the server's id, never a credential.
                        tracing::warn!(
                            server = %server.info.id,
                            "an MCP server refused the orchestrator's credentials"
                        );
                    }
                    failure(&server.info, &error)
                }
            },
        };
        let Ended {
            result,
            state,
            detail,
            output,
        } = ended;
        step.end(state, detail, output).await;
        result
    }
}

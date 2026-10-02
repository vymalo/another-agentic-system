//! The MCP server: `initialize`, `tools/list` and `tools/call` over [`App`].
//!
//! A tool call is one `App` call made as the token's principal (its user and role, ADR 0033), so it
//! is scoped exactly as the resource API is: a job the caller may not read is "no such job", never
//! "forbidden"; what their roles do not allow is refused as `not permitted`. No tool keeps state
//! in the process; a fresh server is built for every request (stateless mode).

use std::sync::Arc;
use std::time::Duration;

use orch_app::{App, AppError, Creation, Inbound, NewThread};
use orch_core::{AgentId, AgentTarget, Classify, ErrorClass, Input, Origin, ThreadId, report};
use orch_ports::{IdGen, Ports, Principal};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ProgressNotificationParam, ProgressToken, ServerCapabilities,
    ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, Peer, RoleServer, ServerHandler};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::Settings;
use crate::auth::McpUser;
use crate::id::{MAX_CLIENT_REQUEST_ID_BYTES, job_id_for};
use crate::job::{JobSummary, summarise};
use crate::tools::{
    AnswerArgs, CancelJobArgs, GateArgs, GetJobArgs, NoArgs, StartJobArgs, ToolName, WaitForJobArgs,
};
use crate::wait::{ProgressSink, WaitEnd, WaitRequest, effective_timeout, wait_for_job};

/// What the server tells a client about itself, once, at `initialize`.
const INSTRUCTIONS: &str = "Start a job with start_job and look at it with get_job, or follow it \
    with wait_for_job (call it again with after_seq set to its resume_after_seq to keep waiting). \
    A job in state blocked waits for an answer: send it with answer. A finished job is not the \
    end of the thread: answer starts its next job (same job_id, the job number says which). Jobs \
    are the caller's own; a job_id that is not yours is reported as unknown.";

/// The server for one request.
pub(crate) struct McpServer<P: Ports> {
    app: Arc<App<P>>,
    settings: Arc<Settings>,
}

impl<P: Ports> McpServer<P> {
    pub(crate) fn new(app: Arc<App<P>>, settings: Arc<Settings>) -> Self {
        McpServer { app, settings }
    }
}

/// Progress notifications to the client that made the request, when it asked for them (a
/// `progressToken`).
struct PeerSink {
    peer: Peer<RoleServer>,
    token: Option<ProgressToken>,
}

impl ProgressSink for PeerSink {
    async fn send(&self, progress: u64, message: String) -> bool {
        let Some(token) = &self.token else {
            return true;
        };
        // A counter of a few thousand at most: exact in an f64.
        #[allow(clippy::cast_precision_loss)]
        let progress = progress as f64;
        self.peer
            .notify_progress(
                ProgressNotificationParam::new(token.clone(), progress).with_message(message),
            )
            .await
            .is_ok()
    }
}

/// What `wait_for_job` answers: how the wait ended, where to resume, and the job as `get_job`
/// says it.
#[derive(Serialize)]
struct WaitResult {
    outcome: &'static str,
    resume_after_seq: i64,
    /// For `interrupted`: the call was cut short by a restart, and this is a short delay after
    /// which to call again (with `after_seq`), so that clients do not spin against a replica
    /// that is going away.
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
    #[serde(flatten)]
    job: JobSummary,
}

/// The delay an interrupted `wait_for_job` suggests before it is called again.
const RETRY_AFTER_INTERRUPTED_SECS: u64 = 2;

/// Runs a tool that must not stay open (everything but `wait_for_job`) for at most `timeout`:
/// the machine route has no request timeout of its own.
async fn within(
    timeout: Duration,
    tool: ToolName,
    call: impl Future<Output = Result<CallToolResult, ErrorData>>,
) -> Result<CallToolResult, ErrorData> {
    if let Ok(result) = tokio::time::timeout(timeout, call).await {
        result
    } else {
        tracing::warn!(
            tool = tool.name(),
            ?timeout,
            "a tool call took too long and was cut off"
        );
        Ok(refused(format!(
            "{} took longer than {} s and was stopped; try again",
            tool.name(),
            timeout.as_secs().max(1)
        )))
    }
}

/// The principal the bearer check let in. A request that reaches a tool without one did not pass
/// the check, which only a wiring mistake can cause: the call is refused (fail closed).
fn caller(context: &RequestContext<RoleServer>) -> Result<Principal, ErrorData> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<McpUser>())
        .map(|user| user.0.clone())
        .ok_or_else(|| {
            tracing::error!("an MCP request reached a tool without an authenticated user");
            ErrorData::internal_error("the request was not authenticated", None)
        })
}

/// Parses the arguments of a tool call.
fn parse_args<T: DeserializeOwned>(
    arguments: Option<serde_json::Map<String, Value>>,
) -> Result<T, ErrorData> {
    serde_json::from_value(Value::Object(arguments.unwrap_or_default()))
        .map_err(|e| ErrorData::invalid_params(format!("invalid arguments: {e}"), None))
}

/// A successful result: the JSON, as text for the model and as structured content.
fn success(value: &impl Serialize) -> Result<CallToolResult, ErrorData> {
    let value = serde_json::to_value(value).map_err(|e| {
        ErrorData::internal_error(format!("cannot serialise the result: {e}"), None)
    })?;
    Ok(CallToolResult::structured(value))
}

/// A tool-level failure the caller should read.
fn refused(message: impl Into<String>) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message)])
}

/// A job id as the caller wrote it. A malformed one is the same answer as an unknown one.
fn job_id(raw: &str) -> Option<ThreadId> {
    raw.trim().parse::<ThreadId>().ok()
}

/// What an application failure looks like to the caller. The causes of a failure the caller
/// cannot act on are logged, never sent.
fn failure(err: &AppError) -> Result<CallToolResult, ErrorData> {
    match err.class() {
        ErrorClass::NotFound => Ok(refused("no such job")),
        // The roles of the token do not allow it: the detail names a permission, never a job.
        ErrorClass::Forbidden => Ok(refused(format!("not permitted: {err}"))),
        ErrorClass::Invalid => Ok(refused(err.to_string())),
        ErrorClass::Rejected => Ok(refused(match err {
            AppError::Finished => "this card belongs to a finished request".to_owned(),
            other => other.to_string(),
        })),
        ErrorClass::Conflict | ErrorClass::Transient | ErrorClass::RateLimited => {
            tracing::warn!(error = %report(err), "a tool call failed temporarily");
            Ok(refused("temporarily unavailable; try again"))
        }
        // Unauthenticated, unsupported, corrupt, internal, and any class added later: a fault of
        // the server, not something the caller can act on.
        _ => {
            tracing::error!(error = %report(err), class = ?err.class(), "a tool call failed");
            Err(ErrorData::internal_error("internal error", None))
        }
    }
}

impl<P: Ports> McpServer<P> {
    async fn list_agents(&self, who: &Principal) -> Result<CallToolResult, ErrorData> {
        let list = match self.app.list_agents(who).await {
            Ok(list) => list,
            Err(e) => return failure(&e),
        };
        let agents: Vec<Value> = list
            .agents
            .iter()
            .map(|a| {
                json!({
                    "id": a.id,
                    "name": a.name,
                    "description": a.description,
                })
            })
            .collect();
        // Said only when something is missing, so a client that does not know the key sees what it
        // always saw (ADR 0022: a registry that cannot be read lists none of its agents).
        let unavailable: Vec<&str> = list
            .sources
            .iter()
            .filter(|s| !s.available)
            .map(|s| s.name.as_str())
            .collect();
        if unavailable.is_empty() {
            success(&json!({ "agents": agents }))
        } else {
            success(&json!({ "agents": agents, "unavailable_sources": unavailable }))
        }
    }

    async fn start_job(
        &self,
        who: &Principal,
        args: StartJobArgs,
    ) -> Result<CallToolResult, ErrorData> {
        let user = &who.user;
        let client_request_id = match args.client_request_id.as_deref().map(str::trim) {
            None => None,
            Some("") => return Ok(refused("client_request_id must not be empty")),
            Some(id) if id.len() > MAX_CLIENT_REQUEST_ID_BYTES => {
                return Ok(refused(format!(
                    "client_request_id must be at most {MAX_CLIENT_REQUEST_ID_BYTES} bytes"
                )));
            }
            Some(id) => Some(id),
        };
        let gate = match args.gate.as_ref().map(GateArgs::to_layer) {
            None => None,
            Some(Ok(layer)) => Some(layer),
            Some(Err(reason)) => return Ok(refused(format!("invalid gate: {reason}"))),
        };
        let named_agent = args
            .agent
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(AgentId::new);
        let agent_id = match &named_agent {
            Some(id) => id.clone(),
            // The first agent listed (ADR 0014), read now from the registry (ADR 0022).
            None => match self.app.default_agent(who).await {
                Some(id) => id,
                None => {
                    // A token whose roles hold no `agent.invoke` is refused for that; one that
                    // may invoke some agent, and finds none listed, is told so.
                    return match self.app.access(who).check(
                        orch_app::Permission::AgentInvoke,
                        &orch_app::Resource::Anything,
                    ) {
                        Err(_) => failure(&AppError::missing_permission(
                            orch_app::Permission::AgentInvoke,
                        )),
                        Ok(()) => Ok(refused("no agent you may use is configured")),
                    };
                }
            },
        };
        // A retried call names the same job; without a request id every call is new.
        let id = match client_request_id {
            Some(request) => job_id_for(user, request),
            None => ThreadId(self.app.ports().ids().new_id()),
        };
        let new = NewThread {
            title: args.title.clone(),
            target: AgentTarget {
                agent_id,
                release: None,
            },
            text: args.text.clone(),
        };
        let inbound = Inbound {
            origin: Origin::Mcp,
            gate: gate.clone(),
            ..Inbound::default()
        };
        let (thread, created) = match self.app.create_thread_as(who, id, new, inbound).await {
            Ok(Creation::Created { thread, .. }) => (thread, true),
            // The caller's own job with this id: a retry, or a request that lost a race with
            // itself. Nothing was written. It is the same request, or it is refused.
            Ok(Creation::Exists) => {
                let thread = match self.app.get_thread(who, id).await {
                    Ok(thread) => thread,
                    Err(e) => return failure(&e),
                };
                match self
                    .difference_from_first_request(
                        who,
                        &thread,
                        &args,
                        named_agent.as_ref(),
                        gate.as_ref(),
                    )
                    .await
                {
                    Ok(None) => (thread, false),
                    Ok(Some(what)) => {
                        return Ok(refused(format!(
                            "client_request_id {:?} was already used for a different request ({what} \
                             differs from the job it started, {}); use a new client_request_id for a \
                             new job",
                            client_request_id.unwrap_or_default(),
                            thread.id
                        )));
                    }
                    Err(e) => return failure(&e),
                }
            }
            // Only a job id that another user's thread already has: the id is derived, so this is
            // a collision or a thread made before the ids were reserved. Nothing of that thread
            // is said.
            Err(AppError::NotFound) if client_request_id.is_some() => {
                tracing::warn!(%id, "start_job: the job id derived for a client_request_id is taken");
                return Ok(refused(
                    "this client_request_id cannot be used (its job id is taken); use another one",
                ));
            }
            Err(e) => return failure(&e),
        };
        let mut result = json!({
            "job_id": thread.id,
            "state": thread.state.as_str(),
            "created": created,
        });
        if let (Some(url), Some(object)) =
            (self.settings.web_url(thread.id), result.as_object_mut())
        {
            object.insert("web_url".to_owned(), Value::String(url));
        }
        success(&result)
    }

    /// What of a repeated `start_job` differs from the request the job was started by: the text of
    /// its first message, the agent and title when the repeat names them, and the gate when it
    /// asks for one that would change the job's. `None` when it is the same request.
    async fn difference_from_first_request(
        &self,
        who: &Principal,
        thread: &orch_core::ThreadRecord,
        args: &StartJobArgs,
        named_agent: Option<&AgentId>,
        gate: Option<&orch_app::GateLayer>,
    ) -> Result<Option<&'static str>, AppError> {
        let first = self.app.list_events(who, thread.id, 0, 1).await?;
        let first_text = first.first().and_then(|e| match &e.body {
            orch_core::EventBody::UserMessage(m) => Some(m.text.as_str()),
            _ => None,
        });
        if first_text != Some(args.text.as_str()) {
            return Ok(Some("the text"));
        }
        if named_agent.is_some_and(|a| a != &thread.target.agent_id) {
            return Ok(Some("the agent"));
        }
        if args.title.as_deref().is_some_and(|t| t != thread.title) {
            return Ok(Some("the title"));
        }
        if let Some(layer) = gate
            && self.app.gate_request_changes(&thread.job.gate, layer)?
        {
            return Ok(Some("the gate"));
        }
        Ok(None)
    }

    async fn get_job(
        &self,
        who: &Principal,
        args: GetJobArgs,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        let thread = match self.app.get_thread(who, id).await {
            Ok(thread) => thread,
            Err(e) => return failure(&e),
        };
        match summarise(&self.app, who, &thread).await {
            Ok(summary) => success(&summary),
            Err(e) => failure(&e),
        }
    }

    async fn wait_for_job(
        &self,
        who: &Principal,
        args: WaitForJobArgs,
        context: &RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        if args.after_seq.is_some_and(|after| after < 0) {
            return Ok(refused("after_seq must not be negative"));
        }
        let token = context.meta.get_progress_token();
        // A wait stays open; only so many are allowed at once, per process and per user.
        let _slot = match self.settings.waits.acquire(&who.user) {
            Ok(slot) => slot,
            Err(busy) => return Ok(refused(busy.message())),
        };
        let request = WaitRequest {
            after_seq: args.after_seq,
            timeout: effective_timeout(
                Duration::from_secs(args.timeout_secs),
                self.settings.wait_max,
                self.settings.heartbeat,
                token.is_some(),
            ),
            heartbeat: self.settings.heartbeat,
        };
        let sink = PeerSink {
            peer: context.peer.clone(),
            token,
        };
        let waited = match wait_for_job(&self.app, who, id, &request, &sink, &context.ct).await {
            Ok(waited) => waited,
            Err(e) => return failure(&e),
        };
        match summarise(&self.app, who, &waited.thread).await {
            Ok(job) => success(&WaitResult {
                outcome: waited.end.as_str(),
                resume_after_seq: waited.resume_after_seq,
                retry_after_secs: (waited.end == WaitEnd::Interrupted)
                    .then_some(RETRY_AFTER_INTERRUPTED_SECS),
                job,
            }),
            Err(e) => failure(&e),
        }
    }

    async fn answer(&self, who: &Principal, args: AnswerArgs) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        let input = Input::UserMessage {
            user: who.user.clone(),
            text: args.text,
            message_id: None,
            run_id: None,
            origin: Origin::Mcp,
            catalog: None,
            mentions: Vec::new(),
        };
        // No idempotency key: an `answer` is not made safe to retry (ADR 0019).
        match self.app.submit(who, id, input, None).await {
            Ok(orch_app::ApplyOutcome::Applied { thread, .. }) => success(&json!({
                "job_id": thread.id,
                "state": thread.state.as_str(),
                "job": thread.job.number,
            })),
            Ok(orch_app::ApplyOutcome::Duplicate | orch_app::ApplyOutcome::Fenced) => {
                tracing::error!("a message without an idempotency key or a lease was not applied");
                Err(ErrorData::internal_error("internal error", None))
            }
            Err(e) => failure(&e),
        }
    }

    async fn cancel_job(
        &self,
        who: &Principal,
        args: CancelJobArgs,
    ) -> Result<CallToolResult, ErrorData> {
        let Some(id) = job_id(&args.job_id) else {
            return Ok(refused("no such job"));
        };
        // Cancelling a finished job is a no-op, as in the chat.
        if let Err(e) = self.app.cancel(who, id).await {
            return failure(&e);
        }
        let thread = match self.app.get_thread(who, id).await {
            Ok(thread) => thread,
            Err(e) => return failure(&e),
        };
        // The agent confirms a cancel later: until then the job is still what it was.
        success(&json!({
            "job_id": thread.id,
            "state": thread.state.as_str(),
            "finished": thread.state.is_terminal(),
        }))
    }
}

impl<P: Ports> ServerHandler for McpServer<P> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            ToolName::ALL.iter().map(|t| t.definition()).collect(),
        ))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        ToolName::parse(name).map(ToolName::definition)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let user = caller(&context)?;
        let Some(tool) = ToolName::parse(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool {:?}", request.name),
                None,
            ));
        };
        let limit = self.settings.tool_timeout;
        let result = match tool {
            ToolName::ListAgents => {
                parse_args::<NoArgs>(request.arguments)?;
                within(limit, tool, self.list_agents(&user)).await
            }
            ToolName::StartJob => {
                let args = parse_args(request.arguments)?;
                within(limit, tool, self.start_job(&user, args)).await
            }
            ToolName::GetJob => {
                let args = parse_args(request.arguments)?;
                within(limit, tool, self.get_job(&user, args)).await
            }
            // The one tool that is meant to stay open: bounded by its own timeout and slots.
            ToolName::WaitForJob => {
                let args = parse_args(request.arguments)?;
                self.wait_for_job(&user, args, &context).await
            }
            ToolName::Answer => {
                let args = parse_args(request.arguments)?;
                within(limit, tool, self.answer(&user, args)).await
            }
            ToolName::CancelJob => {
                let args = parse_args(request.arguments)?;
                within(limit, tool, self.cancel_job(&user, args)).await
            }
        };
        result.map(CallToolResponse::from)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_tool_that_takes_too_long_is_stopped_with_a_message_and_a_fast_one_is_not() {
        let slow = within(Duration::from_secs(30), ToolName::GetJob, async {
            std::future::pending::<()>().await;
            Ok(CallToolResult::success(vec![]))
        })
        .await
        .unwrap();
        assert_eq!(slow.is_error, Some(true));
        let text = format!("{:?}", slow.content);
        assert!(text.contains("get_job took longer than 30 s"), "{text}");

        let fast = within(Duration::from_secs(30), ToolName::GetJob, async {
            Ok(CallToolResult::success(vec![ContentBlock::text("ok")]))
        })
        .await
        .unwrap();
        assert_eq!(fast.is_error, Some(false));
    }
}

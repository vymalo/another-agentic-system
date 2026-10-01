//! The seam for the tools that later slices add to the endpoint: a provider answers `tools/list`
//! with its own tools and `tools/call` for the names it owns.
//!
//! Not a port (ADR 0009): a provider is part of this surface's own composition. The binary picks
//! which providers it builds in and gives them to [`ThreadToolsConfig::with_provider`]
//! (crate::ThreadToolsConfig::with_provider) when it mounts the surface; nothing is loaded at run
//! time. The endpoint answers `tools/list` with the built-in tools first and then each provider's,
//! in the order they were added, and offers a `tools/call` to the built-ins and then to the
//! providers in that order; the first that owns the name answers.

use std::future::Future;

use futures::future::BoxFuture;
use orch_core::UserId;
use orch_thread_token::Claims;
use rmcp::model::{CallToolResult, JsonObject, ProgressNotificationParam, ProgressToken, Tool};
use rmcp::{ErrorData, Peer, RoleServer};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Progress notifications to the client that made the request, when it asked for them (a
/// `progressToken` in the call's `_meta`). Without a token every `send` is a no-op that says the
/// client is still there, so a tool reports progress without asking whether anyone listens.
#[derive(Debug, Clone)]
pub struct ProgressSink {
    peer: Peer<RoleServer>,
    token: Option<ProgressToken>,
}

impl ProgressSink {
    pub(crate) fn new(peer: Peer<RoleServer>, token: Option<ProgressToken>) -> Self {
        ProgressSink { peer, token }
    }

    /// Whether the client asked for progress notifications.
    pub fn is_listening(&self) -> bool {
        self.token.is_some()
    }

    /// Sends one notification. `progress` is 1 for the first and larger for every later one.
    /// `false` means the client is gone, and the call is to end.
    pub async fn send(&self, progress: u64, message: String) -> bool {
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

/// What a tool knows of the call it serves. Built for each request: the endpoint holds no state.
#[derive(Debug, Clone)]
pub struct ToolCtx {
    /// The thread's owner: whose thread this is. The token authorised the thread; this is the
    /// person on whose behalf a tool acts (a relayed call, a nested ask).
    pub owner: UserId,
    /// What the verified token says: the thread, the job, the agent, the caller and the depth.
    pub claims: Claims,
    /// The request's `_meta` (the progress token included), `None` when the client sent none. A
    /// provider reads its own namespaced member of it, such as
    /// `https://agents.vymalo.com/a2a/extensions/thread-tools/v1`.
    pub meta: Option<serde_json::Map<String, Value>>,
    /// Where to report progress.
    pub progress: ProgressSink,
    /// Cancelled when the client cancels the request or goes away: a tool that waits stops.
    pub cancel: CancellationToken,
}

/// A source of tools on the endpoint besides the built-in ones.
///
/// Both methods are asked **for each request**: a provider keeps no per-thread state between
/// calls, so any replica serves any request. `list` must not fail the whole listing: a provider
/// that cannot list leaves its tools out (and says why in its own log). `call` answers `None`
/// for a name that is not its own, so the next provider is asked; `Some(Err(..))` is a protocol
/// error (JSON-RPC), `Some(Ok(result))` with `is_error` a failure of the tool itself.
///
/// A provider bounds its own calls. The endpoint cuts the built-in tools off after
/// [`DEFAULT_TOOL_TIMEOUT`](crate::DEFAULT_TOOL_TIMEOUT), but a provider's tool may take much
/// longer (a nested ask runs another agent) and reports progress while it does.
pub trait ThreadToolProvider: Send + Sync + 'static {
    /// The tools this provider offers to this call's thread and caller.
    fn list(&self, ctx: &ToolCtx) -> impl Future<Output = Vec<Tool>> + Send;

    /// Runs the tool `name` with `args` if it is this provider's; `None` if it is not.
    fn call(
        &self,
        ctx: &ToolCtx,
        name: &str,
        args: JsonObject,
    ) -> impl Future<Output = Option<Result<CallToolResult, ErrorData>>> + Send;
}

/// [`ThreadToolProvider`] with its futures boxed, so that providers of different types are one
/// list. Every provider is one, through the blanket implementation.
pub(crate) trait DynProvider: Send + Sync + 'static {
    fn list<'a>(&'a self, ctx: &'a ToolCtx) -> BoxFuture<'a, Vec<Tool>>;

    fn call<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        name: &'a str,
        args: JsonObject,
    ) -> BoxFuture<'a, Option<Result<CallToolResult, ErrorData>>>;
}

impl<T: ThreadToolProvider> DynProvider for T {
    fn list<'a>(&'a self, ctx: &'a ToolCtx) -> BoxFuture<'a, Vec<Tool>> {
        Box::pin(ThreadToolProvider::list(self, ctx))
    }

    fn call<'a>(
        &'a self,
        ctx: &'a ToolCtx,
        name: &'a str,
        args: JsonObject,
    ) -> BoxFuture<'a, Option<Result<CallToolResult, ErrorData>>> {
        Box::pin(ThreadToolProvider::call(self, ctx, name, args))
    }
}

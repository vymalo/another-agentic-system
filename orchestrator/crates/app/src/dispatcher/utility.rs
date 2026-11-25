//! The model call the utility tasks share (ADR 0035): the title worker and the description worker
//! ask a model the same way, and neither fails a thread when it cannot be had.
//!
//! Since card ANR-670 the call is billed to the thread's owner, so the worker **binds a lease for
//! it first**, in this same step and before the model is asked: the deployment's model injector
//! resolves the lease by the id the request carries and stamps `x-account-id` with its account,
//! which is the thread's owner (`UserId`, ADR 0065). The id is a **lease id**, `thread:<thread
//! id>:<property>` — a selector the caller asserts, not proof of identity (ADR 0084).
//!
//! The key is the `(thread, property)` pair, not the call: a title is asked once per thread and
//! the outbox retries the row, so binding again must answer the *same* lease (the broker treats
//! the same binding as idempotent) and a different person for the same key is `409
//! run_already_bound`, never a re-point. Nothing here is the spend: the ledger settles against
//! the account the injector stamped, once per lease.
//!
//! A binding that cannot be had is **not** a title that cannot be had: the row is left to be
//! retried, and the model is not asked. Proceeding without a lease would be refused by the
//! injector as an unauthenticated call (or, worse, be billed to nobody at an endpoint that does
//! not inject), so the failure stays where its cause is.

use std::time::Duration;

use orch_core::{Classify, ErrorClass, TaskKind, report};
use orch_ports::{
    ChatModel, ChatRequest, ModelError, OutboxItem, Ports, RunBinding, RunBindings, RunId,
    ThreadStore,
};

use super::{DispatchError, Dispatcher};
use crate::TaskSettings;

/// How many times a model that stumbles is asked before the request is declined.
const ATTEMPTS: u32 = 3;

/// How long a utility call's lease lives. The row unbinds it when the call is over; the deadline
/// only bounds a worker that crashed between the bind and the unbind.
pub const UTILITY_BINDING_TTL: Duration = Duration::from_secs(5 * 60);

impl<P: Ports, B: RunBindings> Dispatcher<P, B> {
    /// The model's answer to `request`, or `None` when it cannot be had (said in the log of the
    /// process, as `failure`, never in the thread), under the lease of `row`'s thread.
    ///
    /// A transient failure (a timeout, a 5xx, a rate limit with its `Retry-After`) is tried up to
    /// three times with the dispatcher's backoff; a refusal for good, a credential the endpoint
    /// does not take, and an endpoint the model does not hold (a bug: the configuration checks
    /// that every task names one) are given up at once. Each try is bounded by the task's timeout
    /// whatever the adapter does.
    ///
    /// # Errors
    /// [`DispatchError::Lease`] when the call's lease could not be bound: the row is retried and
    /// the model is not asked.
    pub(super) async fn ask_model(
        &self,
        row: &OutboxItem,
        kind: TaskKind,
        task: &TaskSettings,
        request: &ChatRequest,
        failure: &str,
    ) -> Result<Option<String>, DispatchError> {
        let Some(run) = self.bind_utility_lease(row, kind, task).await? else {
            return Ok(None);
        };
        let request = ChatRequest {
            run: Some(run.clone()),
            ..request.clone()
        };
        let answer = self.ask_under(row, task, &request, failure).await;
        self.unbind_utility_lease(&run, row).await;
        Ok(answer)
    }

    /// The attempts of [`Self::ask_model`] under the lease `request` already names.
    async fn ask_under(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        request: &ChatRequest,
        failure: &str,
    ) -> Option<String> {
        for attempt in 1..=ATTEMPTS {
            let outcome =
                tokio::time::timeout(task.timeout, self.app.ports().model().complete(request))
                    .await
                    .unwrap_or_else(|_| {
                        Err(ModelError::unreachable("the model did not answer in time"))
                    });
            let err = match outcome {
                Ok(text) => return Some(text),
                Err(err) => err,
            };
            let retry = matches!(err.class(), ErrorClass::Transient | ErrorClass::RateLimited);
            if !retry || attempt == ATTEMPTS {
                tracing::warn!(
                    id = %row.id,
                    attempt,
                    endpoint = %request.endpoint,
                    error = %report(&err),
                    "{failure}"
                );
                return None;
            }
            let wait = err
                .retry_after()
                .unwrap_or_else(|| self.backoff(attempt))
                .min(self.cfg.backoff_max);
            tracing::debug!(id = %row.id, attempt, error = %report(&err), "asking the model again");
            tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
        }
        None
    }

    /// Binds the lease of this utility call: `(thread, property)`, the thread's agent, the thread's
    /// **owner** as the person, and the task's endpoint as the model. Idempotent for the same key;
    /// a different person for it is the broker's `409 run_already_bound`.
    ///
    /// A thread that is gone asks nothing (`Ok(None)`): the caller's own read has already decided,
    /// or the row ends with the thread.
    async fn bind_utility_lease(
        &self,
        row: &OutboxItem,
        kind: TaskKind,
        task: &TaskSettings,
    ) -> Result<Option<RunId>, DispatchError> {
        let Some(thread) = self.store().get_thread(None, row.thread_id).await? else {
            // The thread went away under the row: nothing to title, and a retry cannot help.
            return Ok(None);
        };
        let run = RunId::new(format!("thread:{}:{}", row.thread_id, kind.as_str()));
        let binding = RunBinding {
            run: run.clone(),
            agent: thread.target.agent_id.clone(),
            person: thread.owner.clone(),
            model: Some(task.endpoint.clone()),
            expires_at: super::add(self.now(), UTILITY_BINDING_TTL),
        };
        match self.run_bindings.bind(binding).await {
            Ok(()) => {
                // Never the person: the account is the injector's, and a log is not a ledger.
                tracing::debug!(
                    lease = %run,
                    thread = %row.thread_id,
                    "the utility call's lease is bound"
                );
                Ok(Some(run))
            }
            Err(e) => {
                tracing::warn!(
                    lease = %run,
                    thread = %row.thread_id,
                    error = %e,
                    "the utility call's lease could not be bound; the row waits and no model is asked"
                );
                Err(DispatchError::Lease(e.to_string()))
            }
        }
    }

    /// Removes the call's lease. Best effort: a lease left behind is bounded by
    /// [`UTILITY_BINDING_TTL`] and spends nothing.
    async fn unbind_utility_lease(&self, run: &RunId, row: &OutboxItem) {
        if let Err(e) = self.run_bindings.unbind(run).await {
            tracing::warn!(
                lease = %run,
                thread = %row.thread_id,
                error = %e,
                "unbinding the utility call's lease failed; it lapses on its own"
            );
        }
    }
}

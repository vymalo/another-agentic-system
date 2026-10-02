//! The model call the utility tasks share (ADR 0035): the title worker and the description worker
//! ask a model the same way, and neither fails a thread when it cannot be had.

use std::time::Duration;

use orch_core::{Classify, ErrorClass, report};
use orch_ports::{ChatModel, ChatRequest, ModelError, OutboxItem, Ports};

use super::Dispatcher;
use crate::TaskSettings;

/// How many times a model that stumbles is asked before the request is declined.
const ATTEMPTS: u32 = 3;

impl<P: Ports> Dispatcher<P> {
    /// The model's answer to `request`, or `None` when it cannot be had (said in the log of the
    /// process, as `failure`, never in the thread).
    ///
    /// A transient failure (a timeout, a 5xx, a rate limit with its `Retry-After`) is tried up to
    /// three times with the dispatcher's backoff; a refusal for good, a credential the endpoint
    /// does not take, and an endpoint the model does not hold (a bug: the configuration checks
    /// that every task names one) are given up at once. Each try is bounded by the task's timeout
    /// whatever the adapter does.
    pub(super) async fn ask_model(
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
}

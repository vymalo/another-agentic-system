//! The verifier path of the dispatcher (ADR 0018): an outbox row of kind `verify` asks the
//! verifier agent to review the commit the worker pushed, and the answer goes back into the
//! thread as exactly one input.
//!
//! What sets it apart from a delegation:
//!
//! * **The verifier is not the worker.** Its envelopes are never turned into
//!   [`Input::Agent`](orch_core::Input::Agent): its `completed` must not complete the job, and
//!   what it says is not the worker's. They are read here and nowhere else. The one thing that
//!   comes out is [`Input::VerifierReported`] from the `verdict` artifact (a verifier that
//!   finishes without one, or with one that cannot be used, has failed the work: fail closed),
//!   or [`Input::VerifierFailed`] when no answer can be had.
//! * **Its own conversation.** The request goes to the verifier in the context
//!   [`verifier_context`], never the thread's, and the task it creates is recorded on the row
//!   ([`OutboxItem::task_id`]), never on the thread's binding.
//! * **Every write is fenced** with the row's claim, like the delegation path: the verdict, the
//!   record that the message was sent, the retries and the end of the row. A worker that lost
//!   its claim writes nothing.
//! * **It ends when it is no longer wanted.** The verification is the one in progress
//!   (`Verifying`, this attempt, this verification, no verdict yet) or it is nothing: a timeout,
//!   a cancel, a user who wrote, or another source that already failed the round make it
//!   obsolete, and the row looks for that every [`DispatcherConfig::verify_watch`] while it waits
//!   for the verifier, so a verifier that hangs does not hold a worker forever. The look happens
//!   only where nothing is half done: between two envelopes, or between two polls. A request is
//!   never abandoned between being sent and being recorded, and a verdict that is being
//!   committed is finished, not cancelled. The core also refuses a late answer by itself (the
//!   verdict carries its attempt and verification), so this is the resource-saving half, not the
//!   safety half.
//! * **A crash does not ask twice, as far as the verifier lets it be known.** A row that was
//!   sent has its task on it: the next claimant re-attaches to that task (`resubscribe`, then
//!   `get_task`). One that crashed between sending and recording looks the message up by its id
//!   (`find_task_by_message`, retried when it fails) and sends it again only when the lookup
//!   *says* there is no such task: an agent that cannot be asked (no `ListTasks`) is the one case
//!   in which the request may be sent twice, and its context and message id are unique, so it
//!   can tell. The verdict is applied under the key `verdict:<row>`, so the one verdict is one
//!   event whoever applies it.
//! * **A verdict is not lost to a re-attachment.** A resubscription yields the rest of the
//!   stream, not what came before: a task that completes there without a verdict is read with
//!   `get_task` (which holds its artifacts) before "no verdict" is concluded.
//!
//! [`DispatcherConfig::verify_watch`]: super::DispatcherConfig::verify_watch

use std::time::Duration;

use futures::StreamExt;
use orch_core::{
    AgentTaskState, AgentUpdate, CheckSource, Classify, Input, ThreadId, ThreadRecord, ThreadState,
    Verdict, parse_verdict, report, verifier_context,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentError, AgentStream, OutboxFinal, OutboxItem, OutboxPayload,
    Ports, SendContent, SendRequest, TaskHandle, ThreadStore,
};
use tokio::time::MissedTickBehavior;

use super::{DispatchError, Dispatcher, Done, env_state};

/// The name of the artifact that carries the verifier's answer.
const VERDICT_ARTIFACT: &str = "verdict";

/// Most bytes of a `verdict` artifact that are read: a verifier is untrusted, and what it sends
/// is parsed and held in memory before the findings are capped.
const MAX_VERDICT_BYTES: usize = 256 * 1024;

/// One verification, as the row that asks for it describes it.
struct Verification {
    row: OutboxItem,
    thread: ThreadId,
    attempt: u32,
    verification: u32,
    endpoint: AgentEndpoint,
    /// The verifier's A2A context for this verification.
    context: String,
}

/// What the verifier has said so far.
#[derive(Default)]
struct Answer {
    /// The latest `verdict` artifact, already read (an unusable one is a failed verdict).
    verdict: Option<Verdict>,
}

impl Answer {
    fn note(&mut self, update: &Option<AgentUpdate>) {
        let Some(AgentUpdate::Artifact { name, text, .. }) = update else {
            return;
        };
        if name != VERDICT_ARTIFACT {
            return;
        }
        self.verdict = Some(match text {
            Some(t) if t.len() > MAX_VERDICT_BYTES => Verdict::unusable(&format!(
                "it is larger than {} KiB",
                MAX_VERDICT_BYTES / 1024
            )),
            other => parse_verdict(other.as_deref()).unwrap_or_else(|why| Verdict::unusable(&why)),
        });
    }
}

/// How consuming the verifier's stream ended.
enum Flow {
    /// The task ended its turn.
    Ended(AgentTaskState),
    /// The stream broke after the message was recorded as sent.
    Disconnected,
    /// The claim is gone; stop touching anything.
    Lost,
    /// The stream failed before the message was recorded as sent.
    Failed(AgentError),
    /// The verification is over while it was being waited for; the task, if any, is recorded.
    Unwanted,
}

/// Whether the verification `attempt`/`verification` is still the one the thread waits on.
fn wanted(thread: &ThreadRecord, attempt: u32, verification: u32) -> bool {
    let job = &thread.job;
    thread.state == ThreadState::Verifying
        && job.attempt == attempt
        && job.verification == verification
        && job.gate.requires(CheckSource::Verifier)
        && !job
            .results
            .iter()
            .any(|r| r.source == CheckSource::Verifier && r.attempt == attempt)
}

impl<P: Ports> Dispatcher<P> {
    /// Processes one `verify` row. See the module documentation.
    pub(super) async fn verify(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Verify {
            attempt,
            verification,
            verifier,
            text,
            ..
        } = row.payload.clone()
        else {
            return self
                .finish(
                    &row,
                    OutboxFinal::Dead {
                        error: "payload does not match kind".to_owned(),
                    },
                )
                .await;
        };
        let Some(thread) = self.store().get_thread(None, row.thread_id).await? else {
            return self.finish(&row, OutboxFinal::Skipped).await;
        };
        let endpoint = self
            .app
            .directory()
            .get(&verifier)
            .map(|entry| entry.endpoint.clone());
        if !wanted(&thread, attempt, verification) {
            return self.drop_verification(&row, endpoint.as_ref()).await;
        }
        let Some(endpoint) = endpoint else {
            return self
                .verifier_down(
                    &row,
                    (attempt, verification),
                    format!("the verifier agent '{verifier}' is no longer configured"),
                    "verifier agent not configured".to_owned(),
                )
                .await;
        };
        let v = Verification {
            context: verifier_context(row.thread_id, attempt, verification),
            thread: row.thread_id,
            attempt,
            verification,
            endpoint,
            row,
        };
        self.ask_verifier(&v, text).await
    }

    /// Whether the verification is still the one the thread waits on. A thread that cannot be
    /// read now is taken to be waiting: the look comes again.
    async fn still_wanted(&self, v: &Verification) -> bool {
        match self.store().get_thread(None, v.thread).await {
            Ok(Some(thread)) => wanted(&thread, v.attempt, v.verification),
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(id = %v.row.id, error = %report(&e), "looking at the thread failed");
                true
            }
        }
    }

    /// The verification is over while the verifier was being waited for.
    async fn unwanted(&self, v: &Verification) -> Done {
        tracing::debug!(id = %v.row.id, "the verification is over; dropping the row");
        self.drop_verification(&v.row, Some(&v.endpoint)).await
    }

    /// The row is obsolete: ask the verifier to stop, if it has a task, and finish the row.
    async fn drop_verification(&self, row: &OutboxItem, endpoint: Option<&AgentEndpoint>) -> Done {
        // The task may have been recorded after this claim was handed out.
        let task = match self.store().get_outbox(row.id).await? {
            Some(current) => current.task_id,
            None => row.task_id.clone(),
        };
        if let (Some(endpoint), Some(task_id)) = (endpoint, task) {
            let handle = TaskHandle {
                endpoint: endpoint.clone(),
                task_id,
            };
            if let Err(e) = self.app.ports().agents().cancel(&handle).await {
                tracing::debug!(id = %row.id, error = %report(&e), "the verifier did not cancel");
            }
        }
        self.finish(row, OutboxFinal::Skipped).await
    }

    /// Sends the request, or re-attaches to the task an earlier claim started.
    async fn ask_verifier(&self, v: &Verification, text: String) -> Done {
        let mut answer = Answer::default();
        if v.row.sent_at.is_some() {
            let Some(task_id) = v.row.task_id.clone() else {
                return Err(
                    orch_ports::StoreError::corrupt("sent verification without a task id").into(),
                );
            };
            return self.follow_verifier(v, task_id, &mut answer).await;
        }
        // A previous claim may have reached the verifier before it crashed or lost the claim.
        // Only "there is no such task" lets the request go out again: a lookup that failed does
        // not know, and asking twice is worse than waiting (the row is retried, or the thread
        // held for the user).
        if v.row.attempts > 1 {
            match self.find_sent(v).await {
                Ok(Some(task_id)) => {
                    if !self
                        .store()
                        .mark_verify_sent(&self.lease(&v.row), task_id.clone(), self.now())
                        .await?
                    {
                        return Ok(());
                    }
                    return self.follow_verifier(v, task_id, &mut answer).await;
                }
                Ok(None) => {}
                Err(e) => return self.verifier_send_failed(v, e).await,
            }
        }
        let request = SendRequest {
            endpoint: v.endpoint.clone(),
            message_id: v.row.id.to_string(),
            context_id: v.context.clone(),
            task_id: None,
            // never a reference: the verifier has a context of its own and is told nothing of
            // the author's tasks (ADR 0002, ADR 0021)
            reference_task_ids: Vec::new(),
            content: SendContent::Text(text),
            release: None,
            // the verifier is told nothing of the author's screen and gets no tools on the
            // author's thread
            ui_catalog: None,
            thread_tools: None,
            // nor of the conversation a fork of the author's thread continues (ADR 0002)
            history: None,
            steer: false,
            mentions: Vec::new(),
        };
        match self.app.ports().agents().send_stream(request).await {
            Ok(stream) => match self.verifier_stream(v, stream, true, &mut answer).await? {
                Flow::Ended(state) => self.answered(v, answer, state).await,
                Flow::Lost => Ok(()),
                Flow::Unwanted => self.unwanted(v).await,
                Flow::Disconnected => {
                    let task = self
                        .store()
                        .get_outbox(v.row.id)
                        .await?
                        .and_then(|r| r.task_id)
                        .ok_or_else(|| {
                            DispatchError::from(crate::AppError::internal(
                                "task id missing after send",
                            ))
                        })?;
                    self.follow_verifier(v, task, &mut answer).await
                }
                Flow::Failed(e) => self.verifier_send_failed(v, e).await,
            },
            Err(e) => self.verifier_send_failed(v, e).await,
        }
    }

    /// Looks for the task an earlier claim may have started for this row's message.
    async fn find_sent(&self, v: &Verification) -> Result<Option<String>, AgentError> {
        self.find_by_message(&v.endpoint, &v.context, &v.row).await
    }

    /// Reads the verifier's stream until its task ends its turn. With `mark_first`, the first
    /// envelope records the message as sent (and the task) before anything else happens.
    async fn verifier_stream(
        &self,
        v: &Verification,
        mut stream: AgentStream,
        mark_first: bool,
        answer: &mut Answer,
    ) -> Result<Flow, DispatchError> {
        let mut marked = !mark_first;
        let period = self.cfg.verify_watch.max(Duration::from_millis(1));
        let mut watch = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        watch.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            // Only the wait for the next envelope can be interrupted, by the look at the thread:
            // recording the task and reading an envelope are never cut in half. Until the first
            // envelope is recorded the task is unknown and cannot be cancelled, so nothing looks.
            let next = tokio::select! {
                next = stream.next() => next,
                _ = watch.tick(), if marked => {
                    if self.still_wanted(v).await {
                        continue;
                    }
                    return Ok(Flow::Unwanted);
                }
            };
            match next {
                Some(Ok(env)) => {
                    if !marked {
                        if !self
                            .store()
                            .mark_verify_sent(&self.lease(&v.row), env.task_id.clone(), self.now())
                            .await?
                        {
                            return Ok(Flow::Lost);
                        }
                        marked = true;
                    }
                    answer.note(&env.update);
                    if let Some(state) = env_state(&env).filter(|s| s.ends_turn()) {
                        return Ok(Flow::Ended(state));
                    }
                }
                Some(Err(e)) => {
                    return Ok(if marked {
                        Flow::Disconnected
                    } else {
                        Flow::Failed(e)
                    });
                }
                None => {
                    return Ok(if marked {
                        Flow::Disconnected
                    } else {
                        Flow::Failed(AgentError::protocol("stream ended before the first update"))
                    });
                }
            }
        }
    }

    /// Re-attaches to a task that was sent: resubscribe, else poll `get_task`.
    async fn follow_verifier(
        &self,
        v: &Verification,
        task_id: String,
        answer: &mut Answer,
    ) -> Done {
        let handle = TaskHandle {
            endpoint: v.endpoint.clone(),
            task_id,
        };
        match self.app.ports().agents().resubscribe(&handle).await {
            Ok(stream) => match self.verifier_stream(v, stream, false, answer).await? {
                // The stream a resubscription gives is the rest of the task, not the start: a
                // verdict streamed before this claim, or before the stream broke, is not in it.
                // The task itself holds its artifacts; read it before saying there was none.
                Flow::Ended(AgentTaskState::Completed) if answer.verdict.is_none() => {
                    tracing::debug!(id = %v.row.id, "completed without a verdict on the rest of the stream; reading the task");
                }
                Flow::Ended(state) => return self.answered(v, std::mem::take(answer), state).await,
                Flow::Lost => return Ok(()),
                Flow::Unwanted => return self.unwanted(v).await,
                Flow::Disconnected | Flow::Failed(_) => {}
            },
            Err(e) => {
                tracing::debug!(id = %v.row.id, error = %report(&e), "resubscribe unavailable; polling");
            }
        }
        self.poll_verifier(v, &handle, answer).await
    }

    /// Polls `get_task` with backoff until the verifier's task ends its turn.
    async fn poll_verifier(
        &self,
        v: &Verification,
        handle: &TaskHandle,
        answer: &mut Answer,
    ) -> Done {
        let mut delay = self.cfg.poll_min;
        let mut failures = 0_u32;
        loop {
            if !self.still_wanted(v).await {
                return self.unwanted(v).await;
            }
            match self.app.ports().agents().get_task(handle).await {
                Ok(snap) => {
                    failures = 0;
                    for env in &snap.envelopes {
                        answer.note(&env.update);
                    }
                    if snap.state.ends_turn() {
                        return self.answered(v, std::mem::take(answer), snap.state).await;
                    }
                }
                Err(AgentError::TaskNotFound(m)) => {
                    let reason = format!("the verifier lost the task: {m}");
                    return self
                        .verifier_down(&v.row, (v.attempt, v.verification), reason.clone(), reason)
                        .await;
                }
                Err(e) if e.is_retryable() => {
                    failures += 1;
                    tracing::warn!(id = %v.row.id, failures, error = %report(&e), "polling the verifier failed");
                    if failures >= self.cfg.max_poll_failures {
                        return self
                            .verifier_down(
                                &v.row,
                                (v.attempt, v.verification),
                                e.public_detail(),
                                report(&e),
                            )
                            .await;
                    }
                }
                Err(e) => {
                    return self
                        .verifier_down(
                            &v.row,
                            (v.attempt, v.verification),
                            e.public_detail(),
                            report(&e),
                        )
                        .await;
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(self.cfg.poll_max);
        }
    }

    /// The verifier's task ended its turn in `state`. Only `completed` answers; every other end
    /// leaves the work unjudged, which is a hold, not a verdict.
    async fn answered(&self, v: &Verification, answer: Answer, state: AgentTaskState) -> Done {
        match state {
            AgentTaskState::Completed => {
                // No `verdict` artifact is a failed check: a verifier that says nothing has not
                // passed the work.
                let verdict = answer.verdict.unwrap_or_else(Verdict::missing);
                let input = Input::VerifierReported {
                    attempt: v.attempt,
                    verification: v.verification,
                    verdict,
                };
                self.apply_quiet(&v.row, input, format!("verdict:{}", v.row.id))
                    .await?;
                self.finish(&v.row, OutboxFinal::Delivered).await
            }
            AgentTaskState::InputRequired | AgentTaskState::AuthRequired => {
                // Nobody can answer the verifier; let it go (when its task is known).
                let task_id = self
                    .store()
                    .get_outbox(v.row.id)
                    .await?
                    .and_then(|r| r.task_id);
                if let Some(task_id) = task_id {
                    let handle = TaskHandle {
                        endpoint: v.endpoint.clone(),
                        task_id,
                    };
                    if let Err(e) = self.app.ports().agents().cancel(&handle).await {
                        tracing::debug!(id = %v.row.id, error = %report(&e), "the verifier did not cancel");
                    }
                }
                let reason = "the verifier asked for input, which it cannot be given".to_owned();
                self.verifier_down(&v.row, (v.attempt, v.verification), reason.clone(), reason)
                    .await
            }
            AgentTaskState::Failed | AgentTaskState::Rejected | AgentTaskState::Canceled => {
                let reason = format!("the verifier's task ended without an answer ({state:?})");
                self.verifier_down(&v.row, (v.attempt, v.verification), reason.clone(), reason)
                    .await
            }
            AgentTaskState::Submitted | AgentTaskState::Working => {
                let reason = "the verifier's task did not end".to_owned();
                self.verifier_down(&v.row, (v.attempt, v.verification), reason.clone(), reason)
                    .await
            }
        }
    }

    /// The request could not be delivered: retry while it can help, then hold the thread.
    async fn verifier_send_failed(&self, v: &Verification, err: AgentError) -> Done {
        let operator = report(&err);
        if err.is_retryable() && v.row.attempts < self.cfg.max_attempts {
            tracing::warn!(id = %v.row.id, attempt = v.row.attempts, error = %operator, class = ?err.class(), "verification request failed; will retry");
            return self
                .retry(&v.row, self.delay(v.row.attempts, &err), operator)
                .await;
        }
        tracing::warn!(id = %v.row.id, error = %operator, class = ?err.class(), "verification request dead-lettered");
        self.verifier_down(
            &v.row,
            (v.attempt, v.verification),
            err.public_detail(),
            operator,
        )
        .await
    }

    /// The verifier cannot be used: the thread waits for the user (no attempt is spent, the code
    /// is not at fault), and the row is dead. `reason` is for the thread's users, `operator` for
    /// the row.
    async fn verifier_down(
        &self,
        row: &OutboxItem,
        (attempt, verification): (u32, u32),
        reason: String,
        operator: String,
    ) -> Done {
        let input = Input::VerifierFailed {
            attempt,
            verification,
            reason,
        };
        self.apply_quiet(row, input, format!("vfail:{}", row.id))
            .await?;
        self.finish(row, OutboxFinal::Dead { error: operator })
            .await
    }
}

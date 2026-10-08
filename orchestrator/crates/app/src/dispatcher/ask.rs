//! The ask path of the dispatcher (ADR 0026): an outbox row of kind `ask` sends the question the
//! job's agent put to one of the agents the person mentioned, and what the asked agent answers
//! goes back into the thread as exactly one input.
//!
//! The core decided everything that is a rule (who may be asked, how many, how deep, when it
//! ends: [`orch_core`]'s `ask` module). This is the half that talks to the asked agent, and it is
//! built like the verifier's ([`verify`](super::verify)), because an ask is the same kind of
//! thing: a delegation to a second agent that runs under a row of its own, in a conversation of
//! its own, beside the thread's delegation.
//!
//! What sets it apart from a delegation:
//!
//! * **The asked agent is not the worker.** Its envelopes are never turned into
//!   [`Input::Agent`](orch_core::Input::Agent): its `completed` must not complete the job, its
//!   `branch` and `checks` artifacts never reach the gate (owner decision 6), and its messages,
//!   steps and screens are not the thread's. They are read here and nowhere else. What comes out
//!   is one [`Input::AskFinished`] (its answer, the question it asks back, or how it failed) or,
//!   when it cannot be had, one [`Input::AskFailed`].
//! * **Its own conversation.** The message goes to the asked agent in a context of its own, never
//!   the thread's: the agent's first ask of a job names none and lets the agent start one, which
//!   is recorded on the ledger with the task ([`Input::AskSent`]), and a later ask of that agent
//!   in the job goes on in it (ADR 0055; before it, [`ask_context`] named the context). The task
//!   it creates is recorded on the row
//!   ([`OutboxItem::task_id`]) and on the ledger ([`Input::AskSent`]), never on the thread's
//!   binding. The core says what the message continues: the task of the agent's last ask when
//!   that ended waiting for an answer (`continue_task`), else a new task that refers to the
//!   earlier ones (ADR 0021). The asked agent is given the thread's tools as `ask:<n>`, at its
//!   depth.
//! * **There is no cancel row.** The row is its own cancel, as a `verify` row is: the ask is wanted
//!   while the ledger says it runs, and it stops being wanted when the core ends it (the deadline,
//!   the person's stop, the end of the task that asked, the end of the ask above it). The row looks
//!   for that every [`DispatcherConfig::verify_watch`] while it waits for the asked agent, and
//!   then cancels the asked agent's task and ends. A crash that left a task nobody follows is
//!   found by the message id and cancelled the same way.
//! * **Every write is fenced** with the row's claim: the task recorded, the answer, the failure
//!   and the end of the row. The answer and the end of the row are **one commit**
//!   ([`App::apply_finishing`](crate::App::apply_finishing)), so a crash cannot leave a claimable
//!   row behind an ask that ended `input_required`: the next claimant would find the ask over
//!   and cancel the very task the next ask is to continue.
//! * **A crash does not ask twice, as far as the asked agent lets it be known.** A row that was
//!   sent has its task on it: the next claimant re-attaches to that task (`resubscribe`, then
//!   `get_task`). One that crashed between sending and recording looks the message up by its id
//!   (`find_task_by_message`, retried when it fails) and sends it again only when the lookup
//!   *says* there is no such task. The message id is the row's, so a send retried after a lost
//!   claim is the same message.
//! * **The deadline is the core's.** An ask that is waited for long enough ends `timed_out` in
//!   the core (a timer in the inbox), and this row finds it over. Nothing here keeps a clock of
//!   its own, so a send that keeps failing for a reason that is retried (the registry is down)
//!   is bounded by that deadline, and one that fails for any other is bounded by
//!   [`DispatcherConfig::max_attempts`].
//!
//! The asked agent's **progress** is not reported: the log has `ask_started` and `ask_finished`
//! and nothing between (the asked agent's steps are for the change that draws asks).
//!
//! [`DispatcherConfig::verify_watch`]: super::DispatcherConfig::verify_watch
//! [`DispatcherConfig::max_attempts`]: super::DispatcherConfig::max_attempts

use std::time::Duration;

use futures::StreamExt;
use orch_core::{
    AgentId, AgentTaskState, AgentUpdate, AskArtifact, AskOutcome, AskResult, Caller, Classify,
    Input, MAX_ASK_ANSWER_BYTES, MessagePurpose, ThreadId, ThreadRecord, ToolsGrant,
    TransitionError, ask_context, report,
};
use orch_ports::{
    AgentClient, AgentEndpoint, AgentEnvelope, AgentError, AgentStream, OutboxFinal, OutboxItem,
    OutboxPayload, Ports, SendContent, SendRequest, TaskHandle, ThreadStore,
};
use tokio::time::MissedTickBehavior;

use super::{DispatchError, Dispatcher, Done, env_state};

/// The error text a row carries that an earlier build parked (it could not send asks yet): its
/// claims were not sends, so the first claim of this build does not count them.
const PARKED: &str = "asks are not sent yet";

/// The context an ask's message names (ADR 0055): the one the row carries (the agent assigned it to
/// its earlier asks of the job), else none for the agent's first ask of the job. A row that goes on
/// in a task, or refers to earlier ones, and names no context was written before the agent assigned
/// them, and its earlier tasks live in [`ask_context`], so that is where it goes on.
fn send_context(
    thread: ThreadId,
    agent: &AgentId,
    context: Option<String>,
    continue_task: &Option<String>,
    reference_task_ids: &[String],
) -> Option<String> {
    context.or_else(|| {
        (continue_task.is_some() || !reference_task_ids.is_empty())
            .then(|| ask_context(thread, agent))
    })
}

/// [`send_context`] of a row's payload (`None` for a row that is no ask).
fn row_context(thread: ThreadId, agent: &AgentId, payload: &OutboxPayload) -> Option<String> {
    match payload {
        OutboxPayload::Ask {
            continue_task,
            reference_task_ids,
            context,
            ..
        } => send_context(
            thread,
            agent,
            context.clone(),
            continue_task,
            reference_task_ids,
        ),
        _ => None,
    }
}

/// One ask, as the row that sends it describes it.
struct Asking {
    row: OutboxItem,
    thread: ThreadId,
    /// The job the ask belongs to.
    job: u32,
    /// The ask's number in the job.
    ask: u32,
    agent: AgentId,
    endpoint: AgentEndpoint,
    /// The A2A context the message names: the one the agent assigned to its earlier asks of the
    /// job, `None` for its first (ADR 0055).
    context: Option<String>,
}

/// What the asked agent has said so far. Read, never applied to the thread.
#[derive(Default)]
struct Answer {
    /// The words of the status that ended the turn: the answer of a completed task, the question
    /// of one that waits, the reason of one that failed.
    ending: Option<String>,
    /// The latest message the agent stated as its answer.
    stated: Option<String>,
    /// The latest message that says no more than that it is a message.
    said: Option<String>,
    /// What it handed back, by name; the latest of a name wins.
    artifacts: Vec<(AskArtifact, Option<String>)>,
    /// The revision that served the task.
    revision: Option<String>,
}

impl Answer {
    fn note(&mut self, env: &AgentEnvelope) {
        if env.revision.is_some() {
            self.revision.clone_from(&env.revision);
        }
        match &env.update {
            Some(AgentUpdate::Status { state, detail }) if state.ends_turn() => {
                self.ending = detail.clone().filter(|d| !d.trim().is_empty());
            }
            Some(AgentUpdate::Message {
                text,
                is_final: true,
                purpose,
                ..
            }) if !text.trim().is_empty() => match purpose {
                Some(MessagePurpose::Answer) => self.stated = Some(text.clone()),
                None => self.said = Some(text.clone()),
                // words said while it works are not its answer
                Some(MessagePurpose::Working) => {}
            },
            Some(AgentUpdate::Artifact {
                name,
                mime_type,
                uri,
                text,
            }) => {
                self.artifact(
                    AskArtifact {
                        name: name.clone(),
                        uri: uri.clone(),
                        mime_type: mime_type.clone(),
                    },
                    text.clone(),
                );
            }
            // a file is named, never kept: its bytes are not the thread's
            Some(AgentUpdate::File {
                name,
                media_type,
                filename: _,
                bytes: _,
            }) => self.artifact(
                AskArtifact {
                    name: name.clone(),
                    uri: None,
                    mime_type: media_type.clone(),
                },
                None,
            ),
            // not for an asked agent: it cannot show an interface or have steps of its own here
            Some(
                AgentUpdate::Status { .. }
                | AgentUpdate::Message { .. }
                | AgentUpdate::Reasoning { .. }
                | AgentUpdate::FileKept { .. }
                | AgentUpdate::FileRefused { .. }
                | AgentUpdate::Ui { .. }
                | AgentUpdate::UiRejected { .. }
                | AgentUpdate::Step(_)
                // its tokens are applied to the thread on their own (`Dispatcher::ask_usage`)
                | AgentUpdate::Usage(_)
                | AgentUpdate::UsageRejected(_),
            )
            | None => {}
        }
    }

    fn artifact(&mut self, artifact: AskArtifact, text: Option<String>) {
        let text = text.map(|t| cut(&t));
        match self
            .artifacts
            .iter_mut()
            .find(|(a, _)| a.name == artifact.name)
        {
            Some(held) => *held = (artifact, text),
            None => self.artifacts.push((artifact, text)),
        }
    }

    /// What the agent answered: the words it stated as its answer, else those of the status that
    /// ended its turn, else its last message, else the text of what it handed back.
    fn text(&self) -> Option<String> {
        self.stated
            .clone()
            .or_else(|| self.ending.clone())
            .or_else(|| self.said.clone())
            .or_else(|| {
                let texts: Vec<&str> = self
                    .artifacts
                    .iter()
                    .filter_map(|(_, t)| t.as_deref())
                    .filter(|t| !t.trim().is_empty())
                    .collect();
                (!texts.is_empty()).then(|| cut(&texts.join("\n\n")))
            })
    }

    /// What the agent asks back: the words of the status that ended its turn, else its last message.
    fn question(&self) -> Option<String> {
        self.ending
            .clone()
            .or_else(|| self.stated.clone())
            .or_else(|| self.said.clone())
    }

    fn result(&self, state: AgentTaskState) -> AskResult {
        let artifacts = self.artifacts.iter().map(|(a, _)| a.clone()).collect();
        match state {
            AgentTaskState::Completed => AskResult {
                text: self.text(),
                artifacts,
                ..AskResult::of(AskOutcome::Completed)
            },
            AgentTaskState::InputRequired => AskResult {
                question: self.question(),
                artifacts,
                ..AskResult::of(AskOutcome::InputRequired)
            },
            AgentTaskState::AuthRequired => AskResult {
                question: self.question(),
                artifacts,
                ..AskResult::of(AskOutcome::AuthRequired)
            },
            AgentTaskState::Failed => AskResult {
                error: Some(
                    self.ending
                        .clone()
                        .unwrap_or_else(|| "the asked agent's task failed".to_owned()),
                ),
                ..AskResult::of(AskOutcome::Failed)
            },
            AgentTaskState::Rejected => AskResult {
                error: Some(
                    self.ending
                        .clone()
                        .unwrap_or_else(|| "the asked agent refused the request".to_owned()),
                ),
                ..AskResult::of(AskOutcome::Rejected)
            },
            AgentTaskState::Canceled => AskResult {
                error: Some(
                    self.ending
                        .clone()
                        .unwrap_or_else(|| "the asked agent cancelled its task".to_owned()),
                ),
                ..AskResult::of(AskOutcome::Canceled)
            },
            // the callers pass a state that ended the turn
            AgentTaskState::Submitted | AgentTaskState::Working => AskResult {
                error: Some("the asked agent's task did not end".to_owned()),
                ..AskResult::of(AskOutcome::Failed)
            },
        }
    }
}

/// At most what the core keeps of an answer: a longer text is never held in memory beyond it.
fn cut(text: &str) -> String {
    let mut end = text.len().min(MAX_ASK_ANSWER_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// How consuming the asked agent's stream ended.
enum Flow {
    /// The task ended its turn.
    Ended(AgentTaskState),
    /// The stream broke after the message was recorded as sent.
    Disconnected,
    /// The claim is gone; stop touching anything.
    Lost,
    /// The stream failed before the message was recorded as sent.
    Failed(AgentError),
    /// The ask is over while it was being waited for; the task, if any, is recorded.
    Unwanted,
}

/// Whether ask `ask` of job `job` is still running, as the thread's ledger says.
fn wanted(thread: &ThreadRecord, job: u32, ask: u32) -> bool {
    thread.job.number == job && thread.job.asks.iter().any(|a| a.n == ask && a.is_running())
}

/// How many times a send was tried: the claims of the row, but for the ones an earlier build made
/// only to put it back.
fn send_attempts(row: &OutboxItem) -> u32 {
    if row.last_error.as_deref() == Some(PARKED) {
        1
    } else {
        row.attempts
    }
}

impl<P: Ports> Dispatcher<P> {
    /// Processes one `ask` row. See the module documentation.
    pub(super) async fn ask(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Ask {
            job,
            ask,
            agent,
            depth,
            text,
            continue_task,
            reference_task_ids,
            context,
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
        if !wanted(&thread, job, ask) {
            return self.drop_ask(&row, &agent).await;
        }
        // Read now, for every claim, like a delegation's agent: an agent the platform removed
        // fails the ask, and a registry that cannot answer leaves the row to be tried again.
        let endpoint = match self.app.resolve_agent(&agent).await {
            Ok(Some(entry)) => entry.endpoint,
            Ok(None) => {
                let reason = format!("agent '{agent}' is no longer listed");
                return self
                    .ask_failed(&row, job, ask, reason, "agent not listed".to_owned())
                    .await;
            }
            Err(crate::AppError::RegistryUnavailable { source }) => {
                tracing::warn!(id = %row.id, %agent, attempt = row.attempts, error = %report(&source), "the agent registry cannot say where the asked agent is; will retry");
                return self
                    .retry(
                        &row,
                        self.backoff(row.attempts),
                        format!("the agent registry is unreachable: {}", report(&source)),
                    )
                    .await;
            }
            Err(e) => return Err(e.into()),
        };
        let a = Asking {
            context: send_context(
                row.thread_id,
                &agent,
                context,
                &continue_task,
                &reference_task_ids,
            ),
            thread: row.thread_id,
            job,
            ask,
            agent,
            endpoint,
            row,
        };
        let grant = ToolsGrant {
            thread: a.thread,
            job,
            agent: a.agent.clone(),
            caller: Caller::Ask(ask),
            depth,
            attached: self.app.attached_for(&a.agent, &thread.job.tools),
        };
        self.send_ask(&a, text, continue_task, reference_task_ids, grant)
            .await
    }

    /// Whether the ask is still the one the thread's ledger says runs. A thread that cannot be read
    /// now is taken to be waiting: the look comes again.
    async fn ask_wanted(&self, a: &Asking) -> bool {
        match self.store().get_thread(None, a.thread).await {
            Ok(Some(thread)) => wanted(&thread, a.job, a.ask),
            Ok(None) => false,
            Err(e) => {
                tracing::warn!(id = %a.row.id, error = %report(&e), "looking at the thread failed");
                true
            }
        }
    }

    /// The ask is over while the asked agent was being waited for.
    async fn ask_unwanted(&self, a: &Asking) -> Done {
        tracing::debug!(id = %a.row.id, ask = a.ask, "the ask is over; dropping the row");
        self.drop_ask(&a.row, &a.agent).await
    }

    /// The row is obsolete: cancel the asked agent's task, if there is one, and finish the row.
    ///
    /// The task is the one on the row. A row that crashed between sending and recording has none, and
    /// a later claim (`attempts` above 1) looks the message up by its id, so that the task nobody
    /// follows is cancelled too. A cancel that cannot be made now is tried again, a bounded number
    /// of times: what is left running is a waste, not a risk, because nothing it says is applied.
    async fn drop_ask(&self, row: &OutboxItem, agent: &AgentId) -> Done {
        // The task may have been recorded after this claim was handed out.
        let recorded = match self.store().get_outbox(row.id).await? {
            Some(current) => current.task_id,
            None => row.task_id.clone(),
        };
        if recorded.is_none() && row.attempts <= 1 {
            // never sent, as far as this row knows
            return self.finish(row, OutboxFinal::Skipped).await;
        }
        let endpoint = match self.app.resolve_agent(agent).await {
            Ok(Some(entry)) => entry.endpoint,
            Ok(None) => return self.finish(row, OutboxFinal::Skipped).await,
            Err(e) => {
                tracing::debug!(id = %row.id, error = %report(&e), "the asked agent cannot be found to cancel its task");
                return self.cancel_later(row, report(&e)).await;
            }
        };
        let task_id = match recorded {
            Some(task_id) => task_id,
            None => match self
                .find_by_message(
                    &endpoint,
                    row_context(row.thread_id, agent, &row.payload).as_deref(),
                    row,
                )
                .await
            {
                Ok(Some(task_id)) => task_id,
                Ok(None) => return self.finish(row, OutboxFinal::Skipped).await,
                Err(e) => return self.cancel_later(row, report(&e)).await,
            },
        };
        let handle = TaskHandle { endpoint, task_id };
        match self.app.ports().agents().cancel(&handle).await {
            Ok(_) | Err(AgentError::NotCancelable(_) | AgentError::TaskNotFound(_)) => {}
            Err(e) if e.is_retryable() => return self.cancel_later(row, report(&e)).await,
            Err(e) => {
                tracing::debug!(id = %row.id, error = %report(&e), "the asked agent did not cancel");
            }
        }
        self.finish(row, OutboxFinal::Skipped).await
    }

    /// A cancel that could not be made: the row comes back after a while, until the budget of
    /// cancels is spent.
    async fn cancel_later(&self, row: &OutboxItem, error: String) -> Done {
        if row.attempts < self.cfg.max_cancel_attempts {
            return self.retry(row, self.backoff(row.attempts), error).await;
        }
        tracing::warn!(id = %row.id, %error, "the asked agent's task could not be cancelled; giving up");
        self.finish(row, OutboxFinal::Skipped).await
    }

    /// Sends the message, or re-attaches to the task an earlier claim started.
    async fn send_ask(
        &self,
        a: &Asking,
        text: String,
        continue_task: Option<String>,
        reference_task_ids: Vec<String>,
        grant: ToolsGrant,
    ) -> Done {
        let mut answer = Answer::default();
        if a.row.sent_at.is_some() {
            let Some(task_id) = a.row.task_id.clone() else {
                return Err(orch_ports::StoreError::corrupt("sent ask without a task id").into());
            };
            return self.follow_ask(a, task_id, &mut answer).await;
        }
        // A previous claim may have reached the asked agent before it crashed or lost the claim.
        // Only "there is no such task" lets the message go out again: a lookup that failed does
        // not know, and asking twice is worse than waiting.
        if a.row.attempts > 1 {
            match self
                .find_by_message(&a.endpoint, a.context.as_deref(), &a.row)
                .await
            {
                Ok(Some(task_id)) => {
                    if !self
                        .store()
                        .mark_verify_sent(&self.lease(&a.row), task_id.clone(), self.now())
                        .await?
                    {
                        return Ok(());
                    }
                    return self.follow_ask(a, task_id, &mut answer).await;
                }
                Ok(None) => {}
                Err(e) => return self.ask_send_failed(a, e).await,
            }
        }
        let continues = continue_task.is_some();
        let request = SendRequest {
            endpoint: a.endpoint.clone(),
            message_id: a.row.id.to_string(),
            context_id: a.context.clone(),
            task_id: continue_task,
            reference_task_ids,
            content: SendContent::Text(text),
            release: None,
            // the asked agent is told nothing of the person's screen: it cannot show one
            ui_catalog: None,
            thread_tools: Some(grant),
            // nor of the conversation a fork of this thread continues
            history: None,
            steer: false,
            // it was not addressed by the person, so it is told of no mentions
            mentions: Vec::new(),
        };
        self.app
            .record_agent_build(a.row.thread_id, &a.endpoint)
            .await;
        match self.app.ports().agents().send_stream(request).await {
            Ok(stream) => match self
                .ask_stream(a, stream, true, continues, &mut answer)
                .await?
            {
                Flow::Ended(state) => self.ask_answered(a, &answer, state).await,
                Flow::Lost => Ok(()),
                Flow::Unwanted => self.ask_unwanted(a).await,
                Flow::Disconnected => {
                    let task = self
                        .store()
                        .get_outbox(a.row.id)
                        .await?
                        .and_then(|r| r.task_id)
                        .ok_or_else(|| {
                            DispatchError::from(crate::AppError::internal(
                                "task id missing after send",
                            ))
                        })?;
                    self.follow_ask(a, task, &mut answer).await
                }
                Flow::Failed(e) => self.ask_send_failed(a, e).await,
            },
            Err(e) => self.ask_send_failed(a, e).await,
        }
    }

    /// Reads the asked agent's stream until its task ends its turn. With `mark_first`, the first
    /// envelope records the message as sent, and the task on the ledger, before anything else
    /// happens.
    ///
    /// `guard_stale`: when continuing a task that waited for an answer, an `input-required`
    /// envelope seen before the task moved on is a stale snapshot, not the agent's new question.
    async fn ask_stream(
        &self,
        a: &Asking,
        mut stream: AgentStream,
        mark_first: bool,
        guard_stale: bool,
        answer: &mut Answer,
    ) -> Result<Flow, DispatchError> {
        let mut marked = !mark_first;
        let mut seen_working = !guard_stale;
        // An ask that names a context goes on in one the ledger holds already (or in the legacy
        // one, `send_context`): there is nothing to learn.
        let mut context_recorded = a.context.is_some();
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
                    if self.ask_wanted(a).await {
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
                            .mark_verify_sent(&self.lease(&a.row), env.task_id.clone(), self.now())
                            .await?
                        {
                            return Ok(Flow::Lost);
                        }
                        marked = true;
                        self.record_task(a, &env.task_id, Some(&env.context_id))
                            .await?;
                        context_recorded = !env.context_id.is_empty();
                    } else if !context_recorded && !env.context_id.is_empty() {
                        // the first envelope named no context, or this is a task found again after
                        // a crash: the ledger learns the agent's context when it is first seen
                        self.record_task(a, &env.task_id, Some(&env.context_id))
                            .await?;
                        context_recorded = true;
                    }
                    // pieces of a reply being written are the asked agent's own business
                    if env.live.is_some() {
                        continue;
                    }
                    let state = env_state(&env);
                    if state.is_some_and(|s| !s.is_interrupted()) {
                        seen_working = true;
                    }
                    if !seen_working && state.is_some_and(AgentTaskState::is_interrupted) {
                        continue;
                    }
                    answer.note(&env);
                    if let Some(state) = state.filter(|s| s.ends_turn()) {
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

    /// Tells the core the asked agent has a task, so that the next ask of that agent can continue
    /// it or refer to it. Applied again by a claimant that re-attaches (the key makes it one
    /// write), because a crash may have come between recording the task on the row and here.
    ///
    /// The context the agent put the task in (ADR 0055) goes with it, under a key of its own: the
    /// task may have been recorded without one (a claimant that re-attached did not know it yet),
    /// and learning the context later must not be dropped as the same write.
    async fn record_task(&self, a: &Asking, task_id: &str, context: Option<&str>) -> Done {
        let context = context.filter(|c| !c.is_empty());
        let key = match context {
            Some(_) => format!("asksent:{}:context", a.row.id),
            None => format!("asksent:{}", a.row.id),
        };
        self.apply_quiet(
            &a.row,
            Input::AskSent {
                job: a.job,
                ask: a.ask,
                task_id: task_id.to_owned(),
                context_id: context.map(str::to_owned),
            },
            key,
        )
        .await
    }

    /// Re-attaches to a task that was sent: resubscribe, else poll `get_task`.
    async fn follow_ask(&self, a: &Asking, task_id: String, answer: &mut Answer) -> Done {
        self.record_task(a, &task_id, None).await?;
        let handle = TaskHandle {
            endpoint: a.endpoint.clone(),
            task_id,
        };
        match self.app.ports().agents().resubscribe(&handle).await {
            Ok(stream) => match self.ask_stream(a, stream, false, false, answer).await? {
                // The stream a resubscription gives is the rest of the task, not the start: what
                // was said before this claim, or before the stream broke, is not in it. The task
                // holds it; read it before saying the agent answered nothing.
                Flow::Ended(AgentTaskState::Completed) if answer.text().is_none() => {
                    tracing::debug!(id = %a.row.id, "completed with nothing on the rest of the stream; reading the task");
                }
                Flow::Ended(state) => return self.ask_answered(a, answer, state).await,
                Flow::Lost => return Ok(()),
                Flow::Unwanted => return self.ask_unwanted(a).await,
                Flow::Disconnected | Flow::Failed(_) => {}
            },
            Err(e) => {
                tracing::debug!(id = %a.row.id, error = %report(&e), "resubscribe unavailable; polling");
            }
        }
        self.poll_ask(a, &handle, answer).await
    }

    /// Polls `get_task` with backoff until the asked agent's task ends its turn.
    async fn poll_ask(&self, a: &Asking, handle: &TaskHandle, answer: &mut Answer) -> Done {
        let mut delay = self.cfg.poll_min;
        let mut failures = 0_u32;
        let mut learned = false;
        loop {
            if !self.ask_wanted(a).await {
                return self.ask_unwanted(a).await;
            }
            match self.app.ports().agents().get_task(handle).await {
                Ok(snap) => {
                    failures = 0;
                    if !learned && a.context.is_none() && !snap.context_id.is_empty() {
                        self.record_task(a, &snap.task_id, Some(&snap.context_id))
                            .await?;
                        learned = true;
                    }
                    for env in &snap.envelopes {
                        answer.note(env);
                    }
                    if snap.revision.is_some() {
                        answer.revision.clone_from(&snap.revision);
                    }
                    if snap.state.ends_turn() {
                        return self.ask_answered(a, answer, snap.state).await;
                    }
                }
                Err(AgentError::TaskNotFound(m)) => {
                    let reason = format!("the asked agent lost the task: {m}");
                    return self
                        .ask_failed(&a.row, a.job, a.ask, reason.clone(), reason)
                        .await;
                }
                Err(e) if e.is_retryable() => {
                    failures += 1;
                    tracing::warn!(id = %a.row.id, failures, error = %report(&e), "polling the asked agent failed");
                    if failures >= self.cfg.max_poll_failures {
                        return self
                            .ask_failed(&a.row, a.job, a.ask, e.public_detail(), report(&e))
                            .await;
                    }
                }
                Err(e) => {
                    return self
                        .ask_failed(&a.row, a.job, a.ask, e.public_detail(), report(&e))
                        .await;
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(self.cfg.poll_max);
        }
    }

    /// The asked agent's task ended its turn in `state`: the ask ends with what it said, and the
    /// row with it, in one commit.
    async fn ask_answered(&self, a: &Asking, answer: &Answer, state: AgentTaskState) -> Done {
        // The input names the job, so a result for an earlier job's ask changes nothing; the look
        // right before the write spares the commit, and the cancel of a task nobody waits for.
        if !self.ask_wanted(a).await {
            return self.ask_unwanted(a).await;
        }
        let input = Input::AskFinished {
            job: a.job,
            ask: a.ask,
            revision: answer.revision.clone(),
            result: answer.result(state),
        };
        self.end_ask(
            &a.row,
            input,
            format!("askfin:{}", a.row.id),
            OutboxFinal::Delivered,
        )
        .await
    }

    /// The message could not be delivered: retry while it can help, then fail the ask.
    async fn ask_send_failed(&self, a: &Asking, err: AgentError) -> Done {
        let operator = report(&err);
        if err.is_retryable() && send_attempts(&a.row) < self.cfg.max_attempts {
            tracing::warn!(id = %a.row.id, attempt = a.row.attempts, error = %operator, class = ?err.class(), "ask failed; will retry");
            return self
                .retry(&a.row, self.delay(a.row.attempts, &err), operator)
                .await;
        }
        tracing::warn!(id = %a.row.id, error = %operator, class = ?err.class(), "ask dead-lettered");
        self.ask_failed(&a.row, a.job, a.ask, err.public_detail(), operator)
            .await
    }

    /// The asked agent cannot be had: the ask fails (`reason` is for the thread's users,
    /// `operator` for the row), and the row is dead, in one commit.
    async fn ask_failed(
        &self,
        row: &OutboxItem,
        job: u32,
        ask: u32,
        reason: String,
        operator: String,
    ) -> Done {
        self.end_ask(
            row,
            Input::AskFailed { job, ask, reason },
            format!("askfail:{}", row.id),
            OutboxFinal::Dead { error: operator },
        )
        .await
    }

    /// Applies the end of the ask and finishes the row in one commit.
    async fn end_ask(
        &self,
        row: &OutboxItem,
        input: Input,
        key: String,
        finish: OutboxFinal,
    ) -> Done {
        use crate::{AppError, ApplyOutcome};
        let lease = self.lease(row);
        match self
            .app
            .apply_finishing(row.thread_id, input, key, &lease, finish.clone())
            .await
        {
            Ok(ApplyOutcome::Fenced) => Err(DispatchError::Fenced),
            Ok(ApplyOutcome::Applied { .. }) => Ok(()),
            // written before, and its row not finished: end it now
            Ok(ApplyOutcome::Duplicate) => self.finish(row, finish).await,
            Err(AppError::Transition(TransitionError::InvalidInState { .. })) => {
                self.finish(row, OutboxFinal::Skipped).await
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_ports::IdemKey;

    use super::*;

    fn env(update: AgentUpdate) -> AgentEnvelope {
        AgentEnvelope {
            task_id: "t".to_owned(),
            context_id: "c".to_owned(),
            task_state: None,
            revision: None,
            key: IdemKey::Turn("k".to_owned()),
            update: Some(update),
            live: None,
        }
    }

    fn message(text: &str, is_final: bool, purpose: Option<MessagePurpose>) -> AgentEnvelope {
        env(AgentUpdate::Message {
            message_id: "m".to_owned(),
            text: text.to_owned(),
            is_final,
            purpose,
        })
    }

    fn ended(state: AgentTaskState, detail: Option<&str>) -> AgentEnvelope {
        env(AgentUpdate::Status {
            state,
            detail: detail.map(str::to_owned),
        })
    }

    fn artifact(name: &str, text: Option<&str>) -> AgentEnvelope {
        env(AgentUpdate::Artifact {
            name: name.to_owned(),
            mime_type: None,
            uri: Some(format!("https://example.com/{name}")),
            text: text.map(str::to_owned),
        })
    }

    fn answer_of(envelopes: &[AgentEnvelope]) -> Answer {
        let mut answer = Answer::default();
        for e in envelopes {
            answer.note(e);
        }
        answer
    }

    #[test]
    fn the_answer_is_what_the_agent_stated_then_the_words_that_ended_its_turn_then_its_last_message_then_what_it_handed_back()
     {
        let say = message("said", true, None);
        let stated = message("stated", true, Some(MessagePurpose::Answer));
        let done = ended(AgentTaskState::Completed, Some("ending"));
        let file = artifact("report", Some("in the file"));
        let all = [file.clone(), say.clone(), done.clone(), stated.clone()];
        assert_eq!(answer_of(&all).text().as_deref(), Some("stated"));
        let all = [file.clone(), say.clone(), done];
        assert_eq!(answer_of(&all).text().as_deref(), Some("ending"));
        let all = [file.clone(), say, ended(AgentTaskState::Completed, None)];
        assert_eq!(answer_of(&all).text().as_deref(), Some("said"));
        let all = [file, ended(AgentTaskState::Completed, Some("  "))];
        assert_eq!(answer_of(&all).text().as_deref(), Some("in the file"));
        assert_eq!(answer_of(&[]).text(), None);
    }

    #[test]
    fn words_said_while_working_and_pieces_not_yet_whole_are_not_the_answer() {
        let all = [
            message("let me look", true, Some(MessagePurpose::Working)),
            message("half a sen", false, None),
            ended(AgentTaskState::Working, Some("Reading the repository")),
            ended(AgentTaskState::Completed, None),
        ];
        assert_eq!(answer_of(&all).text(), None);
    }

    #[test]
    fn the_status_that_ends_a_turn_is_the_question_or_the_reason() {
        let all = [
            ended(AgentTaskState::Working, Some("thinking")),
            ended(AgentTaskState::InputRequired, Some("Which branch?")),
        ];
        let answer = answer_of(&all);
        let result = answer.result(AgentTaskState::InputRequired);
        assert_eq!(result.outcome, AskOutcome::InputRequired);
        assert_eq!(result.question.as_deref(), Some("Which branch?"));
        assert_eq!(result.text, None);

        let failed = answer_of(&[ended(AgentTaskState::Failed, Some("it broke"))])
            .result(AgentTaskState::Failed);
        assert_eq!(failed.outcome, AskOutcome::Failed);
        assert_eq!(failed.error.as_deref(), Some("it broke"));
        let silent = Answer::default();
        assert_eq!(
            silent.result(AgentTaskState::Failed).error.as_deref(),
            Some("the asked agent's task failed")
        );
        assert_eq!(
            silent.result(AgentTaskState::Rejected).outcome,
            AskOutcome::Rejected
        );
        assert_eq!(
            silent.result(AgentTaskState::Canceled).outcome,
            AskOutcome::Canceled
        );
        assert_eq!(
            silent.result(AgentTaskState::AuthRequired).outcome,
            AskOutcome::AuthRequired
        );
    }

    #[test]
    fn what_was_handed_back_is_listed_once_by_name_and_its_text_is_cut() {
        let big = "x".repeat(MAX_ASK_ANSWER_BYTES + 100);
        let answer = answer_of(&[
            artifact("a", Some("first")),
            artifact("b", None),
            artifact("a", Some(&big)),
        ]);
        let result = answer.result(AgentTaskState::Completed);
        assert_eq!(
            result
                .artifacts
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(result.text.unwrap().len(), MAX_ASK_ANSWER_BYTES);
        // a cut never splits a character
        assert_eq!(cut(&"é".repeat(10)[..]).len(), 20);
        let two_bytes = "é".repeat(MAX_ASK_ANSWER_BYTES);
        assert!(cut(&two_bytes).len() <= MAX_ASK_ANSWER_BYTES);
    }

    #[test]
    fn a_file_is_named_and_its_bytes_are_never_held() {
        let answer = answer_of(&[env(AgentUpdate::File {
            name: "chart".to_owned(),
            media_type: Some("image/png".to_owned()),
            filename: Some("chart.png".to_owned()),
            bytes: vec![1, 2, 3],
        })]);
        let result = answer.result(AgentTaskState::Completed);
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.artifacts[0].name, "chart");
        assert_eq!(result.artifacts[0].mime_type.as_deref(), Some("image/png"));
        assert_eq!(result.text, None);
    }

    #[test]
    fn a_row_an_earlier_build_parked_has_made_no_send() {
        let row = |last_error: Option<&str>, attempts| OutboxItem {
            id: orch_ports::OutboxId(uuid::Uuid::nil()),
            thread_id: ThreadId(uuid::Uuid::nil()),
            kind: orch_ports::OutboxKind::Ask,
            payload: OutboxPayload::Cancel { job: None },
            status: orch_ports::OutboxStatus::Pending,
            attempts,
            sent_at: None,
            task_id: None,
            next_attempt_at: "2026-01-01T00:00:00Z".parse().unwrap(),
            lease_owner: None,
            lease_until: None,
            last_error: last_error.map(str::to_owned),
            created_at: "2026-01-01T00:00:00Z".parse().unwrap(),
        };
        assert_eq!(send_attempts(&row(Some(PARKED), 9)), 1);
        assert_eq!(send_attempts(&row(Some("unreachable"), 9)), 9);
        assert_eq!(send_attempts(&row(None, 1)), 1);
    }
}

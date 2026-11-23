//! A person's start and close of a helper session, carried out (`child-sessions/v1` sections 4 and
//! 6, ADR 0072, S3). Both rows ride the `steer` lane and are never delegations: a start is a message
//! to the **parent's** context naming the helper, a close is a message to the **child's** context.
//!
//! Neither moves the thread's own binding: the task a start or a close makes is a round of the
//! session, followed (when it is one a thread follows) by the child thread's own rows. What the
//! agent says comes back as the round's own word about its session ([`AgentUpdate::ChildSessions`]).
//! A refusal arrives with the agent's own words, and nothing is retried for it.

use std::time::Duration;

use futures::StreamExt;
use orch_core::{
    AgentUpdate, Classify, Input, PersonPrompt, SessionBy, SessionNews, SessionReport,
    SessionStartOutcome,
};
use orch_ports::{
    AgentClient, AgentError, OutboxFinal, OutboxItem, OutboxPayload, SendContent, SendRequest,
    TaskHandle, ThreadStore,
};

use super::{DispatchError, Dispatcher, Done, Loaded, plain_cap};
use crate::{AppError, ApplyOutcome, ChildSessionDrop};
use orch_core::report;
use orch_ports::Ports;

/// How long the agent has to answer a start or a close with the round it made: its first event is
/// the task (`child-sessions/v1`), so this is the time to make a task, not to run one.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// The most envelopes read from the answer to a start or a close while looking for the round's
/// own word about its session.
const ANSWER_ENVELOPES: usize = 16;

/// The round an agent made for a start or a close, read from its answer.
struct Answer {
    /// The task the round is.
    task_id: String,
    /// The context the answer was in.
    context_id: String,
    /// What the round says of its session, when it did.
    report: Option<SessionReport>,
}

impl<P: Ports> Dispatcher<P> {
    /// Reads the answer to a start or a close: the first envelopes of the task it made, up to the
    /// round's own word about its session. A stream that says no session is read once more as a
    /// task (`GetTask`): the contract puts the session on the task as well.
    async fn read_answer(&self, req: SendRequest) -> Result<Answer, AgentError> {
        let endpoint = req.endpoint.clone();
        let agents = self.app.ports().agents();
        let mut stream = agents.send_stream(req).await?;
        let mut first: Option<(String, String)> = None;
        let mut report: Option<SessionReport> = None;
        let mut read = 0;
        while let Some(next) = stream.next().await {
            let env = next?;
            first.get_or_insert_with(|| (env.task_id.clone(), env.context_id.clone()));
            if let Some(AgentUpdate::ChildSessions(SessionNews::OfRound(r))) = &env.update {
                report = Some(r.clone());
                break;
            }
            read += 1;
            if read >= ANSWER_ENVELOPES {
                break;
            }
        }
        drop(stream);
        let (task_id, context_id) =
            first.ok_or_else(|| AgentError::protocol("the agent answered with nothing"))?;
        if report.is_none() {
            let handle = TaskHandle {
                endpoint,
                task_id: task_id.clone(),
            };
            if let Ok(snapshot) = agents.get_task(&handle).await {
                report = snapshot.envelopes.iter().find_map(|e| match &e.update {
                    Some(AgentUpdate::ChildSessions(SessionNews::OfRound(r))) => Some(r.clone()),
                    _ => None,
                });
            }
        }
        Ok(Answer {
            task_id,
            context_id,
            report,
        })
    }

    /// The agent refused, or could not be reached: a failure that can pass is tried again, the
    /// rest ends the start with the agent's words (or ours) and the row dead.
    async fn start_failed(&self, row: &OutboxItem, request: &str, err: AgentError) -> Done {
        if err.is_retryable() && row.attempts < self.cfg.max_attempts {
            tracing::warn!(id = %row.id, attempt = row.attempts, error = %report(&err), "starting a session failed; will retry");
            return self
                .retry(row, self.delay(row.attempts, &err), report(&err))
                .await;
        }
        tracing::info!(id = %row.id, error = %report(&err), class = ?err.class(), "starting a session ended");
        self.end_start_failed(row, request, err.public_detail(), report(&err))
            .await
    }

    /// The start ends `failed` with `reason` (for the person), and the row dead with `operator`.
    pub(super) async fn end_start_failed(
        &self,
        row: &OutboxItem,
        request: &str,
        reason: String,
        operator: String,
    ) -> Done {
        let lease = self.lease(row);
        if !self
            .app
            .end_start(
                row.thread_id,
                Some(&lease),
                request,
                SessionStartOutcome::Failed { reason },
                format!("startend:{}", row.id),
            )
            .await?
        {
            return Err(DispatchError::Fenced);
        }
        self.finish(row, OutboxFinal::Dead { error: operator })
            .await
    }

    /// Starts the session a person asked for (`Command::StartSession`): the text goes to the
    /// parent's context with the helper named, no task, and the answer is round 1 of a context of
    /// its own, which the child thread follows. The thread's binding is not touched.
    ///
    /// A resend after a crash is safe: the agent derives the session from the message id, so it
    /// finds the one it made.
    pub(super) async fn start_session(&self, row: OutboxItem) -> Done {
        let OutboxPayload::SessionStart {
            request,
            agent,
            description,
            text,
            keep_open,
            tier,
            tier_cap,
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
        let Some(Loaded { ctx, binding, .. }) = self.load(&row).await? else {
            return Ok(());
        };
        let Some(parent_context) = binding.context_id.clone().filter(|c| !c.is_empty()) else {
            let why = "the agent has no context for this thread".to_owned();
            return self
                .end_start_failed(&row, &request, why.clone(), why)
                .await;
        };
        let Some(parent) = self.store().get_thread(None, row.thread_id).await? else {
            return self.finish(&row, OutboxFinal::Skipped).await;
        };
        // The tier it goes under is decided by the live card now, as for a delegation.
        let sent_tier = match self.app.tier_for_send(&ctx.endpoint, tier, tier_cap).await {
            Ok(sent_tier) => sent_tier,
            Err(e) => return self.start_failed(&row, &request, e).await,
        };
        let req = SendRequest {
            endpoint: ctx.endpoint.clone(),
            message_id: row.id.to_string(),
            context_id: Some(parent_context.clone()),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::SessionStart {
                text: text.clone(),
                agent,
                description,
                keep_open,
            },
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
            tier: sent_tier,
            tier_cap: plain_cap(sent_tier, tier_cap),
            locale: None,
            pick: None,
        };
        let answer = match tokio::time::timeout(ANSWER_TIMEOUT, self.read_answer(req)).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(e)) => return self.start_failed(&row, &request, e).await,
            Err(_) => {
                let e = AgentError::unreachable("the agent did not answer the start");
                return self.start_failed(&row, &request, e).await;
            }
        };
        // Fail closed: the answer is round 1 of a session of a context of its own, whose parent
        // is this thread's context, and nothing else is applied.
        let report = answer.report.filter(|r| {
            r.check().is_ok()
                && r.context_id != parent_context
                && r.context_id == answer.context_id
                && r.parent_context_id.as_deref() == Some(parent_context.as_str())
                && r.round.as_ref().is_some_and(|round| {
                    round.number == 1 && round.task_id.as_deref() == Some(answer.task_id.as_str())
                })
        });
        let Some(mut report) = report else {
            self.app
                .child_session_dropped(ChildSessionDrop::ForeignContext, row.thread_id);
            let why = "the agent did not report the session it started".to_owned();
            return self
                .end_start_failed(&row, &request, why.clone(), why)
                .await;
        };
        // a start is the person's, whatever the agent said of who began it
        report.by = Some(SessionBy::Person);
        let prompt = PersonPrompt {
            user: parent.owner.clone(),
            text,
            message_id: Some(request.clone()),
            tier,
        };
        let lease = self.lease(&row);
        if !self
            .app
            .person_session_opened(
                row.thread_id,
                Some(&lease),
                &request,
                report,
                prompt,
                format!("startend:{}", row.id),
            )
            .await?
        {
            return Err(DispatchError::Fenced);
        }
        self.finish(&row, OutboxFinal::Delivered).await
    }

    /// Closes the session this child thread is (`Command::CloseSession`): a message to the child's
    /// context with no task. The answer is the closing round (or the working round that closes
    /// with its end), which this thread follows without showing: it is told as sent, then its
    /// session is read. A refusal ends the ask with the agent's words in the child's log.
    pub(super) async fn close_session(&self, row: OutboxItem) -> Done {
        let OutboxPayload::SessionClose { cancel } = row.payload.clone() else {
            return self
                .finish(
                    &row,
                    OutboxFinal::Dead {
                        error: "payload does not match kind".to_owned(),
                    },
                )
                .await;
        };
        let Some(Loaded { ctx, binding, .. }) = self.load(&row).await? else {
            return Ok(());
        };
        let Some(session_context) = binding.context_id.clone().filter(|c| !c.is_empty()) else {
            return self
                .close_failed(
                    &row,
                    "the helper session has no context".to_owned(),
                    "no context".to_owned(),
                )
                .await;
        };
        let Some(thread) = self.store().get_thread(None, row.thread_id).await? else {
            return self.finish(&row, OutboxFinal::Skipped).await;
        };
        let Some(session) = thread.job.child.as_ref().map(|c| c.session.clone()) else {
            return self
                .finish(
                    &row,
                    OutboxFinal::Dead {
                        error: "the thread is not a helper session".to_owned(),
                    },
                )
                .await;
        };
        let req = SendRequest {
            endpoint: ctx.endpoint.clone(),
            message_id: row.id.to_string(),
            context_id: Some(session_context.clone()),
            task_id: None,
            reference_task_ids: Vec::new(),
            content: SendContent::SessionClose { cancel },
            release: None,
            ui_catalog: None,
            thread_tools: None,
            history: None,
            steer: false,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: None,
            pick: None,
        };
        let answer = match tokio::time::timeout(ANSWER_TIMEOUT, self.read_answer(req)).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(e)) if e.is_retryable() && row.attempts < self.cfg.max_attempts => {
                tracing::warn!(id = %row.id, attempt = row.attempts, error = %report(&e), "closing a session failed; will retry");
                return self
                    .retry(&row, self.delay(row.attempts, &e), report(&e))
                    .await;
            }
            Ok(Err(e)) => return self.close_failed(&row, e.public_detail(), report(&e)).await,
            Err(_) => {
                if row.attempts < self.cfg.max_attempts {
                    return self
                        .retry(
                            &row,
                            self.backoff(row.attempts),
                            "the agent did not answer the close".to_owned(),
                        )
                        .await;
                }
                let why = "the agent could not be reached".to_owned();
                return self.close_failed(&row, why.clone(), why).await;
            }
        };
        // Fail closed: what is applied is this session's, in this context
        let report = answer.report.filter(|r| {
            r.check().is_ok() && r.session == session && r.context_id == session_context
        });
        if answer.context_id != session_context {
            self.app
                .child_session_dropped(ChildSessionDrop::ForeignContext, row.thread_id);
            let why = "the agent answered the close with a task of another context".to_owned();
            return self.close_failed(&row, why.clone(), why).await;
        }
        let lease = self.lease(&row);
        if let Some(report) = report {
            if let Some(round) = report.round.as_ref().and_then(|r| {
                r.task_id
                    .clone()
                    .filter(|t| *t == answer.task_id)
                    .map(|t| (r.number, t))
            }) {
                // the round this thread's own message made is followed, never observed
                let input = Input::RoundSent {
                    session: report.session.clone(),
                    number: round.0,
                    task_id: round.1,
                };
                match self
                    .app
                    .apply(
                        row.thread_id,
                        input,
                        Some(format!("roundsent:{}", row.id)),
                        None,
                        Some(&lease),
                    )
                    .await
                {
                    Ok(ApplyOutcome::Fenced) => return Err(DispatchError::Fenced),
                    Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => {}
                    Err(AppError::NotFound) => {
                        return self.finish(&row, OutboxFinal::Skipped).await;
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            if !self
                .app
                .child_sessions_news(row.thread_id, Some(&lease), SessionNews::OfRound(report))
                .await?
            {
                return Err(DispatchError::Fenced);
            }
        }
        self.finish(&row, OutboxFinal::Delivered).await
    }

    /// The agent refused the close, or it cannot be made: the ask is cleared and the child's log
    /// says why in the agent's words; the row ends dead.
    pub(super) async fn close_failed(
        &self,
        row: &OutboxItem,
        reason: String,
        operator: String,
    ) -> Done {
        let lease = self.lease(row);
        if !self
            .app
            .close_refused(
                row.thread_id,
                Some(&lease),
                reason,
                format!("closefailed:{}", row.id),
            )
            .await?
        {
            return Err(DispatchError::Fenced);
        }
        self.finish(row, OutboxFinal::Dead { error: operator })
            .await
    }
}

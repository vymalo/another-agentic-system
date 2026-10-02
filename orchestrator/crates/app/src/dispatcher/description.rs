//! The description path of the dispatcher (ADR 0035): an outbox row of kind `description` asks the
//! model for a sentence or two on what the thread is about now, and the answer goes back into the
//! thread as exactly one input.
//!
//! Like the title, **nothing depends on the answer**: a description is a nicety, so the row never
//! fails a thread and never holds anything back. Whatever happens, the row ends in the commit of
//! [`Input::Described`] (the model's words, cleaned by [`clean_description`]) or
//! [`Input::DescriptionDeclined`], under the key `description:<row id>`, so one row is one event
//! whoever works it.
//!
//! * **It is asked for the end of a job, and only when the conversation has moved.** The core asks
//!   when a transition leaves the thread `done` or `blocked`, once per job. The worker counts the
//!   messages since the last `thread_described` ([`messages_since_description`]); fewer than the
//!   task's `minNewMessages` is a decline **with no model asked**.
//! * **What the model is shown** is read when the row is worked: the head of the log (for the
//!   first message) and its tail (for the latest ones), joined, so a long thread costs two bounded
//!   reads. The previous description is shown too, so that the model updates it. All of it is
//!   untrusted text and goes fenced, as data ([`task_prompt`]).
//! * **The description is in the language the task's rule says, and checked**, as the title is:
//!   an answer in a script the rule refuses is declined and the model asked **once more in the
//!   same row** ([`check_task_language`]); wrong twice, the row is declined.
//! * **A person's description wins.** A row whose thread has one (or has been cleared by the
//!   person) is dropped, and the core would ignore the model's anyway.
//! * **A model that stumbles is asked again, a little**, and then declined
//!   ([`ask_model`](Dispatcher::ask_model)).
//! * **Every write is fenced** with the row's claim: a worker that lost it writes nothing.

use orch_core::{
    DescriptionSource, Event, Input, TaskKind, TaskPrompt, check_task_language, clean_description,
    messages_since_description, task_prompt,
};
use orch_ports::{ChatRequest, OutboxFinal, OutboxItem, OutboxPayload, Ports, ThreadStore};

use super::{DispatchError, Dispatcher, Done};
use crate::{AppError, ApplyOutcome, TaskSettings};

/// How many events of the head of the log are read, for the first message of the person.
const HEAD_EVENTS: u32 = 16;
/// How many events of the tail of the log are read, for the latest messages and the last
/// `thread_described`.
const TAIL_EVENTS: i64 = 512;

impl<P: Ports> Dispatcher<P> {
    /// Processes one `description` row. See the module documentation.
    pub(super) async fn description(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Description { job } = row.payload.clone() else {
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
        // A person wrote (or cleared) the description while the row waited.
        if thread.job.description.source() == DescriptionSource::User {
            return self.finish(&row, OutboxFinal::Skipped).await;
        }
        let input = match self.app.task(TaskKind::Description) {
            // descriptions were switched off since the row was written
            None => Input::DescriptionDeclined { job },
            Some(task) => {
                let events = self.description_events(&row, thread.last_seq).await?;
                let new = messages_since_description(&events);
                if new < usize::try_from(task.min_new_messages).unwrap_or(usize::MAX) {
                    tracing::debug!(
                        id = %row.id,
                        new,
                        wanted = task.min_new_messages,
                        "too few new messages for a description; no model is asked"
                    );
                    Input::DescriptionDeclined { job }
                } else {
                    match self
                        .description_in_language(&row, task, &events, thread.description.as_deref())
                        .await
                    {
                        Some(description) => Input::Described { job, description },
                        None => Input::DescriptionDeclined { job },
                    }
                }
            }
        };
        let lease = self.lease(&row);
        match self
            .app
            .apply_finishing(
                row.thread_id,
                input,
                format!("description:{}", row.id),
                &lease,
                OutboxFinal::Delivered,
            )
            .await
        {
            Ok(ApplyOutcome::Fenced) => Err(DispatchError::Fenced),
            Ok(ApplyOutcome::Applied { .. }) => Ok(()),
            // written before, and its row not finished: end it now
            Ok(ApplyOutcome::Duplicate) => self.finish(&row, OutboxFinal::Delivered).await,
            Err(AppError::NotFound) => self.finish(&row, OutboxFinal::Skipped).await,
            Err(e) => Err(e.into()),
        }
    }

    /// The part of the log a description is made from: the head, then the tail after it (no event
    /// twice). A short log is read whole.
    async fn description_events(
        &self,
        row: &OutboxItem,
        last_seq: i64,
    ) -> Result<Vec<Event>, DispatchError> {
        let tail_from = last_seq.saturating_sub(TAIL_EVENTS).max(0);
        let mut events = if tail_from > 0 {
            let head = self
                .store()
                .list_events(row.thread_id, 0, HEAD_EVENTS)
                .await?;
            head.into_iter().filter(|e| e.seq <= tail_from).collect()
        } else {
            Vec::new()
        };
        let tail = self
            .store()
            .list_events(
                row.thread_id,
                tail_from,
                u32::try_from(TAIL_EVENTS).unwrap_or(u32::MAX),
            )
            .await?;
        events.extend(tail);
        Ok(events)
    }

    /// The description the model writes for the conversation `events`, in the language the task's
    /// rule asks for, or `None`: nothing to describe yet, it cannot be had, or it was in the wrong
    /// script twice.
    async fn description_in_language(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        events: &[Event],
        previous: Option<&str>,
    ) -> Option<String> {
        let spec = |retry| TaskPrompt {
            kind: TaskKind::Description,
            guidance: task.guidance.as_deref(),
            language: task.language,
            previous,
            retry,
        };
        let description = self
            .description_from(row, task, events, spec(false))
            .await?;
        let Err(wrong) = check_task_language(task.language, events, &description) else {
            return Some(description);
        };
        tracing::debug!(
            id = %row.id,
            script = ?wrong.script,
            "the description is in a script the language rule does not allow; asking once more"
        );
        let description = self.description_from(row, task, events, spec(true)).await?;
        if let Err(wrong) = check_task_language(task.language, events, &description) {
            tracing::debug!(
                id = %row.id,
                script = ?wrong.script,
                "the second description is in the wrong script too; the thread keeps what it has"
            );
            return None;
        }
        Some(description)
    }

    /// One question to the model and the description its answer stands for (`None` for nothing to
    /// describe yet or no answer).
    async fn description_from(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        events: &[Event],
        spec: TaskPrompt<'_>,
    ) -> Option<String> {
        let (system, user) = task_prompt(&spec, events);
        let request = ChatRequest {
            endpoint: task.endpoint.clone(),
            model: task.model.clone(),
            system,
            user,
            max_tokens: task.max_tokens,
        };
        let text = self
            .ask_model(
                row,
                task,
                &request,
                "the model could not describe the thread",
            )
            .await?;
        let description = clean_description(&text, task.max_chars);
        if description.is_none() {
            tracing::debug!(id = %row.id, "the model had no description for the conversation yet");
        }
        description
    }
}

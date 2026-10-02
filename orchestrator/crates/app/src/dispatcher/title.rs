//! The title path of the dispatcher (ADR 0005, MVP slice 6): an outbox row of kind `title` asks the
//! model for a short title of the thread, and the answer goes back into the thread as exactly one
//! input.
//!
//! What sets it apart from a delegation: **nothing depends on the answer.** A title is a
//! nicety, so the row never fails a thread and never holds anything back. Whatever happens, the
//! row ends in the commit of [`Input::Titled`] (the model's words, cleaned by
//! [`clean_title`]) or [`Input::TitleDeclined`] (the conversation has no topic yet, the model is
//! not configured, cannot be reached, or said nothing usable), under the key `title:<row id>`, so
//! one row is one event whoever works it. A decline costs nothing: the thread keeps the first
//! message's words, and the next reply of the agent asks again while the thread has asks left
//! ([`orch_core::MAX_TITLE_ASKS`]).
//!
//! * **The conversation is read when the row is worked**, from the head of the log
//!   ([`TITLE_EVENTS`] events), not copied into the row: the row stays small and a crash loses
//!   nothing. What is read is untrusted text and goes to the model fenced, as data
//!   ([`title_prompt`]); what comes back is cleaned to one line of plain text and checked again by
//!   the core before it is logged.
//! * **The title is in the person's language, and checked.** The prompt names the language last
//!   ([`title_prompt`]); a title in a script none of the person's messages uses (the Chinese title
//!   of an English thread) is declined by [`check_title_language`], and the model is asked **once
//!   more in the same row**, with the language named again and what went wrong
//!   ([`title_retry_prompt`]). If that answer is wrong too the row is declined: the thread keeps
//!   the first message's words, and the next reply may ask again, as after `NONE`.
//! * **A person's title wins.** A row whose thread has been renamed meanwhile is dropped, and the
//!   core would ignore the model's title anyway.
//! * **A model that stumbles is asked again, a little.** A transient failure (a timeout, a 5xx, a
//!   rate limit) is tried up to three times with the dispatcher's backoff, then declined; a refusal
//!   for good is declined at once. Each try is bounded by the task's timeout
//!   ([`TaskSettings::timeout`](crate::TaskSettings::timeout)), whatever the adapter does
//!   ([`ask_model`](Dispatcher::ask_model)).
//! * **The task says how.** The endpoint, the model, the guidance, the tokens and the language rule
//!   are the title task's ([`TaskSettings`](crate::TaskSettings)); the form of the answer, the data
//!   clause, the fence and the language line, last, are the core's and always there
//!   ([`task_prompt`]).
//! * **Every write is fenced** with the row's claim: a worker that lost it writes nothing.

use orch_core::{
    Event, Input, TaskKind, TaskPrompt, TitleSource, check_task_language, clean_title, task_prompt,
};
use orch_ports::{ChatRequest, OutboxFinal, OutboxItem, OutboxPayload, Ports, ThreadStore};

use super::{DispatchError, Dispatcher, Done};
use crate::{AppError, ApplyOutcome, TaskSettings};

/// How many events of the head of the log the conversation is read from.
const TITLE_EVENTS: u32 = 128;

impl<P: Ports> Dispatcher<P> {
    /// Processes one `title` row. See the module documentation.
    pub(super) async fn title(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Title { ask } = row.payload.clone() else {
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
        // A person renamed the thread (or the model titled it) while the row waited.
        if thread.job.title.source() != TitleSource::FirstMessage {
            return self.finish(&row, OutboxFinal::Skipped).await;
        }
        let input = match self.app.task(TaskKind::Title) {
            // titles were switched off since the row was written
            None => Input::TitleDeclined { ask },
            Some(task) => {
                let events = self
                    .store()
                    .list_events(row.thread_id, 0, TITLE_EVENTS)
                    .await?;
                match self.title_in_language(&row, task, &events).await {
                    Some(title) => Input::Titled { ask, title },
                    None => Input::TitleDeclined { ask },
                }
            }
        };
        let lease = self.lease(&row);
        match self
            .app
            .apply_finishing(
                row.thread_id,
                input,
                format!("title:{}", row.id),
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

    /// The title the model writes for the conversation `events`, in the language the task's rule
    /// asks for, or `None`: it has no topic yet, it cannot be had, or it was in the wrong script
    /// twice.
    async fn title_in_language(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        events: &[Event],
    ) -> Option<String> {
        let spec = |retry| TaskPrompt {
            kind: TaskKind::Title,
            guidance: task.guidance.as_deref(),
            language: task.language,
            previous: None,
            retry,
        };
        let title = self
            .title_from(row, task, task_prompt(&spec(false), events))
            .await?;
        let Err(wrong) = check_task_language(task.language, events, &title) else {
            return Some(title);
        };
        tracing::debug!(
            id = %row.id,
            script = ?wrong.script,
            "the title is in a script the language rule does not allow; asking once more"
        );
        let title = self
            .title_from(row, task, task_prompt(&spec(true), events))
            .await?;
        if let Err(wrong) = check_task_language(task.language, events, &title) {
            tracing::debug!(
                id = %row.id,
                script = ?wrong.script,
                "the second title is in the wrong script too; the thread keeps the first words"
            );
            return None;
        }
        Some(title)
    }

    /// One question to the model and the title its answer stands for (`None` for no topic yet or
    /// no answer).
    async fn title_from(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        (system, user): (String, String),
    ) -> Option<String> {
        let request = ChatRequest {
            endpoint: task.endpoint.clone(),
            model: task.model.clone(),
            system,
            user,
            max_tokens: task.max_tokens,
        };
        let text = self
            .ask_model(row, task, &request, "the model could not title the thread")
            .await?;
        let title = clean_title(&text);
        if title.is_none() {
            tracing::debug!(id = %row.id, "the model had no title for the conversation yet");
        }
        title
    }
}

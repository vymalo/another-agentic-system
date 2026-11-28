//! The suggestions path of the dispatcher (ADR 0035, amended 2026-11-28): an outbox row of kind
//! `suggestions` asks the model for the next things the person could say under a job's finished
//! answer, and the answer goes back into the thread as exactly one input.
//!
//! Like the title and the description, **nothing depends on the answer**: a suggestion fills the
//! message box and never sends (ADR 0013, rule 7), so the row never fails a thread and never holds
//! anything back. Whatever happens, the row ends in the commit of [`Input::Suggested`] (the model's
//! words, cleaned by [`clean_suggestions`]) or [`Input::SuggestionsDeclined`], under the key
//! `suggestions:<row id>`, so one row is one event whoever works it.
//!
//! * **It is asked for the end of a job, once.** The core asks when a transition leaves the thread
//!   `done` or `blocked`, at most once per job; a later job asks again. A **child thread** never
//!   asks (its jobs follow a helper session's rounds), and a row whose thread is no longer `done`
//!   or `blocked` — the person answered meanwhile — declines without asking.
//! * **What the model is shown is the last exchange only**, read from the tail of the log when the
//!   row is worked: the person's last message and the agent's final answer **of the same job**,
//!   never the whole log ([`task_prompt`]). It is untrusted text and goes fenced, as data. A window
//!   with no message of the person declines without asking.
//! * **The suggestions are in the language the task's rule says, and checked**, as the title is:
//!   an answer in a script the rule refuses is declined and the model asked **once more in the
//!   same row** ([`check_task_language`]); wrong twice, the row is declined.
//! * **A model that stumbles is asked again, a little**, and then declined
//!   ([`ask_model`](Dispatcher::ask_model)); a slow model is given up on after the task's timeout.
//!   The person sees no error: the thread stays `done` with no suggestions.
//! * **Every write is fenced** with the row's claim: a worker that lost it writes nothing.

use orch_core::{
    Event, Input, TaskKind, TaskPrompt, ThreadState, check_task_language, clean_suggestions,
    has_exchange, task_prompt,
};
use orch_ports::{
    ChatRequest, OutboxFinal, OutboxItem, OutboxPayload, Ports, RunBindings, ThreadStore,
};

use super::{DispatchError, Dispatcher, Done};
use crate::{AppError, ApplyOutcome, LateSource, TaskSettings};

/// How many events of the tail of the log are read: the last exchange, and the person's messages
/// the language rule is decided from. The description's bound: a long job of tool calls is more
/// than a hundred events, and the person's message has to stay in the window.
const TAIL_EVENTS: u32 = 512;

impl<P: Ports, B: RunBindings> Dispatcher<P, B> {
    /// Processes one `suggestions` row. See the module documentation.
    pub(super) async fn suggestions(&self, row: OutboxItem) -> Done {
        let OutboxPayload::Suggestions { job } = row.payload.clone() else {
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
        let input = match self.app.task(TaskKind::Suggestions) {
            // suggestions were switched off since the row was written
            None => Input::SuggestionsDeclined { job },
            Some(task) => {
                if !matches!(thread.state, ThreadState::Done | ThreadState::Blocked) {
                    // The person answered since the row was written: the thread has moved on, and
                    // the suggestions of a job it is no longer waiting on are not asked for.
                    tracing::debug!(
                        id = %row.id,
                        state = thread.state.as_str(),
                        "the thread moved on; no suggestions"
                    );
                    Input::SuggestionsDeclined { job }
                } else {
                    let events = self.suggestions_events(&row, thread.last_seq).await?;
                    if !has_exchange(&events) {
                        tracing::debug!(
                            id = %row.id,
                            "no exchange of the person to suggest from; no model is asked"
                        );
                        Input::SuggestionsDeclined { job }
                    } else {
                        match self.suggestions_in_language(&row, task, &events).await? {
                            Some(suggestions) => Input::Suggested { job, suggestions },
                            None => Input::SuggestionsDeclined { job },
                        }
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
                format!("suggestions:{}", row.id),
                &lease,
                OutboxFinal::Delivered,
            )
            .await
        {
            Ok(ApplyOutcome::Fenced) => Err(DispatchError::Fenced),
            Ok(ApplyOutcome::Applied { .. }) => Ok(()),
            // written before, and its row not finished: end it now
            Ok(ApplyOutcome::Duplicate) => self.finish(&row, OutboxFinal::Delivered).await,
            Err(AppError::NotFound) => {
                self.app
                    .late_input_dropped(LateSource::Dispatcher, row.thread_id);
                self.finish(&row, OutboxFinal::Skipped).await
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The tail of the log a suggestions request is made from: the last exchange, and the person's
    /// messages the language rule is decided from.
    async fn suggestions_events(
        &self,
        row: &OutboxItem,
        last_seq: i64,
    ) -> Result<Vec<Event>, DispatchError> {
        let from = last_seq.saturating_sub(i64::from(TAIL_EVENTS)).max(0);
        Ok(self
            .store()
            .list_events(row.thread_id, from, TAIL_EVENTS)
            .await?)
    }

    /// The suggestions the model writes for the conversation `events`, in the language the task's
    /// rule asks for, or `None`: nothing to suggest, it cannot be had, or it was in the wrong
    /// script twice.
    async fn suggestions_in_language(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        events: &[Event],
    ) -> Result<Option<Vec<String>>, DispatchError> {
        let spec = |retry| TaskPrompt {
            kind: TaskKind::Suggestions,
            guidance: task.guidance.as_deref(),
            language: task.language,
            previous: None,
            retry,
        };
        let Some(suggestions) = self
            .suggestions_from(row, task, events, spec(false))
            .await?
        else {
            return Ok(None);
        };
        let Err(wrong) = check_task_language(task.language, events, &suggestions.join("\n")) else {
            return Ok(Some(suggestions));
        };
        tracing::debug!(
            id = %row.id,
            script = ?wrong.script,
            "the suggestions are in a script the language rule does not allow; asking once more"
        );
        let Some(suggestions) = self.suggestions_from(row, task, events, spec(true)).await? else {
            return Ok(None);
        };
        if let Err(wrong) = check_task_language(task.language, events, &suggestions.join("\n")) {
            tracing::debug!(
                id = %row.id,
                script = ?wrong.script,
                "the second answer is in the wrong script too; the thread keeps no suggestions"
            );
            return Ok(None);
        }
        Ok(Some(suggestions))
    }

    /// One question to the model and the suggestions its answer stands for (`None` for nothing to
    /// suggest or no answer).
    async fn suggestions_from(
        &self,
        row: &OutboxItem,
        task: &TaskSettings,
        events: &[Event],
        spec: TaskPrompt<'_>,
    ) -> Result<Option<Vec<String>>, DispatchError> {
        let (system, user) = task_prompt(&spec, events);
        let request = ChatRequest {
            endpoint: task.endpoint.clone(),
            model: task.model.clone(),
            system,
            user,
            max_tokens: task.max_tokens,
            run: None,
        };
        let text = self
            .ask_model(
                row,
                TaskKind::Suggestions,
                task,
                &request,
                "the model could not suggest what to say next",
            )
            .await?;
        let suggestions = text
            .map(|text| clean_suggestions(&text))
            .unwrap_or_default();
        if suggestions.is_empty() {
            tracing::debug!(id = %row.id, "the model had no suggestion for the conversation");
            return Ok(None);
        }
        Ok(Some(suggestions))
    }
}

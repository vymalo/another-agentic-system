//! A person starts and closes a helper session (`child-sessions/v1` sections 4 and 6, ADR 0072,
//! S3). Both are decided here, before anything is written, and carried out by the dispatcher
//! through the outbox: the start is **asynchronous**, answered with the id of the request, and its
//! outcome is the parent's `child_session_start_ended`; the close is asked of the agent and its
//! outcome is the child's `child_session_state`.
//!
//! What only the agent can refuse (its limits of open sessions, working rounds and rounds) arrives
//! after the answer, in the agent's own words.

use orch_core::{
    AccessTier, Input, MAX_CHILD_SESSIONS, MAX_START_DESCRIPTION_CHARS, PersonPrompt,
    SessionReport, SessionStartOutcome, SessionState, ThreadId,
};
use orch_ports::{Lease, Ports, ThreadStore};

use super::{App, ApplyOutcome, validate_text};
use crate::{AppError, ChildSessionDrop, Requester};

/// What a person asks of `startChildSession`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartChildSession {
    /// The id of the request, chosen by the caller: a repeat of it changes nothing.
    pub request: String,
    /// The helper, one of the card's.
    pub agent: String,
    /// What the session is for. Required: `None` is refused like a blank one.
    pub description: Option<String>,
    /// Round 1's prompt, which becomes the person's first message in the child thread.
    pub text: String,
    /// Whether the session goes idle after a round.
    pub keep_open: bool,
    /// The access tier of round 1, as for a message.
    pub tier: Option<AccessTier>,
}

/// What the orchestrator's record says after a close was asked (`closeChildSession`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildCloseState {
    /// A round works and the session closes when it ends, or the close is on its way.
    Closing,
    /// The session is closed.
    Closed,
}

/// Characters that read as nothing, or that reorder what is around them: refused in a description
/// as a control character is.
fn is_format(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{2069}'
            | '\u{feff}'
    )
}

/// The description of a start, as the agent will read it (trimmed), or the refusal in the agent's
/// own words (`bad_description`).
fn check_start_description(description: Option<&str>) -> Result<String, AppError> {
    let refuse = |detail: String| AppError::Arrangement {
        code: "bad_description",
        detail,
    };
    let Some(description) = description else {
        return Err(refuse(
            "`start.description` is missing: it is a string".to_owned(),
        ));
    };
    let description = description.trim();
    if description.is_empty() {
        return Err(refuse("`start.description` is blank".to_owned()));
    }
    if description.chars().any(|c| c.is_control() || is_format(c)) {
        return Err(refuse(
            "`start.description` is one line without control characters".to_owned(),
        ));
    }
    let chars = description.chars().count();
    if chars > MAX_START_DESCRIPTION_CHARS {
        return Err(refuse(format!(
            "`start.description` is {chars} characters; at most {MAX_START_DESCRIPTION_CHARS}"
        )));
    }
    Ok(description.to_owned())
}

impl<P: Ports> App<P> {
    /// A person starts a helper session of `parent`'s agent (`startChildSession`): the request is
    /// written (`child_session_start_asked`, with no text) and ordered, and the outcome comes back
    /// as the parent's `child_session_start_ended`. The same `request` again changes nothing.
    ///
    /// Every refusal below comes before anything is written.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for a thread that is not the person's; [`AppError::Forbidden`]
    /// without `thread.write` or `agent.invoke`; [`AppError::Invalid`] for a text that is empty or
    /// too long; [`AppError::Arrangement`] `child_thread` (the thread is a helper's),
    /// `no_child_sessions` (the live card does not offer helpers), `unknown_helper`,
    /// `bad_description`; [`AppError::Unprocessable`] for a tier above the role's cap or one the
    /// card does not honour; [`AppError::Blocked`] `no_context` (the agent has not answered the
    /// thread), `too_many_starts` (four starts still wait) and `too_many_sessions` (the thread
    /// notes [`MAX_CHILD_SESSIONS`] sessions); [`AppError::CardUnavailable`] when
    /// the card cannot be read; [`AppError::NotFound`] also for an agent the registry no longer lists.
    pub async fn start_child_session(
        &self,
        who: &impl Requester,
        parent: ThreadId,
        start: StartChildSession,
    ) -> Result<(), AppError> {
        let access = self.access(who);
        let thread = self.writable_thread(&access, parent).await?;
        if thread.child_of.is_some() {
            return Err(AppError::child_thread());
        }
        self.may_invoke(&access, &thread).await?;
        validate_text(&start.text)?;
        let description = check_start_description(start.description.as_deref())?;
        // the live card, now: the extension and the helper are the agent's to say
        let agent = self.canonical_agent(&thread.target.agent_id);
        // an agent the registry no longer lists is one that does not exist
        let entry = self
            .resolve_agent(&agent)
            .await?
            .ok_or(AppError::NotFound)?;
        let card =
            self.live_card(&entry.endpoint)
                .await
                .ok_or_else(|| AppError::CardUnavailable {
                    agent: agent.clone(),
                })?;
        let Some(offer) = card.child_sessions else {
            return Err(AppError::arrangement("no_child_sessions"));
        };
        if !offer.offers_agent(&start.agent) {
            let names: Vec<&str> = offer.agents().iter().map(|a| a.name.as_str()).collect();
            return Err(AppError::Arrangement {
                code: "unknown_helper",
                detail: format!(
                    "no helper `{}`; `start.agent` is one of: {}",
                    start.agent,
                    names.join(", ")
                ),
            });
        }
        let pin = self
            .resolve_tier(&access, &thread.target.agent_id, start.tier, None)
            .await?;
        // a session is started in the context the agent assigned the thread
        let has_context = self
            .ports
            .store()
            .get_binding(parent)
            .await?
            .and_then(|b| b.context_id)
            .is_some_and(|c| !c.is_empty());
        if !has_context {
            return Err(AppError::blocked(
                "no_context",
                "send the agent a message first",
            ));
        }
        // the parent notes a bounded number of sessions in its life: the agent would open one
        // the parent never records
        if thread.job.sessions.len() >= MAX_CHILD_SESSIONS {
            return Err(AppError::blocked(
                "too_many_sessions",
                "this thread has started as many helper sessions as one thread may note",
            ));
        }
        let key = format!("start:{}", start.request);
        let input = Input::StartSession {
            user: who.user().clone(),
            request: start.request,
            agent: start.agent,
            description,
            text: start.text,
            keep_open: start.keep_open,
            tier: pin.tier,
            tier_cap: pin.cap,
        };
        match self.apply(parent, input, Some(key), None, None).await? {
            ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate => Ok(()),
            ApplyOutcome::Fenced => Err(AppError::internal(
                "a commit without a lease was reported as fenced",
            )),
        }
    }

    /// A person closes the helper session `child` is (`closeChildSession`): an idle session
    /// closes at once, a working one when its round ends, and with `cancel` the round is stopped
    /// first. A session that is closed already, or whose close was asked with the same or a
    /// stronger `cancel`, writes nothing.
    ///
    /// # Errors
    /// [`AppError::NotFound`] for a thread that is not the person's; [`AppError::Forbidden`]
    /// without `thread.write`; [`AppError::Arrangement`] `not_child_thread` for a thread that is
    /// not a helper session's and `no_child_sessions` when the live card no longer offers helpers;
    /// [`AppError::CardUnavailable`] when the card cannot be read.
    pub async fn close_child_session(
        &self,
        who: &impl Requester,
        child: ThreadId,
        cancel: bool,
    ) -> Result<ChildCloseState, AppError> {
        let access = self.access(who);
        let thread = self.writable_thread(&access, child).await?;
        if thread.child_of.is_none() {
            return Err(AppError::arrangement("not_child_thread"));
        }
        // the live card, now, as a start reads it: an agent that stopped offering sessions takes
        // no close, and nothing is written for it. A session closed already is nothing either way.
        let open = thread
            .job
            .child
            .as_ref()
            .is_some_and(|c| c.state != SessionState::Closed);
        if open {
            let agent = self.canonical_agent(&thread.target.agent_id);
            let entry = self
                .resolve_agent(&agent)
                .await?
                .ok_or(AppError::NotFound)?;
            let card =
                self.live_card(&entry.endpoint)
                    .await
                    .ok_or_else(|| AppError::CardUnavailable {
                        agent: agent.clone(),
                    })?;
            if card.child_sessions.is_none() {
                return Err(AppError::arrangement("no_child_sessions"));
            }
        }
        // the ledger decides what is written: a repeat, or a close already as strong, is nothing
        let input = Input::CloseSession {
            user: who.user().clone(),
            cancel,
        };
        self.apply(child, input, None, None, None).await?;
        let after = self
            .ports
            .store()
            .get_thread(None, child)
            .await?
            .ok_or(AppError::NotFound)?;
        Ok(
            if after
                .job
                .child
                .is_some_and(|c| c.state == SessionState::Closed)
            {
                ChildCloseState::Closed
            } else {
                ChildCloseState::Closing
            },
        )
    }

    /// The agent opened the session a person started (the answer to the start's message): the
    /// child thread is made with the person's prompt before round 1, the parent's
    /// `child_session_opened` (by the person) is written, and the start ends `opened`. Every step
    /// is idempotent, so the answer applied twice (a resend after a lost claim) writes nothing
    /// twice. Returns `false` when `lease` is no longer ours.
    ///
    /// # Errors
    /// [`AppError::Store`] when the store fails, [`AppError::Contended`] when a thread kept
    /// changing.
    pub(crate) async fn person_session_opened(
        &self,
        parent_id: ThreadId,
        lease: Option<&Lease>,
        request: &str,
        report: SessionReport,
        prompt: PersonPrompt,
        end_key: String,
    ) -> Result<bool, AppError> {
        let Some(parent) = self.ports.store().get_thread(None, parent_id).await? else {
            self.late_input_dropped(super::LateSource::Dispatcher, parent_id);
            return Ok(true);
        };
        let session = report.session.clone();
        // the ledger filled while the agent opened the session: it is not noted, so no thread is
        // made for it, and the person is told
        if parent.job.sessions.note(&session).is_none()
            && parent.job.sessions.len() >= MAX_CHILD_SESSIONS
        {
            self.child_session_dropped(ChildSessionDrop::LedgerFull, parent_id);
            return self
                .end_start(
                    parent_id,
                    lease,
                    request,
                    SessionStartOutcome::Failed {
                        reason: "this thread has started as many helper sessions as one thread \
                                 may note"
                            .to_owned(),
                    },
                    end_key,
                )
                .await;
        }
        let Some((child, created)) =
            Box::pin(self.open_child(&parent, &report, Some(prompt))).await?
        else {
            // the store refused the thread: the person is told, and the start ends
            return self
                .end_start(
                    parent_id,
                    lease,
                    request,
                    SessionStartOutcome::Failed {
                        reason: "the helper's thread could not be made".to_owned(),
                    },
                    end_key,
                )
                .await;
        };
        if !created {
            self.tell_child(child.id, &report).await?;
        }
        if !self.tell_parent(parent_id, lease, report, child.id).await? {
            return Ok(false);
        }
        self.end_start(
            parent_id,
            lease,
            request,
            SessionStartOutcome::Opened {
                session,
                child: child.id,
            },
            end_key,
        )
        .await
    }

    /// The start of `request` ended: `child_session_start_ended` is written and the pending start
    /// dropped. Returns `false` when `lease` is no longer ours.
    pub(crate) async fn end_start(
        &self,
        parent: ThreadId,
        lease: Option<&Lease>,
        request: &str,
        outcome: SessionStartOutcome,
        key: String,
    ) -> Result<bool, AppError> {
        let input = Input::SessionStartEnded {
            request: request.to_owned(),
            outcome,
        };
        match self.apply(parent, input, Some(key), None, lease).await {
            Ok(ApplyOutcome::Fenced) => Ok(false),
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(true),
            // the parent was deleted while the start was in flight: nobody is waiting
            Err(AppError::NotFound) => Ok(true),
            Err(e) => Err(e),
        }
    }

    /// The agent refused a close a person asked for: the ask is cleared and the agent's words are
    /// the child's `error`. Returns `false` when `lease` is no longer ours.
    pub(crate) async fn close_refused(
        &self,
        child: ThreadId,
        lease: Option<&Lease>,
        reason: String,
        key: String,
    ) -> Result<bool, AppError> {
        let input = Input::SessionCloseFailed { reason };
        match self.apply(child, input, Some(key), None, lease).await {
            Ok(ApplyOutcome::Fenced) => Ok(false),
            Ok(ApplyOutcome::Applied { .. } | ApplyOutcome::Duplicate) => Ok(true),
            Err(AppError::NotFound) => Ok(true),
            Err(e) => Err(e),
        }
    }
}

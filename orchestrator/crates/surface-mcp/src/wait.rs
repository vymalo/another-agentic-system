//! `wait_for_job`: follow a job through its event log until it is over or blocked, or the wait
//! runs out (ADR 0019).
//!
//! It keeps no state in the process. What it says is read from the log through
//! [`App::event_stream`], the read every surface uses, from a cursor the caller gave
//! (`after_seq`). A call that ends for any reason (its timeout, a shutdown, a killed replica, a
//! dropped connection) is repeated with the cursor it reported, on any replica, and loses nothing:
//! the log is the source of truth, and a progress notification is only a note about an event.
//!
//! One call reads in two phases:
//!
//! 1. **Catch up.** The events after the cursor that the log holds when the call starts are read
//!    and reported first. No deadline applies: a call always reports what already happened, even
//!    with a timeout of zero. Then, if the job is finished or blocked, the call returns.
//! 2. **Live.** Events are reported as they arrive, a heartbeat notification goes out every
//!    `heartbeat` (so a client's idle window keeps being reset), and the call returns when the job
//!    is finished or blocked and its last event was read, when `timeout` runs out, or when the
//!    stream ends (the process is shutting down).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use orch_app::{App, AppError};
use orch_core::{Event, EventBody, ThreadId, ThreadRecord, ThreadState, UserId};
use orch_ports::Ports;
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// The longest text of a progress message.
const MAX_MESSAGE_CHARS: usize = 200;

/// Where the progress notifications of one call go.
///
/// The counter only increases within a call, and a `false` answer means the client is gone
/// (the call then ends).
pub trait ProgressSink: Send + Sync {
    /// Sends one notification. `progress` is 1 for the first and larger for every later one.
    fn send(&self, progress: u64, message: String) -> impl Future<Output = bool> + Send;
}

/// What one call asks for.
#[derive(Debug, Clone)]
pub struct WaitRequest {
    /// Report the events after this `seq`. `None` starts at the end of the log as it is when the
    /// call starts (only what happens next is reported); `Some(0)` reports the whole log. A value
    /// beyond the end counts as the end.
    pub after_seq: Option<i64>,
    /// How long the live phase may last.
    pub timeout: Duration,
    /// How often a heartbeat is sent.
    pub heartbeat: Duration,
}

/// Why a call returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WaitEnd {
    /// The job is `done`, `failed` or `cancelled`.
    Finished,
    /// The job waits for an answer (or, under a gate, for the user after a timeout).
    Blocked,
    /// The timeout ran out.
    TimedOut,
    /// The call was cut short: the process is shutting down, or the client went away.
    Interrupted,
}

impl WaitEnd {
    /// The wire spelling of the outcome.
    pub const fn as_str(self) -> &'static str {
        match self {
            WaitEnd::Finished => "finished",
            WaitEnd::Blocked => "blocked",
            WaitEnd::TimedOut => "timed_out",
            WaitEnd::Interrupted => "interrupted",
        }
    }
}

/// What a call ends with.
#[derive(Debug)]
pub struct Waited {
    /// Why it returned.
    pub end: WaitEnd,
    /// The `seq` of the last event this call read: call again with it as `after_seq` to lose
    /// nothing.
    pub resume_after_seq: i64,
    /// The job as it is now.
    pub thread: ThreadRecord,
}

/// Whether the job has nothing more to say until someone acts.
fn stopped(thread: &ThreadRecord) -> Option<WaitEnd> {
    match thread.state {
        ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled => Some(WaitEnd::Finished),
        ThreadState::Blocked => Some(WaitEnd::Blocked),
        ThreadState::Queued | ThreadState::Working | ThreadState::Verifying => None,
    }
}

/// One line for an event, or `None` for one that is not worth a notification (a partial message
/// of an agent that is still writing it). Text from agents is untrusted; it is cut short and
/// flattened onto one line.
pub fn describe(event: &Event) -> Option<String> {
    let text = match &event.body {
        EventBody::UserMessage(m) => format!("message from {}", m.origin.as_str()),
        EventBody::AgentMessage(m) if m.is_final => format!("agent message: {}", m.text),
        EventBody::AgentMessage(_) => return None,
        EventBody::AgentStatus(s) => match &s.detail {
            Some(detail) => format!("agent {}: {detail}", status_word(s.status)),
            None => format!("agent {}", status_word(s.status)),
        },
        EventBody::Artifact(a) => format!("artifact: {}", a.name),
        EventBody::ThreadState(s) => format!("job {}", s.state.as_str()),
        EventBody::Error(e) => format!("error: {}", e.message),
        EventBody::UiSurface(_) => "the agent showed an interface".to_owned(),
        EventBody::UiAction(_) => "an interface action".to_owned(),
        EventBody::CiResult(r) => format!("CI {}: {}", r.name, r.conclusion.as_str()),
        EventBody::CheckResult(c) => {
            format!("check {}: {:?}", c.source.as_str(), c.status).to_lowercase()
        }
        EventBody::Rework(r) => format!("rework: attempt {} of {}", r.attempt, r.max_attempts),
    };
    Some(format!("#{} {}", event.seq, one_line(&text)))
}

fn status_word(status: orch_core::AgentStatus) -> String {
    serde_json::to_value(status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "status".to_owned())
}

/// `text` on one line, at most [`MAX_MESSAGE_CHARS`] characters.
fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_MESSAGE_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(MAX_MESSAGE_CHARS - 1).collect();
    cut.push('…');
    cut
}

/// The notifications of one call: a counter that only increases.
struct Reporter<'a, S: ProgressSink> {
    sink: &'a S,
    counter: u64,
}

impl<S: ProgressSink> Reporter<'_, S> {
    /// Sends one notification; `false` when the client is gone.
    async fn send(&mut self, message: String) -> bool {
        self.counter += 1;
        self.sink.send(self.counter, message).await
    }
}

/// Follows `id` (a job of `user`) as `request` says, reporting through `sink`. `cancel` ends the
/// call when the client goes away.
///
/// The call is [`AppError::NotFound`] for a job that is not the user's, before anything is read.
pub async fn wait_for_job<P: Ports>(
    app: &Arc<App<P>>,
    user: &UserId,
    id: ThreadId,
    request: &WaitRequest,
    sink: &impl ProgressSink,
    cancel: &CancellationToken,
) -> Result<Waited, AppError> {
    let deadline = Instant::now()
        .checked_add(request.timeout)
        .unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400 * 365));
    let mut thread = app.get_thread(user, id).await?;
    let start = request
        .after_seq
        .map_or(thread.last_seq, |after| after.clamp(0, thread.last_seq));
    let mut cursor = start;
    let mut report = Reporter { sink, counter: 0 };
    let mut events = app.event_stream(user, id, start).await?;

    // The job as the log holds it now: the events up to here are reported whatever the deadline.
    let caught_up_to = thread.last_seq;
    while cursor < caught_up_to {
        let event = tokio::select! {
            biased;
            () = cancel.cancelled() => return finish(app, user, id, WaitEnd::Interrupted, cursor).await,
            event = events.next() => event,
        };
        let Some(event) = event else {
            return finish(app, user, id, WaitEnd::Interrupted, cursor).await;
        };
        cursor = event.seq;
        if let Some(message) = describe(&event)
            && !report.send(message).await
        {
            return finish(app, user, id, WaitEnd::Interrupted, cursor).await;
        }
    }
    thread = app.get_thread(user, id).await?;
    if cursor >= thread.last_seq
        && let Some(end) = stopped(&thread)
    {
        return Ok(Waited {
            end,
            resume_after_seq: cursor,
            thread,
        });
    }

    let mut heartbeat =
        tokio::time::interval_at(Instant::now() + request.heartbeat, request.heartbeat);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let end = tokio::select! {
            biased;
            () = cancel.cancelled() => Some(WaitEnd::Interrupted),
            event = events.next() => match event {
                // The stream ends once it has caught up and the process is shutting down.
                None => Some(WaitEnd::Interrupted),
                Some(event) => {
                    cursor = event.seq;
                    let sent = match describe(&event) {
                        Some(message) => report.send(message).await,
                        None => true,
                    };
                    if !sent {
                        Some(WaitEnd::Interrupted)
                    } else if matches!(event.body, EventBody::ThreadState(_)) {
                        // A job stops on the event that says so, once that is the last one.
                        thread = app.get_thread(user, id).await?;
                        (cursor >= thread.last_seq).then(|| stopped(&thread)).flatten()
                    } else {
                        None
                    }
                }
            },
            _ = heartbeat.tick() => {
                let message = format!("still waiting (job {}, last event #{cursor})", thread.state.as_str());
                if report.send(message).await {
                    // A safety net: a state the log did not announce is seen here at the latest.
                    thread = app.get_thread(user, id).await?;
                    (cursor >= thread.last_seq).then(|| stopped(&thread)).flatten()
                } else {
                    Some(WaitEnd::Interrupted)
                }
            },
            () = tokio::time::sleep_until(deadline) => Some(WaitEnd::TimedOut),
        };
        if let Some(end) = end {
            return finish(app, user, id, end, cursor).await;
        }
    }
}

/// Ends the call with `end`, unless the job turns out to have stopped by the time the log was
/// read to `cursor` (a state seen late is still the state).
async fn finish<P: Ports>(
    app: &Arc<App<P>>,
    user: &UserId,
    id: ThreadId,
    end: WaitEnd,
    cursor: i64,
) -> Result<Waited, AppError> {
    let thread = app.get_thread(user, id).await?;
    let end = match end {
        WaitEnd::TimedOut if cursor >= thread.last_seq => stopped(&thread).unwrap_or(end),
        other => other,
    };
    Ok(Waited {
        end,
        resume_after_seq: cursor,
        thread,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use orch_core::{
        Actor, AgentMessageData, AgentStatus, AgentStatusData, ArtifactData, ErrorData, Origin,
        ThreadStateData, UserMessageData,
    };

    use super::*;

    fn event(seq: i64, body: EventBody) -> Event {
        Event {
            seq,
            thread_id: ThreadId(uuid::Uuid::nil()),
            at: "2026-09-30T10:00:00Z".parse().unwrap(),
            actor: Actor::system(),
            body,
        }
    }

    #[test]
    fn every_kind_of_event_that_matters_gets_one_line_with_its_seq() {
        let cases = [
            (
                EventBody::UserMessage(UserMessageData {
                    origin: Origin::Mcp,
                    ..UserMessageData::new("hi")
                }),
                "#1 message from mcp",
            ),
            (
                EventBody::AgentStatus(AgentStatusData {
                    status: AgentStatus::InputRequired,
                    detail: Some("Which branch?".to_owned()),
                }),
                "#1 agent input_required: Which branch?",
            ),
            (
                EventBody::AgentStatus(AgentStatusData {
                    status: AgentStatus::Working,
                    detail: None,
                }),
                "#1 agent working",
            ),
            (
                EventBody::Artifact(ArtifactData {
                    name: "Pull request".to_owned(),
                    mime_type: None,
                    uri: None,
                    text: None,
                }),
                "#1 artifact: Pull request",
            ),
            (
                EventBody::ThreadState(ThreadStateData {
                    state: ThreadState::Done,
                }),
                "#1 job done",
            ),
            (
                EventBody::Error(ErrorData {
                    message: "boom\nsecond line".to_owned(),
                    retryable: true,
                }),
                "#1 error: boom second line",
            ),
        ];
        for (body, want) in cases {
            assert_eq!(describe(&event(1, body)).as_deref(), Some(want));
        }
    }

    #[test]
    fn a_partial_message_is_not_worth_a_notification_and_a_long_one_is_cut() {
        let partial = EventBody::AgentMessage(AgentMessageData {
            text: "half a sen".to_owned(),
            message_id: "m".to_owned(),
            is_final: false,
        });
        assert_eq!(describe(&event(2, partial)), None);
        let long = EventBody::AgentMessage(AgentMessageData {
            text: "x".repeat(1000),
            message_id: "m".to_owned(),
            is_final: true,
        });
        let line = describe(&event(3, long)).unwrap();
        assert!(line.chars().count() <= MAX_MESSAGE_CHARS + 3, "{line}");
        assert!(line.ends_with('…'));
    }
}

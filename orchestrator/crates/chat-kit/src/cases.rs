//! The four cases of the chat contract, each a function that takes the factory building a home.
//!
//! A case asks the factory for the homes it needs, each with its own [`Setup`], and returns
//! [`Failure`] with a message naming what the home did wrong. `None` from the factory means the
//! home is unavailable: the case prints `skipped: …` and passes, as the port testkits do.

use std::fmt::Display;
use std::future::Future;

use orch_agui_proto as agui;

use crate::{ChatHome, Failure, HomeError, LimitRule, Setup};

/// The `CUSTOM` name of the event that says the application's limit is reached: the same name the
/// embed home and a cloud adapter use.
pub const LIMIT_EVENT: &str = "vymalo.limit_reached";

/// Why the case did not run.
fn skipped(reason: &str) -> Result<(), Failure> {
    eprintln!("skipped: {reason}");
    Ok(())
}

/// The failure for a home that answered with an error where the case needed a success.
fn failed(what: impl Display, error: HomeError) -> Failure {
    Failure::new(format!("{what}: {error}"))
}

/// The first case: a replay from a sequence number says only what follows it.
///
/// It sends one message on a fresh thread, reads the whole thread as a replay from 0, keeps every
/// resume point (at least two), replays from the middle one, and asserts that the replay is not
/// empty, starts with `RUN_STARTED`, ends with `RUN_FINISHED`, has no resume id at or before the
/// cursor (no repeat, no frame from before it), has strictly increasing resume ids (no repeat, no
/// reordering), and holds strictly fewer frames than the whole thread (no gap).
///
/// The whole and the tail are both read through [`ChatHome::replay`], never through
/// [`ChatHome::send`]: a home's send answers the *requester* (whose own message it already holds
/// and may not restate), while a replay is the viewer's, so comparing a requester stream with a
/// viewer tail would compare two audiences.
pub async fn replay_from_a_seq_says_only_what_follows<F, Fut, H>(make: F) -> Result<(), Failure>
where
    F: Fn(Setup) -> Fut,
    Fut: Future<Output = Option<H>>,
    H: ChatHome,
{
    let Some(home) = make(Setup::default()).await else {
        return skipped("the home is unavailable");
    };
    let sent = home
        .send(None, "kit-replay-1", "the first message")
        .await
        .map_err(|error| failed("the first send failed", error))?;
    let whole = home
        .replay(&sent.thread, 0)
        .await
        .map_err(|error| failed("the replay from the start failed", error))?;
    let points: Vec<i64> = whole.iter().filter_map(|frame| frame.resume_id).collect();
    if points.len() < 2 {
        return Err(Failure::new(format!(
            "the run has {} resume points; at least two are needed",
            points.len()
        )));
    }
    let mid = points[points.len() / 2 - 1];
    let tail = home
        .replay(&sent.thread, mid)
        .await
        .map_err(|error| failed(format!("the replay after {mid} failed"), error))?;
    if tail.is_empty() {
        return Err(Failure::new(format!("the replay after {mid} said nothing")));
    }
    let ids: Vec<i64> = tail.iter().filter_map(|frame| frame.resume_id).collect();
    if let Some(repeated) = ids.iter().find(|seq| **seq <= mid) {
        return Err(Failure::new(format!(
            "the replay after {mid} repeated the frame at seq {repeated}"
        )));
    }
    // No gap and no repeat: the tail's resume points are exactly the whole thread's, after the
    // cursor, in the same order. Both reads are replays, so the two lists are the same audience's.
    let expected: Vec<i64> = points.iter().copied().filter(|seq| *seq > mid).collect();
    if ids != expected {
        return Err(Failure::new(format!(
            "the replay after {mid} said the resume ids {ids:?}, not the whole thread's after it ({expected:?})"
        )));
    }
    if !matches!(
        tail.first().map(|frame| &frame.event),
        Some(agui::Event::RunStarted(_))
    ) {
        return Err(Failure::new(format!(
            "the replay after {mid} did not start with RUN_STARTED"
        )));
    }
    if !matches!(
        tail.last().map(|frame| &frame.event),
        Some(agui::Event::RunFinished(_))
    ) {
        return Err(Failure::new(format!(
            "the replay after {mid} did not end with RUN_FINISHED"
        )));
    }
    if tail.len() >= whole.len() {
        return Err(Failure::new(format!(
            "the replay after {mid} said {} frames, not fewer than the whole thread's {}",
            tail.len(),
            whole.len()
        )));
    }
    Ok(())
}

/// The second case: the client message id is the idempotency key.
///
/// It sends a first message on a fresh thread, then sends `client-1` twice with the same words,
/// and asserts that the thread holds exactly two user messages and that `client-1` appears once.
pub async fn a_client_message_id_makes_a_retry_one_message<F, Fut, H>(
    make: F,
) -> Result<(), Failure>
where
    F: Fn(Setup) -> Fut,
    Fut: Future<Output = Option<H>>,
    H: ChatHome,
{
    let Some(home) = make(Setup::default()).await else {
        return skipped("the home is unavailable");
    };
    let first = home
        .send(None, "kit-message-1", "the first message")
        .await
        .map_err(|error| failed("the first send failed", error))?;
    home.send(Some(&first.thread), "client-1", "the same words")
        .await
        .map_err(|error| failed("the first client-1 send failed", error))?;
    home.send(Some(&first.thread), "client-1", "the same words")
        .await
        .map_err(|error| failed("the retried client-1 send failed", error))?;
    let messages = home
        .user_messages(&first.thread)
        .await
        .map_err(|error| failed("reading the user messages failed", error))?;
    if messages.len() != 2 {
        return Err(Failure::new(format!(
            "the thread holds {} user messages, not 2",
            messages.len()
        )));
    }
    let retries = messages
        .iter()
        .filter(|message| message.id == "client-1")
        .count();
    if retries != 1 {
        return Err(Failure::new(format!(
            "client-1 appears {retries} times, not once"
        )));
    }
    Ok(())
}

/// The third case: a limit reached during a run is one `CUSTOM` event.
///
/// A home that reports [`LimitRule::NotApplicable`] skips the case with its reason. Otherwise the
/// send must be [`HomeError::Limit`] and its event must be `CUSTOM` named [`LIMIT_EVENT`], with
/// `value.status == 402`, a non-empty string `value.code` and a non-empty `value.message`.
pub async fn a_limit_is_one_custom_event<F, Fut, H>(make: F) -> Result<(), Failure>
where
    F: Fn(Setup) -> Fut,
    Fut: Future<Output = Option<H>>,
    H: ChatHome,
{
    let Some(home) = make(Setup {
        limit: true,
        ..Setup::default()
    })
    .await
    else {
        return skipped("the home is unavailable");
    };
    match home.limit_rule() {
        LimitRule::NotApplicable(reason) => return skipped(reason),
        LimitRule::Enforced => {}
    }
    let event = match home.send(None, "kit-limit-1", "the first message").await {
        Err(HomeError::Limit { event }) => event,
        Err(other) => {
            return Err(Failure::new(format!(
                "the send failed with {other:?}, not the 402 limit"
            )));
        }
        Ok(_) => {
            return Err(Failure::new(
                "the send was allowed although the home says it enforces a limit",
            ));
        }
    };
    let custom = match event {
        agui::Event::Custom(custom) => custom,
        other => {
            return Err(Failure::new(format!(
                "the 402 was not a CUSTOM event: {other:?}"
            )));
        }
    };
    if custom.name != LIMIT_EVENT {
        return Err(Failure::new(format!(
            "the 402 event is named {:?}, not {LIMIT_EVENT:?}",
            custom.name
        )));
    }
    if custom.value["status"] != 402 {
        return Err(Failure::new(format!(
            "the 402 event's status is {}, not 402",
            custom.value["status"]
        )));
    }
    if custom.value["code"].as_str().is_none_or(str::is_empty) {
        return Err(Failure::new("the 402 event has no code"));
    }
    if custom.value["message"].as_str().is_none_or(str::is_empty) {
        return Err(Failure::new("the 402 event has no message"));
    }
    Ok(())
}

/// The fourth case: an old, unreadable or absent client is refused with 426.
///
/// Three homes from the same factory: a client stating `2.0.9` under the minimum `2.1.0` is
/// [`HomeError::UpdateRequired`] naming `2.1.0`; a client stating no version under the same
/// minimum is refused too; a client stating `2.1` is allowed to send on a fresh thread.
pub async fn an_old_or_unstated_client_is_refused<F, Fut, H>(make: F) -> Result<(), Failure>
where
    F: Fn(Setup) -> Fut,
    Fut: Future<Output = Option<H>>,
    H: ChatHome,
{
    const MIN: &str = "2.1.0";
    let Some(home) = make(Setup {
        min_client_version: Some(MIN),
        client_version: Some("2.0.9"),
        ..Setup::default()
    })
    .await
    else {
        return skipped("the home is unavailable");
    };
    match home.send(None, "kit-gate-old", "the first message").await {
        Err(HomeError::UpdateRequired { min }) if min == MIN => {}
        other => {
            return Err(Failure::new(format!(
                "an old client was answered {other:?}, not 426 for {MIN}"
            )));
        }
    }

    let Some(home) = make(Setup {
        min_client_version: Some(MIN),
        ..Setup::default()
    })
    .await
    else {
        return skipped("the home is unavailable");
    };
    match home.send(None, "kit-gate-none", "the first message").await {
        Err(HomeError::UpdateRequired { .. }) => {}
        other => {
            return Err(Failure::new(format!(
                "a client stating no version was answered {other:?}, not 426"
            )));
        }
    }

    let Some(home) = make(Setup {
        min_client_version: Some(MIN),
        client_version: Some("2.1"),
        ..Setup::default()
    })
    .await
    else {
        return skipped("the home is unavailable");
    };
    home.send(None, "kit-gate-current", "the first message")
        .await
        .map_err(|error| failed("a client at the minimum was refused", error))?;
    Ok(())
}

//! Reads what an agent says under `child-sessions/v1` (ADR 0072, `docs/api/child-sessions-v1.md`)
//! into a [`SessionNews`], strictly and within bounds. No I/O.
//!
//! Three shapes sit under the URI: the `sessions` object of a parent task (every session of the
//! context), an `event` of a parent's status update (one session), and the object a round task
//! carries about the session it belongs to. Anything that breaks a bound refuses the whole entry:
//! the reason is a fixed text, never a value of the entry.

use orch_core::{
    AgentTaskState, MAX_SESSION_AGENT_BYTES, MAX_SESSION_CONTEXT_BYTES,
    MAX_SESSION_DESCRIPTION_CHARS, MAX_SESSION_ID_BYTES, MAX_SESSION_TASK_BYTES,
    MAX_SESSIONS_LISTED, RoundReport, SessionBy, SessionEvent, SessionNews, SessionReport,
    SessionState,
};
use serde_json::{Map, Value};

/// Why an entry was refused: a fixed text, never a value from the entry.
pub(crate) type Refused = &'static str;

type Object = Map<String, Value>;

/// Where a session report is read from: what each shape leaves to the task it rides on.
enum Shape<'a> {
    /// An entry of a `sessions` array.
    Listed,
    /// A status update's event: the session's state follows from the event and the round.
    Event(SessionEvent),
    /// A round task's own object: the round is the task, read from `task_id` and `task_state`.
    Round {
        task_id: &'a str,
        context_id: &'a str,
        task_state: Option<AgentTaskState>,
    },
}

/// Reads the entry under the extension's URI of a task or a status update. `Ok(None)` for an
/// object that says nothing of sessions (unknown members are ignored).
///
/// `task_id`, `context_id` and `task_state` are those of the task or the update the entry came
/// on: a round task's own object has no `contextId` or round task id of its own.
pub(crate) fn read(
    entry: &Value,
    task_id: &str,
    context_id: &str,
    task_state: Option<AgentTaskState>,
) -> Result<Option<SessionNews>, Refused> {
    let object = entry.as_object().ok_or("the entry is not an object")?;
    if let Some(list) = object.get("sessions") {
        let list = list.as_array().ok_or("sessions is not an array")?;
        if list.len() > MAX_SESSIONS_LISTED {
            return Err("sessions lists too many sessions");
        }
        let mut reports: Vec<SessionReport> = Vec::with_capacity(list.len());
        for item in list {
            let item = item.as_object().ok_or("a session is not an object")?;
            let report = report(item, &Shape::Listed)?;
            if reports.iter().any(|r| r.session == report.session) {
                return Err("a session is listed twice");
            }
            reports.push(report);
        }
        return Ok(Some(SessionNews::Listed(reports)));
    }
    if let Some(event) = object.get("event") {
        let kind = match event.as_str() {
            Some("opened") => SessionEvent::Opened,
            Some("round") => SessionEvent::Round,
            Some("reported") => SessionEvent::Reported,
            Some("closed") => SessionEvent::Closed,
            _ => return Err("the event is not one of opened, round, reported, closed"),
        };
        return Ok(Some(SessionNews::Event(
            kind,
            report(object, &Shape::Event(kind))?,
        )));
    }
    if object.contains_key("session") {
        let shape = Shape::Round {
            task_id,
            context_id,
            task_state,
        };
        return Ok(Some(SessionNews::OfRound(report(object, &shape)?)));
    }
    Ok(None)
}

fn printable(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

fn text<'a>(object: &'a Object, key: &str, missing: Refused) -> Result<&'a str, Refused> {
    object.get(key).and_then(Value::as_str).ok_or(missing)
}

fn flag(object: &Object, key: &str, wrong: Refused) -> Result<bool, Refused> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(wrong),
    }
}

/// A whole number from 1 to `u32::MAX`, written either way (A2A metadata is a protobuf `Struct`).
fn round_number(value: &Value) -> Option<u32> {
    let n = match value.as_u64() {
        Some(n) => n,
        None => {
            let f = value.as_f64()?;
            if f.fract() != 0.0 || !(1.0..=f64::from(u32::MAX)).contains(&f) {
                return None;
            }
            // whole and in range, checked above
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let n = f as u64;
            n
        }
    };
    u32::try_from(n).ok().filter(|n| *n >= 1)
}

fn session_state(word: &str) -> Option<SessionState> {
    match word {
        "working" => Some(SessionState::Working),
        "idle" => Some(SessionState::Idle),
        "closed" => Some(SessionState::Closed),
        _ => None,
    }
}

/// The task state an A2A word names: `Some(Some(state))` for ProtoJSON (`TASK_STATE_WORKING`) or
/// the plain word, `Some(None)` for `TASK_STATE_UNSPECIFIED` (unknown), `None` for a word that is
/// neither.
pub(crate) fn state_word(word: &str) -> Option<Option<AgentTaskState>> {
    let state = match word {
        "TASK_STATE_UNSPECIFIED" => return Some(None),
        "TASK_STATE_SUBMITTED" | "submitted" => AgentTaskState::Submitted,
        "TASK_STATE_WORKING" | "working" => AgentTaskState::Working,
        "TASK_STATE_COMPLETED" | "completed" => AgentTaskState::Completed,
        "TASK_STATE_FAILED" | "failed" => AgentTaskState::Failed,
        "TASK_STATE_CANCELED" | "canceled" => AgentTaskState::Canceled,
        "TASK_STATE_INPUT_REQUIRED" | "input-required" => AgentTaskState::InputRequired,
        "TASK_STATE_REJECTED" | "rejected" => AgentTaskState::Rejected,
        "TASK_STATE_AUTH_REQUIRED" | "auth-required" => AgentTaskState::AuthRequired,
        _ => return None,
    };
    Some(Some(state))
}

/// The `round` object of a session or an event: `number` from 1, `taskId` optional, `state`
/// optional (`null` is unknown).
fn round_of(value: &Value) -> Result<RoundReport, Refused> {
    let round = value.as_object().ok_or("the round is not an object")?;
    let number = round
        .get("number")
        .and_then(round_number)
        .ok_or("the round number is not a whole number from 1")?;
    let task_id = match round.get("taskId") {
        None | Some(Value::Null) => None,
        Some(Value::String(t)) if printable(t, MAX_SESSION_TASK_BYTES) => Some(t.clone()),
        Some(_) => return Err("the round task id is not printable text of a bounded size"),
    };
    let state = match round.get("state") {
        None | Some(Value::Null) => None,
        Some(Value::String(word)) => {
            state_word(word).ok_or("the round state is not a task state")?
        }
        Some(_) => return Err("the round state is not a task state"),
    };
    Ok(RoundReport {
        number,
        task_id,
        state,
    })
}

/// What an event says of the session's state, from its kind and its round: a round that ended
/// leaves a kept-open session idle and another closed, unless the round waits for the person.
fn state_of_event(
    kind: SessionEvent,
    keep_open: bool,
    round: Option<&RoundReport>,
) -> SessionState {
    match kind {
        SessionEvent::Opened | SessionEvent::Round => SessionState::Working,
        SessionEvent::Closed => SessionState::Closed,
        SessionEvent::Reported => {
            let waits = round.and_then(|r| r.state).is_some_and(|s| {
                matches!(
                    s,
                    AgentTaskState::InputRequired | AgentTaskState::AuthRequired
                )
            });
            if waits {
                SessionState::Working
            } else if keep_open {
                SessionState::Idle
            } else {
                SessionState::Closed
            }
        }
    }
}

fn report(object: &Object, shape: &Shape<'_>) -> Result<SessionReport, Refused> {
    let session = text(object, "session", "the session id is missing")?;
    if !printable(session, MAX_SESSION_ID_BYTES) {
        return Err("the session id is not 1 to 128 printable bytes");
    }
    let context_id = match shape {
        Shape::Round { context_id, .. } => *context_id,
        _ => text(object, "contextId", "the context id is missing")?,
    };
    if !printable(context_id, MAX_SESSION_CONTEXT_BYTES) {
        return Err("the context id is not 1 to 512 printable bytes");
    }
    let agent = text(object, "agent", "the helper name is missing")?;
    if agent.len() > MAX_SESSION_AGENT_BYTES || agent.chars().any(char::is_control) {
        return Err("the helper name is not at most 128 printable bytes");
    }
    let description = match object.get("description") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(d)) => d.chars().take(MAX_SESSION_DESCRIPTION_CHARS).collect(),
        Some(_) => return Err("the description is not text"),
    };
    let keep_open = flag(object, "keepOpen", "keepOpen is not a boolean")?;
    let closing = flag(object, "closing", "closing is not a boolean")?;
    let by = match object.get("by") {
        None | Some(Value::Null) => None,
        Some(Value::String(w)) if w == "agent" => Some(SessionBy::Agent),
        Some(Value::String(w)) if w == "person" => Some(SessionBy::Person),
        Some(_) => return Err("by is neither agent nor person"),
    };
    let round = match shape {
        Shape::Round {
            task_id,
            task_state,
            ..
        } => {
            let number = object
                .get("round")
                .and_then(round_number)
                .ok_or("the round number is not a whole number from 1")?;
            if !printable(task_id, MAX_SESSION_TASK_BYTES) {
                return Err("the round task id is not printable text of a bounded size");
            }
            Some(RoundReport {
                number,
                task_id: Some((*task_id).to_owned()),
                state: *task_state,
            })
        }
        _ => object.get("round").map(round_of).transpose()?,
    };
    let state = match shape {
        Shape::Event(kind) => state_of_event(*kind, keep_open, round.as_ref()),
        Shape::Listed | Shape::Round { .. } => object
            .get("state")
            .and_then(Value::as_str)
            .and_then(session_state)
            .ok_or("the session state is not working, idle or closed")?,
    };
    Ok(SessionReport {
        session: session.to_owned(),
        context_id: context_id.to_owned(),
        agent: agent.to_owned(),
        description,
        keep_open,
        by,
        state,
        closing,
        round,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use orch_core::{RoundReport, SessionBy, SessionEvent, SessionReport, SessionState};
    use serde_json::{Value, json};

    use super::*;

    fn listed(entry: &Value) -> Vec<SessionReport> {
        match read(entry, "t1", "c1", Some(AgentTaskState::Working)) {
            Ok(Some(SessionNews::Listed(v))) => v,
            other => panic!("not a listing: {other:?}"),
        }
    }

    fn refused(entry: &Value) -> Refused {
        read(entry, "t1", "c1", Some(AgentTaskState::Working)).expect_err("should be refused")
    }

    fn session(extra: Value) -> Value {
        let mut base = json!({
            "session": "s1", "contextId": "k1", "agent": "planner",
            "description": "Plan it", "keepOpen": true, "state": "working",
            "round": {"number": 1, "taskId": "r1", "state": "TASK_STATE_WORKING"}
        });
        if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        base
    }

    #[test]
    fn a_sessions_object_is_read_with_the_protojson_state() {
        let got = listed(&json!({"sessions": [session(json!({}))]}));
        assert_eq!(
            got,
            vec![SessionReport {
                session: "s1".into(),
                context_id: "k1".into(),
                agent: "planner".into(),
                description: "Plan it".into(),
                keep_open: true,
                by: None,
                state: SessionState::Working,
                closing: false,
                round: Some(RoundReport {
                    number: 1,
                    task_id: Some("r1".into()),
                    state: Some(AgentTaskState::Working),
                }),
            }]
        );
    }

    #[test]
    fn every_task_state_word_is_read_in_both_spellings_and_null_is_unknown() {
        for (word, want) in [
            ("TASK_STATE_SUBMITTED", AgentTaskState::Submitted),
            ("TASK_STATE_WORKING", AgentTaskState::Working),
            ("TASK_STATE_COMPLETED", AgentTaskState::Completed),
            ("TASK_STATE_FAILED", AgentTaskState::Failed),
            ("TASK_STATE_CANCELED", AgentTaskState::Canceled),
            ("TASK_STATE_INPUT_REQUIRED", AgentTaskState::InputRequired),
            ("TASK_STATE_REJECTED", AgentTaskState::Rejected),
            ("TASK_STATE_AUTH_REQUIRED", AgentTaskState::AuthRequired),
            ("submitted", AgentTaskState::Submitted),
            ("working", AgentTaskState::Working),
            ("completed", AgentTaskState::Completed),
            ("failed", AgentTaskState::Failed),
            ("canceled", AgentTaskState::Canceled),
            ("input-required", AgentTaskState::InputRequired),
            ("rejected", AgentTaskState::Rejected),
            ("auth-required", AgentTaskState::AuthRequired),
        ] {
            assert_eq!(state_word(word), Some(Some(want)), "{word}");
        }
        assert_eq!(state_word("TASK_STATE_UNSPECIFIED"), Some(None));
        assert_eq!(state_word("done"), None);
        assert_eq!(state_word(""), None);
        let got =
            listed(&json!({"sessions": [session(json!({"round": {"number": 2, "state": null}}))]}));
        let round = got[0].round.as_ref().unwrap();
        assert_eq!(
            (round.number, round.task_id.clone(), round.state),
            (2, None, None)
        );
        let plain = listed(
            &json!({"sessions": [session(json!({"round": {"number": 1, "state": "completed"}}))]}),
        );
        assert_eq!(
            plain[0].round.as_ref().unwrap().state,
            Some(AgentTaskState::Completed)
        );
    }

    #[test]
    fn whole_doubles_and_optional_members_are_read() {
        let got = listed(&json!({"sessions": [
            {"session": "s", "contextId": "k", "agent": "a", "state": "idle", "by": "person",
             "closing": true, "round": {"number": 2.0}}
        ]}));
        assert_eq!(got[0].description, "");
        assert!(!got[0].keep_open);
        assert_eq!(got[0].by, Some(SessionBy::Person));
        assert!(got[0].closing);
        assert_eq!(got[0].round.as_ref().unwrap().number, 2);
        assert_eq!(listed(&json!({"sessions": []})), vec![]);
    }

    #[test]
    fn a_long_description_is_cut_and_the_bounds_refuse() {
        let long = "é".repeat(300);
        let got = listed(&json!({"sessions": [session(json!({"description": long}))]}));
        assert_eq!(got[0].description.chars().count(), 256);

        let bad = |extra: Value| refused(&json!({"sessions": [session(extra)]}));
        bad(json!({"session": ""}));
        bad(json!({"session": "x".repeat(129)}));
        bad(json!({"session": "a\nb"}));
        bad(json!({"session": 5}));
        bad(json!({"contextId": ""}));
        bad(json!({"contextId": "x".repeat(513)}));
        bad(json!({"contextId": "a\u{7}"}));
        bad(json!({"agent": "x".repeat(129)}));
        bad(json!({"agent": 3}));
        bad(json!({"description": 3}));
        bad(json!({"keepOpen": "yes"}));
        bad(json!({"by": "robot"}));
        bad(json!({"by": 1}));
        bad(json!({"state": "open"}));
        bad(json!({"state": null}));
        bad(json!({"closing": 1}));
        bad(json!({"round": {"number": 0}}));
        bad(json!({"round": {"number": 1.5}}));
        bad(json!({"round": {"number": 4294967296_u64}}));
        bad(json!({"round": {"number": -1}}));
        bad(json!({"round": {"number": 1, "taskId": ""}}));
        bad(json!({"round": {"number": 1, "taskId": "t".repeat(257)}}));
        bad(json!({"round": {"number": 1, "state": "done"}}));
        bad(json!({"round": {"number": 1, "state": 4}}));
        bad(json!({"round": "1"}));
        bad(json!({"round": {"taskId": "r"}}));
        // a bad entry among good ones refuses the lot, and an id may be listed once
        refused(&json!({"sessions": [session(json!({})), session(json!({"session": ""}))]}));
        refused(&json!({"sessions": [session(json!({})), session(json!({}))]}));
        refused(&json!({"sessions": "none"}));
        refused(&json!({"sessions": [5]}));
        refused(&json!("sessions"));
        refused(&json!([]));
    }

    #[test]
    fn at_most_512_sessions_are_listed() {
        let many = |n: usize| json!({"sessions": (0..n).map(|i| session(json!({"session": format!("s{i}")}))).collect::<Vec<_>>()});
        assert_eq!(listed(&many(512)).len(), 512);
        refused(&many(513));
    }

    #[test]
    fn an_event_is_read_with_the_state_it_implies() {
        let ev = |event: &str, keep: bool, round_state: &str| {
            json!({"event": event, "session": "s1", "contextId": "k1", "agent": "planner",
                   "description": "d", "keepOpen": keep, "by": "agent",
                   "round": {"number": 1, "taskId": "r1", "state": round_state}})
        };
        let read_event =
            |entry: &Value| match read(entry, "t1", "c1", Some(AgentTaskState::Working)) {
                Ok(Some(SessionNews::Event(kind, report))) => (kind, report),
                other => panic!("not an event: {other:?}"),
            };
        let (kind, r) = read_event(&ev("opened", true, "TASK_STATE_WORKING"));
        assert_eq!(
            (kind, r.state, r.by),
            (
                SessionEvent::Opened,
                SessionState::Working,
                Some(SessionBy::Agent)
            )
        );
        let (kind, r) = read_event(&ev("round", true, "TASK_STATE_WORKING"));
        assert_eq!(
            (kind, r.state),
            (SessionEvent::Round, SessionState::Working)
        );
        let (kind, r) = read_event(&ev("reported", true, "TASK_STATE_COMPLETED"));
        assert_eq!(
            (kind, r.state),
            (SessionEvent::Reported, SessionState::Idle)
        );
        let (_, r) = read_event(&ev("reported", false, "TASK_STATE_COMPLETED"));
        assert_eq!(r.state, SessionState::Closed);
        // a round that waits for the person leaves the session working
        let (_, r) = read_event(&ev("reported", false, "TASK_STATE_INPUT_REQUIRED"));
        assert_eq!(r.state, SessionState::Working);
        let (kind, r) = read_event(&ev("closed", true, "TASK_STATE_COMPLETED"));
        assert_eq!(
            (kind, r.state),
            (SessionEvent::Closed, SessionState::Closed)
        );
        refused(&ev("started", true, "TASK_STATE_WORKING"));
        refused(&json!({"event": 3, "session": "s1", "contextId": "k", "agent": "a"}));
    }

    #[test]
    fn a_round_tasks_own_object_is_its_session() {
        let entry = json!({"session": "s1", "parentContextId": "c0", "agent": "planner",
            "description": "d", "keepOpen": true, "round": 2.0, "by": "person",
            "state": "working", "closing": true});
        let got = read(&entry, "r2", "k1", Some(AgentTaskState::Completed))
            .unwrap()
            .unwrap();
        let SessionNews::OfRound(r) = got else {
            panic!("{got:?}")
        };
        assert_eq!(r.context_id, "k1");
        assert_eq!(r.by, Some(SessionBy::Person));
        assert!(r.closing);
        assert_eq!(
            r.round,
            Some(RoundReport {
                number: 2,
                task_id: Some("r2".into()),
                state: Some(AgentTaskState::Completed)
            })
        );
        let bad = json!({"session": "s1", "agent": "a", "round": 0, "state": "working"});
        assert!(read(&bad, "r", "k", None).is_err());
        let bad =
            json!({"session": "s1", "agent": "a", "round": {"number": 1}, "state": "working"});
        assert!(read(&bad, "r", "k", None).is_err());
        assert!(read(&entry, "r2", "", None).is_err());
    }

    #[test]
    fn an_entry_that_says_nothing_of_sessions_is_nothing() {
        assert_eq!(read(&json!({}), "t", "c", None), Ok(None));
        assert_eq!(read(&json!({"other": 1}), "t", "c", None), Ok(None));
    }
}

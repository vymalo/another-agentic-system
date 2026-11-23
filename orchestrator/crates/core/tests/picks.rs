//! The model and reasoning effort a person picks for a message (ADR 0086, `model-pick/v1`): what the
//! core records of it and what it carries. Whether the card offers it is the application's check
//! (`orch_app::pick`); here the pick is taken as checked.
//!
//! The rule this file exists for: **the log is the state**, so the pick is recorded on the
//! `user_message` and everything that continues that message carries it — the delegation, the steer,
//! the text a Stop & send holds, and a redelivery. A message that names none writes what it always
//! wrote, and a log written before the field existed reads back with none.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;
use serde_json::json;

use ThreadState::{Blocked, Done, Queued, Working};

fn user() -> UserId {
    UserId::new("me@example.com")
}

fn deep() -> ModelPick {
    ModelPick {
        model: Some(ModelId::parse("deep").unwrap()),
        reasoning_effort: Some(ReasoningEffort::High),
    }
}

fn fast() -> ModelPick {
    ModelPick::model("fast").unwrap()
}

fn said(text: &str, pick: Option<ModelPick>) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick,
    }
}

fn stop(text: &str, pick: Option<ModelPick>) -> Input {
    Input::StopAndSend {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick,
    }
}

fn step(snap: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snap, input).unwrap()
}

fn logged(cmds: &[Command]) -> &UserMessageData {
    cmds.iter()
        .find_map(|c| match c {
            Command::Append(EventDraft {
                body: EventBody::UserMessage(m),
                ..
            }) => Some(m),
            _ => None,
        })
        .expect("a user_message")
}

/// The pick of the one delegation or steer in `cmds` (`None` when it names none).
fn sent(cmds: &[Command]) -> Option<ModelPick> {
    cmds.iter()
        .find_map(|c| match c {
            Command::Delegate { pick, .. } | Command::Steer { pick, .. } => Some(pick.clone()),
            _ => None,
        })
        .expect("a delegation or a steer")
}

#[test]
fn the_message_that_names_a_pick_records_it_and_carries_it() {
    let (snap, cmds) = start_thread(GatePolicy::default(), &said("go deeper", Some(deep())))
        .expect("a first message");
    assert_eq!(sent(&cmds), Some(deep()), "the delegation carries it");
    let event = serde_json::to_value(logged(&cmds)).unwrap();
    assert_eq!(
        event["pick"],
        json!({"model": "deep", "reasoningEffort": "high"}),
        "{event}"
    );
    let back: UserMessageData = serde_json::from_value(event).unwrap();
    assert_eq!(back.pick, Some(deep()), "and it reads back");
    // a log written before the field existed has none
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old.pick, None);
    assert_eq!(snap.state, Queued);
}

#[test]
fn a_message_that_names_none_writes_the_event_it_always_did() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("go", None)).unwrap();
    assert_eq!(
        serde_json::to_value(logged(&cmds)).unwrap(),
        json!({"text": "go", "messageId": "m-1", "runId": "r-1"}),
        "no `pick` member"
    );
    assert_eq!(sent(&cmds), None);
}

#[test]
fn every_way_a_message_reaches_the_agent_carries_its_own_pick() {
    let (_, cmds) = step(&Snapshot::new(Blocked), &said("yes", Some(fast())));
    assert_eq!(sent(&cmds), Some(fast()));
    let (_, cmds) = step(&Snapshot::new(Done), &said("again", Some(deep())));
    assert_eq!(sent(&cmds), Some(deep()));
    let (_, cmds) = step(&Snapshot::new(Working), &said("and here", Some(fast())));
    assert_eq!(sent(&cmds), Some(fast()));
}

#[test]
fn a_stop_and_send_holds_the_newest_pick_for_the_next_job_and_a_cancel_forgets_it() {
    let (snap, _) = step(&Snapshot::new(Working), &stop("this first", Some(deep())));
    assert_eq!(snap.job.after_stop_pick, Some(deep()));
    // a message joined behind it that names none leaves the held pick as it is
    let (snap, _) = step(&snap, &stop("and that", None));
    assert_eq!(snap.job.after_stop_pick, Some(deep()));
    // a newer pick replaces it
    let (snap, _) = step(&snap, &stop("no, fast", Some(fast())));
    assert_eq!(snap.job.after_stop_pick, Some(fast()));
    // the next job starts with it
    let (_, cmds) = step(&snap, &Input::CancelledBeforeStart);
    assert_eq!(sent(&cmds), Some(fast()));
    // a person who cancels instead takes the pick with the text
    let (snap, _) = step(&Snapshot::new(Working), &stop("x", Some(deep())));
    let (snap, _) = step(&snap, &Input::Cancel { user: user() });
    assert_eq!(snap.job.after_stop_pick, None);
}

#[test]
fn a_redelivery_carries_the_pick_its_message_recorded() {
    let redeliver = |pick| Input::Redeliver {
        text: "go".into(),
        sent: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick,
    };
    let (_, cmds) = step(&Snapshot::new(Done), &redeliver(Some(deep())));
    assert_eq!(sent(&cmds), Some(deep()));
    let (_, cmds) = step(&Snapshot::new(Done), &redeliver(None));
    assert_eq!(sent(&cmds), None);
}

#[test]
fn a_fork_first_message_and_an_edit_carry_the_pick_they_name() {
    let mut edit = Replacement::edit("new words".into(), None, None);
    edit.pick = Some(deep());
    assert_eq!(edit.pick, Some(deep()));
}

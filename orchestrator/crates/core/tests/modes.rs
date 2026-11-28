//! The mode a person picks for a message (ADR 0109, `agent-mode/v1`): what the core records of it
//! and what it carries. Whether the live card offers it is the application's check
//! (`orch_app::mode`); here the mode is taken as checked.
//!
//! The rule this file exists for: **the log is the state**, so the mode is recorded on the
//! `user_message` and everything that continues that message carries it — the delegation, the steer,
//! the text a Stop & send holds, and a redelivery. A message that names none writes what it always
//! wrote, and a log written before the field existed reads back with none. A mode is never carried
//! from one job to the next.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;
use serde_json::json;

use ThreadState::{Blocked, Done, Queued, Working};

fn user() -> UserId {
    UserId::new("me@example.com")
}

fn plan() -> ModeName {
    ModeName::parse("plan").unwrap()
}

fn build() -> ModeName {
    ModeName::parse("build").unwrap()
}

fn said(text: &str, mode: Option<ModeName>) -> Input {
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
        pick: None,
        mode,
        prompt_signature: None,
    }
}

fn stop(text: &str, mode: Option<ModeName>) -> Input {
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
        pick: None,
        mode,
        prompt_signature: None,
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

/// The mode of the one delegation or steer in `cmds` (`None` when it names none).
fn sent(cmds: &[Command]) -> Option<ModeName> {
    cmds.iter()
        .find_map(|c| match c {
            Command::Delegate { mode, .. } | Command::Steer { mode, .. } => Some(mode.clone()),
            _ => None,
        })
        .expect("a delegation or a steer")
}

#[test]
fn the_message_that_names_a_mode_records_it_and_carries_it() {
    let (snap, cmds) = start_thread(GatePolicy::default(), &said("think first", Some(plan())))
        .expect("a first message");
    assert_eq!(sent(&cmds), Some(plan()), "the delegation carries it");
    let event = serde_json::to_value(logged(&cmds)).unwrap();
    assert_eq!(event["mode"], json!("plan"), "{event}");
    let back: UserMessageData = serde_json::from_value(event).unwrap();
    assert_eq!(back.mode, Some(plan()), "and it reads back");
    // a log written before the field existed has none
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old.mode, None);
    assert_eq!(snap.state, Queued);
}

#[test]
fn a_message_that_names_none_writes_no_member_and_sends_none() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("go", None)).unwrap();
    assert_eq!(
        serde_json::to_value(logged(&cmds)).unwrap(),
        json!({"text": "go", "messageId": "m-1", "runId": "r-1"}),
        "no `mode` member"
    );
    assert_eq!(sent(&cmds), None);
}

#[test]
fn every_way_a_message_reaches_the_agent_carries_its_own_mode() {
    let (_, cmds) = step(&Snapshot::new(Blocked), &said("yes", Some(plan())));
    assert_eq!(sent(&cmds), Some(plan()));
    let (_, cmds) = step(&Snapshot::new(Done), &said("again", Some(build())));
    assert_eq!(sent(&cmds), Some(build()));
    let (_, cmds) = step(&Snapshot::new(Working), &said("and here", Some(plan())));
    assert_eq!(sent(&cmds), Some(plan()));
}

#[test]
fn a_steer_that_names_none_keeps_the_jobs_mode_and_one_that_names_one_replaces_it() {
    let mut working = Snapshot::new(Working);
    working.job.mode = Some(plan());
    // a message sent while the task runs names none: the task keeps its mode
    let (snap, cmds) = step(&working, &said("a thought", None));
    assert_eq!(sent(&cmds), None, "nothing new is sent for it");
    assert_eq!(snap.job.mode, Some(plan()), "the task keeps its mode");
    // one that names a mode holds it from the next step
    let (snap, cmds) = step(&snap, &said("switch", Some(build())));
    assert_eq!(sent(&cmds), Some(build()));
    assert_eq!(snap.job.mode, Some(build()));
}

#[test]
fn a_stop_and_send_holds_the_newest_mode_for_the_next_job_and_a_cancel_forgets_it() {
    let (snap, _) = step(&Snapshot::new(Working), &stop("this first", Some(plan())));
    assert_eq!(snap.job.after_stop_mode, Some(plan()));
    // a message joined behind it that names none leaves the held mode as it is
    let (snap, _) = step(&snap, &stop("and that", None));
    assert_eq!(snap.job.after_stop_mode, Some(plan()));
    // a newer mode replaces it
    let (snap, _) = step(&snap, &stop("no, build", Some(build())));
    assert_eq!(snap.job.after_stop_mode, Some(build()));
    // the next job starts with it
    let (snap, cmds) = step(&snap, &Input::CancelledBeforeStart);
    assert_eq!(sent(&cmds), Some(build()));
    assert_eq!(snap.job.mode, Some(build()));
    // a person who cancels instead takes the mode with the text
    let (snap, _) = step(&Snapshot::new(Working), &stop("x", Some(plan())));
    let (snap, _) = step(&snap, &Input::Cancel { user: user() });
    assert_eq!(snap.job.after_stop_mode, None);
}

#[test]
fn a_redelivery_carries_the_mode_its_message_recorded() {
    let redeliver = |mode| Input::Redeliver {
        text: "go".into(),
        sent: false,
        mentions: Vec::new(),
        tier: None,
        tier_cap: None,
        locale: None,
        pick: None,
        mode,
        prompt_signature: None,
    };
    let (_, cmds) = step(&Snapshot::new(Done), &redeliver(Some(plan())));
    assert_eq!(sent(&cmds), Some(plan()));
    let (_, cmds) = step(&Snapshot::new(Done), &redeliver(None));
    assert_eq!(sent(&cmds), None);
}

#[test]
fn a_mode_is_never_carried_from_one_job_to_the_next() {
    // a job worked under `plan` ends; the message that starts the next job names none, and the
    // orchestrator sends none: the agent's own default is never sent either.
    let mut done = Snapshot::new(Done);
    done.job.mode = Some(plan());
    let (next, cmds) = step(&done, &said("next job", None));
    assert_eq!(sent(&cmds), None);
    assert_eq!(next.job.mode, None, "the next job starts with none");
}

#[test]
fn an_edit_carries_the_mode_it_names() {
    let mut edit = Replacement::edit("new words".into(), None, None);
    edit.mode = Some(plan());
    assert_eq!(edit.mode, Some(plan()));
}

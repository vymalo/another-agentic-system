//! The interface locale of a message (ADR 0085, `locale/v1`): what the core records of it and what
//! it carries. The checks of the tag that arrives (its shape) are the application's
//! (`orch_app::locale`, `orch_core::LocaleHint::parse`); here they are taken as made.
//!
//! The rule this file exists for: **the log is the state**, so the hint is recorded on the
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

fn tag(text: &str) -> LocaleHint {
    LocaleHint::parse(text).expect("a well-formed tag")
}

fn said(text: &str, locale: Option<&str>) -> Input {
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
        locale: locale.map(tag),
        pick: None,
    }
}

fn stop(text: &str, locale: Option<&str>) -> Input {
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
        locale: locale.map(tag),
        pick: None,
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

/// The tag of the one delegation or steer in `cmds` (`None` when it names none).
fn sent(cmds: &[Command]) -> Option<LocaleHint> {
    cmds.iter()
        .find_map(|c| match c {
            Command::Delegate { locale, .. } | Command::Steer { locale, .. } => {
                Some(locale.clone())
            }
            _ => None,
        })
        .expect("a delegation or a steer")
}

#[test]
fn the_message_that_names_a_locale_records_it_and_carries_it() {
    let (snap, cmds) = start_thread(
        GatePolicy::default(),
        &said("corrige la page", Some("fr-FR")),
    )
    .expect("a first message");
    assert_eq!(sent(&cmds), Some(tag("fr-FR")), "the delegation carries it");
    // the log is the state: the event holds the tag, so an export shows what the agent was told
    let event = serde_json::to_value(logged(&cmds)).unwrap();
    assert_eq!(event["locale"], "fr-FR", "{event}");
    let back: UserMessageData = serde_json::from_value(event).unwrap();
    assert_eq!(back.locale, Some(tag("fr-FR")), "and it reads back");
    // a log written before the field existed has none
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old.locale, None);
    assert_eq!(snap.state, Queued);
}

#[test]
fn a_message_that_names_none_writes_the_event_it_always_did() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("corrige la page", None)).unwrap();
    assert_eq!(
        serde_json::to_value(logged(&cmds)).unwrap(),
        json!({"text": "corrige la page", "messageId": "m-1", "runId": "r-1"}),
        "no `locale` member"
    );
    assert_eq!(sent(&cmds), None);
}

#[test]
fn every_way_a_message_reaches_the_agent_carries_its_own_locale() {
    // blocked: an answer re-delegates
    let (_, cmds) = step(&Snapshot::new(Blocked), &said("oui", Some("fr-FR")));
    assert_eq!(sent(&cmds), Some(tag("fr-FR")));
    // done: the next job starts with it
    let (_, cmds) = step(&Snapshot::new(Done), &said("again", Some("fur-IT")));
    assert_eq!(sent(&cmds), Some(tag("fur-IT")));
    // working: the message is steered into the running task, with its own tag
    let (_, cmds) = step(&Snapshot::new(Working), &said("et ici", Some("gsw-CH")));
    assert_eq!(sent(&cmds), Some(tag("gsw-CH")));
}

#[test]
fn a_stop_and_send_holds_the_newest_locale_for_the_next_job_and_a_cancel_forgets_it() {
    // the message that stops a running job holds its tag, and the next job's delegation carries it
    let (snap, cmds) = step(
        &Snapshot::new(Working),
        &stop("d'abord ceci", Some("fr-FR")),
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::RequestCancel { .. }))
    );
    assert_eq!(snap.job.after_stop_locale, Some(tag("fr-FR")));
    // a second message joined behind it that names none leaves the held one as it is
    let (snap, _) = step(&snap, &stop("et cela", None));
    assert_eq!(snap.job.after_stop_locale, Some(tag("fr-FR")));
    // the next job starts with it
    let (_, cmds) = step(&snap, &Input::CancelledBeforeStart);
    assert_eq!(sent(&cmds), Some(tag("fr-FR")));
    // a person who cancels instead takes the hint with the text
    let (snap, _) = step(&Snapshot::new(Working), &stop("jamais", Some("fr-FR")));
    let (snap, _) = step(&snap, &Input::Cancel { user: user() });
    assert_eq!(snap.job.after_stop_locale, None);
}

#[test]
fn a_redelivery_carries_the_locale_its_message_recorded() {
    // The case that was silently wrong before the ruling: the message is in the log already, so the
    // redelivery writes no `user_message` — the hint comes from what the dispatcher read on its row,
    // which is what the `user_message` holds, and the delegation must carry it.
    let (_, cmds) = step(
        &Snapshot::new(Done),
        &Input::Redeliver {
            text: "corrige la page".into(),
            sent: false,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: Some(tag("fr-FR")),
            pick: None,
        },
    );
    assert_eq!(sent(&cmds), Some(tag("fr-FR")), "the delegation carries it");
    // no event is written for it: the message is in the log already
    assert!(
        !cmds.iter().any(|c| matches!(
            c,
            Command::Append(EventDraft {
                body: EventBody::UserMessage(_),
                ..
            })
        )),
        "{cmds:?}"
    );
    // and a redelivery whose message named none carries none
    let (_, cmds) = step(
        &Snapshot::new(Done),
        &Input::Redeliver {
            text: "hi".into(),
            sent: false,
            mentions: Vec::new(),
            tier: None,
            tier_cap: None,
            locale: None,
            pick: None,
        },
    );
    assert_eq!(sent(&cmds), None);
}

#[test]
fn an_action_carries_no_locale() {
    // an action is a gesture on a surface, not a message of a screen with a language: the payload
    // the application writes for it has no member, and the command has none to give
    let (_, cmds) = step(
        &Snapshot::new(Blocked),
        &Input::UiAction {
            user: user(),
            action: UiActionData {
                surface_id: "s-1".into(),
                name: "go".into(),
                source_component_id: "b-1".into(),
                context: serde_json::Map::new(),
                version: UiVersion::V1_0,
                run_id: None,
                tier: None,
                tier_cap: None,
            },
            catalog: None,
            tier: None,
            tier_cap: None,
        },
    );
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::DelegateAction { .. }))
    );
    assert!(!cmds.iter().any(|c| matches!(
        c,
        Command::Delegate {
            locale: Some(_),
            pick: None,
            ..
        } | Command::Steer {
            locale: Some(_),
            pick: None,
            ..
        }
    )));
}

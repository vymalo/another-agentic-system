//! The cap of a person's role on an agent (ADR 0061, `max_tier`) when no tier could be decided
//! for a message because the agent's card offered none then. The application pins the cap beside
//! the tier (`orch_app::tier`); the core keeps it on the log, the job and every command that sends
//! on the message's behalf, so that the dispatcher can decide again from the live card each time it
//! sends. One test per place the tier travels: the message, a steer, a rework, a Stop & send, a
//! cancel the agent refused, a redelivery, an action on a surface, and an ask of a mentioned agent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;
use serde_json::json;

use AccessTier::{Auto, Bypass, Edits, Plan};
use ThreadState::{Blocked, Queued, Working};

fn user() -> UserId {
    UserId::new("me@example.com")
}
fn agent(id: &str) -> AgentId {
    AgentId::new(id)
}

/// A message the application accepted with `tier` and `cap` (see `orch_app::tier::TierPin`).
fn said(text: &str, tier: Option<AccessTier>, cap: Option<AccessTier>) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier,
        tier_cap: cap,
    }
}
fn stop(text: &str, tier: Option<AccessTier>, cap: Option<AccessTier>) -> Input {
    Input::StopAndSend {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier,
        tier_cap: cap,
    }
}
fn status(state: AgentTaskState) -> Input {
    Input::Agent {
        agent: agent("coder"),
        revision: None,
        update: AgentUpdate::Status {
            state,
            detail: None,
            tier: None,
        },
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
/// The tier and the cap of the one delegation or steer in `cmds`.
fn sent(cmds: &[Command]) -> (Option<AccessTier>, Option<AccessTier>) {
    cmds.iter()
        .find_map(|c| match c {
            Command::Delegate { tier, tier_cap, .. } | Command::Steer { tier, tier_cap, .. } => {
                Some((*tier, *tier_cap))
            }
            _ => None,
        })
        .expect("a delegation or a steer")
}
/// The tier and the cap of the one action delegation in `cmds`.
fn sent_action(cmds: &[Command]) -> (Option<AccessTier>, Option<AccessTier>) {
    cmds.iter()
        .find_map(|c| match c {
            Command::DelegateAction { tier, tier_cap, .. } => Some((*tier, *tier_cap)),
            _ => None,
        })
        .expect("an action delegation")
}
fn action() -> UiActionData {
    UiActionData {
        surface_id: "s1".into(),
        name: "pick".into(),
        source_component_id: "b1".into(),
        context: serde_json::Map::new(),
        version: UiVersion::V0_9_1,
        run_id: None,
        tier: None,
        tier_cap: None,
    }
}
fn acted(tier: Option<AccessTier>, cap: Option<AccessTier>) -> Input {
    Input::UiAction {
        user: user(),
        action: action(),
        catalog: None,
        tier,
        tier_cap: cap,
    }
}
fn started(cap: AccessTier) -> Snapshot {
    start_thread(GatePolicy::default(), &said("go", None, Some(cap)))
        .unwrap()
        .0
}

#[test]
fn a_message_with_a_cap_and_no_tier_logs_the_cap_and_delegates_under_it() {
    let (snap, cmds) = start_thread(GatePolicy::default(), &said("go", None, Some(Edits))).unwrap();
    let event = logged(&cmds);
    assert_eq!((event.tier, event.tier_cap), (None, Some(Edits)));
    assert_eq!(
        sent(&cmds),
        (None, Some(Edits)),
        "the delegation carries it"
    );
    assert_eq!(
        (snap.job.tier, snap.job.tier_cap),
        (None, Some(Edits)),
        "and so does the job"
    );
}

#[test]
fn the_cap_is_a_member_of_the_event_only_when_it_is_set() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("go", None, Some(Plan))).unwrap();
    let event = serde_json::to_value(logged(&cmds)).unwrap();
    assert_eq!(event["tierCap"], "plan");
    assert!(event.get("tier").is_none());
    let back: UserMessageData = serde_json::from_value(event).unwrap();
    assert_eq!(back.tier_cap, Some(Plan));
    // a message that has a tier, or a person with no cap: no member, and an old log reads as none
    let (_, cmds) = start_thread(GatePolicy::default(), &said("go", Some(Plan), None)).unwrap();
    assert!(
        serde_json::to_value(logged(&cmds))
            .unwrap()
            .get("tierCap")
            .is_none()
    );
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old.tier_cap, None);
}

#[test]
fn a_message_sent_while_the_job_runs_is_steered_with_its_cap() {
    let working = Snapshot {
        state: Working,
        ..started(Plan)
    };
    let (snap, cmds) = step(&working, &said("faster", None, Some(Edits)));
    assert_eq!(sent(&cmds), (None, Some(Edits)));
    assert!(matches!(cmds.last(), Some(Command::Steer { .. })));
    assert_eq!((snap.job.tier, snap.job.tier_cap), (None, Some(Edits)));
    // the newest message is the latest word: one that was given a tier clears the cap
    let (snap, _) = step(&snap, &said("again", Some(Plan), None));
    assert_eq!((snap.job.tier, snap.job.tier_cap), (Some(Plan), None));
}

fn failing_checks() -> Vec<Input> {
    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let artifact = |name: &str, data: serde_json::Value| Input::Agent {
        agent: agent("coder"),
        revision: None,
        update: AgentUpdate::Artifact {
            name: name.into(),
            mime_type: Some("application/json".into()),
            uri: None,
            text: Some(data.to_string()),
        },
    };
    vec![
        artifact(
            "branch",
            json!({"repository": "https://github.com/vymalo/repo", "branch": "agent/x", "commit": SHA}),
        ),
        artifact(
            "checks",
            json!({"passed": false, "commit": SHA, "summary": "cargo test", "findings": ["red"]}),
        ),
        status(AgentTaskState::Completed),
    ]
}

#[test]
fn every_rework_of_a_job_with_a_cap_and_no_tier_carries_the_cap() {
    let mut gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    gate.max_attempts = 3;
    let (mut snap, first) = start_thread(gate, &said("go", None, Some(Edits))).unwrap();
    assert_eq!(sent(&first), (None, Some(Edits)));
    let mut sends = Vec::new();
    for _ in 0..2 {
        for input in failing_checks() {
            let (next, cmds) = transition(&snap, &input).unwrap();
            snap = next;
            sends.extend(cmds);
        }
    }
    let reworks: Vec<_> = sends
        .iter()
        .filter_map(|c| match c {
            Command::Delegate { tier, tier_cap, .. } => Some((*tier, *tier_cap)),
            _ => None,
        })
        .collect();
    assert_eq!(
        reworks,
        [(None, Some(Edits)), (None, Some(Edits))],
        "a rework is never sent with neither: an agent that has started to offer tiers would take \
         its own default"
    );
}

#[test]
fn a_stop_and_send_carries_the_cap_of_the_newest_message_to_the_next_job() {
    let working = Snapshot {
        state: Working,
        ..started(Auto)
    };
    let (held, cmds) = step(&working, &stop("one", None, Some(Plan)));
    assert_eq!(logged(&cmds).tier_cap, Some(Plan));
    assert_eq!(held.job.after_stop_tier_cap, Some(Plan));
    // a later message given neither leaves it; one given a tier replaces both
    let (held, _) = step(&held, &said("two", None, None));
    assert_eq!(held.job.after_stop_tier_cap, Some(Plan));
    let (held, _) = step(&held, &stop("three", Some(Edits), None));
    assert_eq!(
        (held.job.after_stop_tier, held.job.after_stop_tier_cap),
        (Some(Edits), None)
    );
    let (held, _) = step(&held, &stop("four", None, Some(Plan)));
    assert_eq!(
        (held.job.after_stop_tier, held.job.after_stop_tier_cap),
        (None, Some(Plan))
    );
    // the task ends: the next job starts with it, and does not keep the held one
    let (next, cmds) = step(&held, &status(AgentTaskState::Canceled));
    assert_eq!(sent(&cmds), (None, Some(Plan)));
    assert_eq!((next.job.tier, next.job.tier_cap), (None, Some(Plan)));
    assert_eq!(next.job.after_stop_tier_cap, None);
}

#[test]
fn a_cancel_forgets_the_cap_a_stop_held() {
    let working = Snapshot {
        state: Working,
        ..started(Auto)
    };
    let (held, _) = step(&working, &stop("x", None, Some(Edits)));
    assert_eq!(held.job.after_stop_tier_cap, Some(Edits));
    let (after, _) = step(&held, &Input::Cancel { user: user() });
    assert_eq!(after.job.after_stop_tier_cap, None);
}

#[test]
fn a_cancel_the_agent_refused_sends_the_held_text_as_a_steer_with_its_cap() {
    let working = Snapshot {
        state: Working,
        ..started(Auto)
    };
    let (held, _) = step(&working, &stop("do X instead", None, Some(Edits)));
    let (after, cmds) = step(
        &held,
        &Input::CancelRejected {
            agent: agent("coder"),
            reason: "cannot".into(),
            retryable: false,
        },
    );
    assert_eq!(sent(&cmds), (None, Some(Edits)));
    assert_eq!(after.job.after_stop_tier_cap, None);
    assert_eq!(
        (after.job.tier, after.job.tier_cap),
        (None, Some(Edits)),
        "the held message is the job's newest, and so is its cap"
    );
}

#[test]
fn a_redelivered_message_carries_the_cap_its_row_held() {
    for state in [Queued, Working, Blocked, ThreadState::Verifying] {
        let (_, cmds) = step(
            &Snapshot::new(state),
            &Input::Redeliver {
                text: "again".into(),
                sent: false,
                mentions: Vec::new(),
                tier: None,
                tier_cap: Some(Plan),
            },
        );
        assert_eq!(sent(&cmds), (None, Some(Plan)), "{state:?}");
    }
    for state in [ThreadState::Done, ThreadState::Failed] {
        let (snap, cmds) = step(
            &Snapshot::new(state),
            &Input::Redeliver {
                text: "again".into(),
                sent: false,
                mentions: Vec::new(),
                tier: None,
                tier_cap: Some(Edits),
            },
        );
        assert_eq!(sent(&cmds), (None, Some(Edits)), "{state:?}");
        assert_eq!(snap.job.tier_cap, Some(Edits), "{state:?}");
    }
}

#[test]
fn a_redelivery_of_a_message_already_sent_gives_the_job_the_cap_it_went_out_under() {
    // nothing is sent again, but what follows (a rework) is not above the cap
    for state in [Queued, Working, Blocked, ThreadState::Done] {
        let (snap, cmds) = step(
            &Snapshot::new(state),
            &Input::Redeliver {
                text: "again".into(),
                sent: true,
                mentions: Vec::new(),
                tier: None,
                tier_cap: Some(Plan),
            },
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::Delegate { .. } | Command::Steer { .. })),
            "{state:?}: sent already"
        );
        assert_eq!(snap.job.tier_cap, Some(Plan), "{state:?}");
    }
}

#[test]
fn an_action_on_a_surface_is_sent_under_the_cap_of_the_job() {
    let blocked = Snapshot {
        state: Blocked,
        ..started(Edits)
    };
    // the application had nothing to add (a person with no cap now, or a thread it read as
    // finished): the job's own stands
    let (after, cmds) = step(&blocked, &acted(None, None));
    assert_eq!(sent_action(&cmds), (None, Some(Edits)));
    assert_eq!(after.job.tier_cap, Some(Edits));
    // the application decided again from the live card and the person's cap now
    let (after, cmds) = step(&blocked, &acted(None, Some(Plan)));
    assert_eq!(sent_action(&cmds), (None, Some(Plan)));
    assert_eq!((after.job.tier, after.job.tier_cap), (None, Some(Plan)));
    let (after, cmds) = step(&blocked, &acted(Some(Plan), None));
    assert_eq!(sent_action(&cmds), (Some(Plan), None));
    assert_eq!((after.job.tier, after.job.tier_cap), (Some(Plan), None));
}

#[test]
fn the_action_is_logged_with_the_tier_and_the_cap_it_went_out_under() {
    fn logged_action(cmds: &[Command]) -> UiActionData {
        cmds.iter()
            .find_map(|c| match c {
                Command::Append(EventDraft {
                    body: EventBody::UiAction(data),
                    ..
                }) => Some(data.clone()),
                _ => None,
            })
            .expect("a ui_action")
    }
    let blocked = Snapshot {
        state: Blocked,
        ..start_thread(GatePolicy::default(), &said("go", Some(Bypass), None))
            .unwrap()
            .0
    };
    let (_, cmds) = step(&blocked, &acted(Some(Edits), None));
    let data = logged_action(&cmds);
    assert_eq!((data.tier, data.tier_cap), (Some(Edits), None));
    assert_eq!(serde_json::to_value(&data).unwrap()["tier"], "edits");
    // what the job carries when the application says nothing is what is logged
    let (_, cmds) = step(&blocked, &acted(None, None));
    assert_eq!(logged_action(&cmds).tier, Some(Bypass));
    // a surface cannot put a tier of its own in the log
    let forged = Input::UiAction {
        user: user(),
        action: UiActionData {
            tier: Some(Bypass),
            tier_cap: Some(Bypass),
            ..action()
        },
        catalog: None,
        tier: None,
        tier_cap: None,
    };
    let capped = Snapshot {
        state: Blocked,
        ..started(Plan)
    };
    let (_, cmds) = step(&capped, &forged);
    let data = logged_action(&cmds);
    assert_eq!((data.tier, data.tier_cap), (None, Some(Plan)));
    // and an old log, or an action that went out under none, has no member
    let old: UiActionData = serde_json::from_value(json!({
        "surfaceId": "s", "name": "n", "sourceComponentId": "c", "context": {}, "version": "v0.9.1"
    }))
    .unwrap();
    assert_eq!((old.tier, old.tier_cap), (None, None));
    let plain = serde_json::to_value(action()).unwrap();
    assert!(plain.get("tier").is_none() && plain.get("tierCap").is_none());
}

fn mention(id: &str, cap: Option<AccessTier>) -> Mention {
    Mention {
        agent_id: agent(id),
        label: format!("@{id}"),
        start: 0,
        end: u32::try_from(id.len() + 1).unwrap(),
        card_url: None,
        tier_cap: cap,
    }
}

fn mentioning(text: &str, mentions: Vec<Mention>) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions,
        tier: None,
        tier_cap: None,
    }
}

fn ask(to: &str) -> Input {
    Input::Ask {
        actor: Actor::agent(&agent("coder"), Some("r1".into())),
        caller: Caller::Main,
        agent: agent(to),
        text: "find the data".into(),
        call_key: None,
        parent_step: None,
        limits: AskLimits::default(),
    }
}

fn asked_cap(cmds: &[Command]) -> Option<AccessTier> {
    cmds.iter()
        .find_map(|c| match c {
            Command::Ask { tier_cap, .. } => Some(*tier_cap),
            _ => None,
        })
        .expect("an ask command")
}

#[test]
fn a_mention_pins_the_cap_on_the_agent_and_its_ask_carries_it() {
    let (snap, _) = start_thread(
        GatePolicy::default(),
        &mentioning(
            "@researcher and @browser",
            vec![mention("researcher", Some(Plan)), mention("browser", None)],
        ),
    )
    .unwrap();
    assert_eq!(snap.job.mentioned.len(), 2);
    assert_eq!(
        snap.job.mentioned_caps,
        [(agent("researcher"), Plan)].into(),
        "an agent no role caps has no entry"
    );
    let working = Snapshot {
        state: Working,
        ..snap
    };
    let (_, cmds) = step(&working, &ask("researcher"));
    assert_eq!(asked_cap(&cmds), Some(Plan));
    let (_, cmds) = step(&working, &ask("browser"));
    assert_eq!(asked_cap(&cmds), None, "no cap, an ask as it always was");
}

#[test]
fn the_lowest_cap_a_message_pinned_for_an_agent_stands_for_the_job() {
    let (snap, _) = start_thread(
        GatePolicy::default(),
        &mentioning("@r", vec![mention("researcher", Some(Edits))]),
    )
    .unwrap();
    let (snap, _) = step(
        &snap,
        &mentioning("@r again", vec![mention("researcher", Some(Plan))]),
    );
    assert_eq!(
        snap.job.mentioned_caps,
        [(agent("researcher"), Plan)].into()
    );
    // a later message that pinned a higher cap, or none, does not lift it
    let (snap, _) = step(
        &snap,
        &mentioning("@r", vec![mention("researcher", Some(Auto))]),
    );
    let (snap, _) = step(&snap, &mentioning("@r", vec![mention("researcher", None)]));
    assert_eq!(
        snap.job.mentioned_caps,
        [(agent("researcher"), Plan)].into()
    );
    // the next job starts with the caps of its own first message only
    let done = Snapshot {
        state: ThreadState::Done,
        ..snap
    };
    let (next, _) = step(&done, &said("fresh", None, None));
    assert!(next.job.mentioned_caps.is_empty() && next.job.mentioned.is_empty());
}

#[test]
fn a_mention_carries_its_cap_in_the_log_and_reads_back_without_one() {
    let with = serde_json::to_value(mention("coder", Some(Edits))).unwrap();
    assert_eq!(with["tierCap"], "edits");
    let without = serde_json::to_value(mention("coder", None)).unwrap();
    assert!(without.get("tierCap").is_none());
    let old: Mention = serde_json::from_value(
        json!({"agentId": "coder", "label": "@coder", "start": 0, "end": 6}),
    )
    .unwrap();
    assert_eq!(old.tier_cap, None);
}

#[test]
fn a_ledger_with_no_cap_is_what_it_always_was() {
    let job = Job::default();
    let json = serde_json::to_string(&job).unwrap();
    assert!(
        !json.contains("tier_cap") && !json.contains("mentioned_caps"),
        "{json}"
    );
    let with = Job {
        tier_cap: Some(Plan),
        after_stop_tier_cap: Some(Edits),
        mentioned_caps: [(agent("coder"), Plan)].into(),
        ..Job::default()
    };
    let read: Job = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(read, with);
    // a job stored before the fields existed reads without them
    let read: Job = serde_json::from_str(&json).unwrap();
    assert_eq!(read, job);
}

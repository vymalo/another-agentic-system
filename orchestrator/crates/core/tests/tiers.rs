//! The access tier of a message (ADR 0061, `access-tier/v1`): what the core keeps of the tier a
//! person picked and what the agent echoes. The checks of the tier that arrives (its shape, the
//! person's cap, the live card) are the application's (`orch_app::tier`); here they are taken as
//! made. One test per rule of the contract's section 7 that the core decides: the event, the
//! delegation, the steer, what a Stop & send carries to the next job, a redelivery, and the echo.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;
use serde_json::json;

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};

fn user() -> UserId {
    UserId::new("me@example.com")
}
fn said(text: &str, tier: Option<AccessTier>) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier,
    }
}
fn stop(text: &str, tier: Option<AccessTier>) -> Input {
    Input::StopAndSend {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier,
    }
}
fn status(state: AgentTaskState, tier: Option<AccessTier>) -> Input {
    Input::Agent {
        agent: AgentId::new("coder"),
        revision: None,
        update: AgentUpdate::Status {
            state,
            detail: None,
            tier,
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
/// The tier of the one delegation or steer in `cmds`.
fn sent(cmds: &[Command]) -> Option<AccessTier> {
    cmds.iter()
        .find_map(|c| match c {
            Command::Delegate { tier, .. } | Command::Steer { tier, .. } => Some(*tier),
            _ => None,
        })
        .expect("a delegation or a steer")
}
fn statuses(cmds: &[Command]) -> Vec<&AgentStatusData> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(EventDraft {
                body: EventBody::AgentStatus(data),
                ..
            }) => Some(data),
            _ => None,
        })
        .collect()
}

use AccessTier::{Auto, Bypass, Edits, Plan};

#[test]
fn the_first_message_records_its_tier_as_sent_and_delegates_it() {
    for tier in AccessTier::ALL {
        let (snap, cmds) = start_thread(GatePolicy::default(), &said("do it", Some(tier))).unwrap();
        assert_eq!(logged(&cmds).tier, Some(tier), "stored as sent");
        assert_eq!(sent(&cmds), Some(tier), "the delegation carries it");
        assert_eq!(snap.state, Queued);
    }
}

#[test]
fn a_message_without_a_tier_writes_the_event_it_always_did() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("hi", None)).unwrap();
    assert_eq!(
        serde_json::to_value(logged(&cmds)).unwrap(),
        json!({"text": "hi", "messageId": "m-1", "runId": "r-1"}),
        "no `tier` member"
    );
    assert_eq!(sent(&cmds), None, "and the delegation names none");
}

#[test]
fn a_tier_is_a_member_of_the_event_and_reads_back() {
    let (_, cmds) = start_thread(GatePolicy::default(), &said("hi", Some(Plan))).unwrap();
    let event = serde_json::to_value(logged(&cmds)).unwrap();
    assert_eq!(event["tier"], "plan");
    let back: UserMessageData = serde_json::from_value(event).unwrap();
    assert_eq!(back.tier, Some(Plan));
    // a log written before the field existed has none
    let old: UserMessageData = serde_json::from_value(json!({"text": "hi"})).unwrap();
    assert_eq!(old.tier, None);
}

#[test]
fn every_way_a_message_reaches_the_agent_carries_its_own_tier() {
    // blocked: an answer re-delegates
    let (_, cmds) = step(&Snapshot::new(Blocked), &said("yes", Some(Edits)));
    assert_eq!(logged(&cmds).tier, Some(Edits));
    assert_eq!(sent(&cmds), Some(Edits));
    // done: the next job starts with it
    for state in [Done, Failed, Cancelled] {
        let (_, cmds) = step(&Snapshot::new(state), &said("again", Some(Auto)));
        assert_eq!(logged(&cmds).tier, Some(Auto), "{state:?}");
        assert_eq!(sent(&cmds), Some(Auto), "{state:?}");
    }
    // verifying
    let (_, cmds) = step(&Snapshot::new(Verifying), &said("wait", Some(Bypass)));
    assert_eq!(sent(&cmds), Some(Bypass));
}

#[test]
fn a_message_sent_while_the_job_runs_is_steered_with_its_own_tier_and_not_the_one_before() {
    let (snap, first) = start_thread(GatePolicy::default(), &said("go", Some(Bypass))).unwrap();
    assert_eq!(sent(&first), Some(Bypass));
    for state in [Queued, Working] {
        let running = Snapshot {
            state,
            ..snap.clone()
        };
        let (_, steered) = step(&running, &said("more", Some(Plan)));
        assert_eq!(logged(&steered).tier, Some(Plan), "{state:?}");
        assert!(matches!(
            steered.last(),
            Some(Command::Steer {
                tier: Some(Plan),
                ..
            })
        ));
        // one that names none is a plain steer: the person picks again for each message
        let (_, plain) = step(&running, &said("more", None));
        assert_eq!(logged(&plain).tier, None, "{state:?}");
        assert_eq!(sent(&plain), None, "{state:?}");
    }
}

#[test]
fn a_stop_and_send_carries_the_newest_tier_to_the_next_job() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Auto))).unwrap();
    let working = Snapshot {
        state: Working,
        ..snap
    };
    let (held, cmds) = step(&working, &stop("one", Some(Plan)));
    assert_eq!(logged(&cmds).tier, Some(Plan), "logged as sent");
    assert_eq!(held.job.after_stop_tier, Some(Plan));
    // a later message that names none leaves it, one that names another replaces it
    let (held, _) = step(&held, &said("two", None));
    assert_eq!(held.job.after_stop_tier, Some(Plan));
    let (held, _) = step(&held, &stop("three", Some(Edits)));
    assert_eq!(held.job.after_stop_tier, Some(Edits));
    let (held, _) = step(&held, &stop("four", None));
    assert_eq!(held.job.after_stop_tier, Some(Edits));
    // the task ends: the next job starts with it
    let (next, cmds) = step(&held, &status(AgentTaskState::Canceled, None));
    let delegate = cmds
        .iter()
        .find_map(|c| match c {
            Command::Delegate { tier, text, .. } => Some((*tier, text.clone())),
            _ => None,
        })
        .expect("the next job's delegation");
    assert_eq!(delegate.0, Some(Edits));
    assert_eq!(delegate.1, "one\n\ntwo\n\nthree\n\nfour");
    assert_eq!(next.job.after_stop_tier, None, "and it is not kept");
    assert_eq!(next.job.after_stop, None);
}

#[test]
fn a_stop_that_names_no_tier_starts_the_next_job_with_none() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Bypass))).unwrap();
    let working = Snapshot {
        state: Working,
        ..snap
    };
    let (held, _) = step(&working, &stop("stop", None));
    assert_eq!(held.job.after_stop_tier, None);
    let (_, cmds) = step(&held, &status(AgentTaskState::Canceled, None));
    assert_eq!(sent(&cmds), None, "never the tier of the job it replaces");
}

#[test]
fn a_cancel_forgets_the_tier_a_stop_held() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", None)).unwrap();
    let working = Snapshot {
        state: Working,
        ..snap
    };
    let (held, _) = step(&working, &stop("x", Some(Bypass)));
    assert_eq!(held.job.after_stop_tier, Some(Bypass));
    let (after, _) = step(&held, &Input::Cancel { user: user() });
    assert_eq!(after.job.after_stop_tier, None);
    assert_eq!(after.job.after_stop, None);
}

#[test]
fn a_cancel_the_agent_refused_sends_the_held_text_as_a_steer_with_its_tier() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", None)).unwrap();
    let working = Snapshot {
        state: Working,
        ..snap
    };
    let (held, _) = step(&working, &stop("do X instead", Some(Edits)));
    let (after, cmds) = step(
        &held,
        &Input::CancelRejected {
            agent: AgentId::new("coder"),
            reason: "cannot".into(),
            retryable: false,
        },
    );
    assert!(matches!(
        cmds.last(),
        Some(Command::Steer {
            tier: Some(Edits),
            ..
        })
    ));
    assert_eq!(after.job.after_stop_tier, None);
    assert_eq!(
        after.job.tier,
        Some(Edits),
        "the held message is the job's newest, and so is its tier: what follows continues it"
    );
}

#[test]
fn a_redelivered_message_carries_the_tier_its_row_held() {
    for state in [Queued, Working, Blocked, Verifying] {
        let (_, cmds) = step(
            &Snapshot::new(state),
            &Input::Redeliver {
                text: "again".into(),
                sent: false,
                mentions: Vec::new(),
                tier: Some(Plan),
            },
        );
        assert_eq!(sent(&cmds), Some(Plan), "{state:?}");
    }
    for state in [Done, Failed] {
        let (_, cmds) = step(
            &Snapshot::new(state),
            &Input::Redeliver {
                text: "again".into(),
                sent: false,
                mentions: Vec::new(),
                tier: Some(Auto),
            },
        );
        assert_eq!(sent(&cmds), Some(Auto), "{state:?}");
    }
}

#[test]
fn a_redelivery_of_a_message_already_sent_gives_the_job_the_tier_it_went_out_under() {
    // the row was sent before the crash: nothing is sent again, but the job is under its tier, so a
    // rework after the redelivery is sent under it
    for state in [Queued, Working, Blocked, Verifying, Done, Failed] {
        let (snap, cmds) = step(
            &Snapshot::new(state),
            &Input::Redeliver {
                text: "again".into(),
                sent: true,
                mentions: Vec::new(),
                tier: Some(Edits),
            },
        );
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::Delegate { .. } | Command::Steer { .. })),
            "{state:?}: sent already"
        );
        assert_eq!(snap.job.tier, Some(Edits), "{state:?}");
    }
    // a message that went out under none is the latest word as well
    let (snap, _) = step(
        &Snapshot {
            job: Job {
                tier: Some(Auto),
                ..Job::default()
            },
            ..Snapshot::new(Working)
        },
        &Input::Redeliver {
            text: "again".into(),
            sent: true,
            mentions: Vec::new(),
            tier: None,
        },
    );
    assert_eq!(snap.job.tier, None);
}

/// The artifacts and the status that make a gate under `agent-checks` fail once.
fn failing_checks() -> Vec<Input> {
    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let artifact = |name: &str, data: serde_json::Value| Input::Agent {
        agent: AgentId::new("coder"),
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
        status(AgentTaskState::Completed, None),
    ]
}

/// What the gate sends the agent when its checks fail, after `first` started the thread.
fn rework_after(first: Input) -> (Snapshot, Vec<Command>, Vec<Command>) {
    let gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    let (mut snap, started) = start_thread(gate, &first).unwrap();
    let mut all = Vec::new();
    for input in failing_checks() {
        let (next, cmds) = transition(&snap, &input).unwrap();
        snap = next;
        all.extend(cmds);
    }
    assert_eq!(snap.state, Queued, "reworked, not done");
    (snap, started, all)
}

#[test]
fn the_rework_after_a_redelivery_of_a_sent_message_is_under_its_tier() {
    let gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    let (snap, _) = start_thread(gate, &said("go", None)).unwrap();
    // the dispatcher died after sending the message: the redelivery says it went out under plan
    let (mut snap, cmds) = step(
        &snap,
        &Input::Redeliver {
            text: "go".into(),
            sent: true,
            mentions: Vec::new(),
            tier: Some(Plan),
        },
    );
    assert!(cmds.is_empty());
    let mut all = Vec::new();
    for input in failing_checks() {
        let (next, cmds) = transition(&snap, &input).unwrap();
        snap = next;
        all.extend(cmds);
    }
    assert_eq!(
        sent(&all),
        Some(Plan),
        "the rework is not at the agent's default"
    );
}

#[test]
fn the_rework_of_a_failed_gate_is_sent_under_the_tier_of_the_job() {
    // the orchestrator's own message on the thread's behalf continues the person's message, so it
    // is sent under the tier that message was (ADR 0061): an agent that works on it at its own
    // default would do more than the person allowed
    for tier in AccessTier::ALL {
        let (snap, first, all) = rework_after(said("go", Some(tier)));
        assert_eq!(sent(&first), Some(tier), "the person's message carries it");
        assert_eq!(
            sent(&all),
            Some(tier),
            "the rework of a job at {tier} carries it"
        );
        assert_eq!(snap.job.tier, Some(tier));
    }
}

#[test]
fn the_rework_of_a_job_that_chose_no_tier_names_none() {
    let (snap, first, all) = rework_after(said("go", None));
    assert_eq!(sent(&first), None);
    assert_eq!(
        sent(&all),
        None,
        "no choice, so the agent's own default applies"
    );
    assert_eq!(snap.job.tier, None);
}

#[test]
fn every_rework_of_a_job_carries_its_tier() {
    // the gate allows three attempts: each rework goes under the tier
    let mut gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    gate.max_attempts = 3;
    let (mut snap, _) = start_thread(gate, &said("go", Some(Edits))).unwrap();
    let mut reworks = Vec::new();
    for _ in 0..2 {
        for input in failing_checks() {
            let (next, cmds) = transition(&snap, &input).unwrap();
            snap = next;
            reworks.extend(cmds);
        }
    }
    let delegated: Vec<_> = reworks
        .iter()
        .filter_map(|c| match c {
            Command::Delegate { tier, .. } => Some(*tier),
            _ => None,
        })
        .collect();
    assert_eq!(delegated, [Some(Edits), Some(Edits)]);
}

#[test]
fn the_newest_message_of_the_person_is_the_tier_of_the_job() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Plan))).unwrap();
    assert_eq!(snap.job.tier, Some(Plan));
    // a steered message sets it, named or not (the web sends the person's pick with every message)
    let (snap, cmds) = step(&snap, &said("faster", Some(Bypass)));
    assert_eq!(sent(&cmds), Some(Bypass));
    assert_eq!(snap.job.tier, Some(Bypass));
    let (snap, cmds) = step(&snap, &said("slower", None));
    assert_eq!(sent(&cmds), None);
    assert_eq!(
        snap.job.tier, None,
        "a message that names none is the latest word"
    );
}

#[test]
fn the_job_a_stop_and_send_starts_is_under_the_tier_of_the_messages_that_stopped_it() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Plan))).unwrap();
    let (snap, _) = step(&snap, &stop("instead", Some(Auto)));
    let (done, cmds) = step(&snap, &status(AgentTaskState::Canceled, None));
    assert_eq!(sent(&cmds), Some(Auto));
    assert_eq!(done.job.tier, Some(Auto), "the next job works under it");
    assert_eq!(done.job.number, 2);
}

#[test]
fn an_action_on_a_surface_is_sent_under_the_tier_of_the_job() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Edits))).unwrap();
    let blocked = Snapshot {
        state: Blocked,
        ..snap
    };
    let (_, cmds) = step(
        &blocked,
        &Input::UiAction {
            user: user(),
            action: UiActionData {
                surface_id: "s1".into(),
                name: "pick".into(),
                source_component_id: "b1".into(),
                context: serde_json::Map::new(),
                version: UiVersion::V0_9_1,
                run_id: None,
            },
            catalog: None,
            tier: None,
        },
    );
    let tiers: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::DelegateAction { tier, .. } => Some(*tier),
            _ => None,
        })
        .collect();
    assert_eq!(tiers, [Some(Edits)]);
}

#[test]
fn an_action_the_application_lowered_is_sent_lowered_and_the_job_continues_under_it() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Bypass))).unwrap();
    let blocked = Snapshot {
        state: Blocked,
        ..snap
    };
    let (after, cmds) = step(
        &blocked,
        &Input::UiAction {
            user: user(),
            action: UiActionData {
                surface_id: "s1".into(),
                name: "pick".into(),
                source_component_id: "b1".into(),
                context: serde_json::Map::new(),
                version: UiVersion::V0_9_1,
                run_id: None,
            },
            catalog: None,
            tier: Some(Edits),
        },
    );
    let tiers: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            Command::DelegateAction { tier, .. } => Some(*tier),
            _ => None,
        })
        .collect();
    assert_eq!(tiers, [Some(Edits)]);
    assert_eq!(
        after.job.tier,
        Some(Edits),
        "a rework after it is not above it either"
    );
}

#[test]
fn a_ledger_that_names_no_tier_is_what_it_always_was() {
    // `tier` of the job is omitted when none, and a job stored before it existed reads without it
    let job = Job::default();
    assert!(!serde_json::to_string(&job).unwrap().contains("tier"));
    let read: Job = serde_json::from_str(&serde_json::to_string(&job).unwrap()).unwrap();
    assert_eq!(read.tier, None);
    let with = Job {
        tier: Some(Edits),
        ..Job::default()
    };
    let read: Job = serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
    assert_eq!(read.tier, Some(Edits));
}

#[test]
fn the_first_status_after_the_echo_records_the_tier_the_agent_says() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Plan))).unwrap();
    let (working, cmds) = step(&snap, &status(AgentTaskState::Working, Some(Plan)));
    assert_eq!(working.state, Working);
    let written = statuses(&cmds);
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].status, AgentStatus::Working);
    assert_eq!(written[0].tier, Some(Plan));
    // as a member of the event
    let json = serde_json::to_value(written[0]).unwrap();
    assert_eq!(json, json!({"status": "working", "tier": "plan"}));
}

#[test]
fn a_status_with_no_echo_writes_the_event_it_always_did() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", None)).unwrap();
    let (_, cmds) = step(&snap, &status(AgentTaskState::Working, None));
    assert_eq!(
        serde_json::to_value(statuses(&cmds)[0]).unwrap(),
        json!({"status": "working"})
    );
}

#[test]
fn a_change_of_tier_is_news_on_a_thread_that_works_already() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Auto))).unwrap();
    let (working, _) = step(&snap, &status(AgentTaskState::Working, Some(Auto)));
    // a repeated `working` with nothing to say writes nothing, as it always did
    let (_, quiet) = step(&working, &status(AgentTaskState::Working, None));
    assert!(statuses(&quiet).is_empty());
    // the same status that says a tier is written, so the log holds what the agent said
    let (_, changed) = step(&working, &status(AgentTaskState::Working, Some(Plan)));
    let written = statuses(&changed);
    assert_eq!(written.len(), 1);
    assert_eq!(written[0].tier, Some(Plan));
}

#[test]
fn the_end_of_a_turn_carries_the_echo_too() {
    for (task, expected) in [
        (AgentTaskState::Completed, AgentStatus::Completed),
        (AgentTaskState::Failed, AgentStatus::Failed),
        (AgentTaskState::InputRequired, AgentStatus::InputRequired),
        (AgentTaskState::Canceled, AgentStatus::Canceled),
    ] {
        let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Edits))).unwrap();
        let working = Snapshot {
            state: Working,
            ..snap
        };
        let (_, cmds) = step(&working, &status(task, Some(Edits)));
        let written = statuses(&cmds);
        assert_eq!(written.len(), 1, "{task:?}");
        assert_eq!(written[0].status, expected);
        assert_eq!(written[0].tier, Some(Edits), "{task:?}");
    }
}

#[test]
fn an_echo_the_adapter_refused_changes_nothing_in_the_core() {
    let (snap, _) = start_thread(GatePolicy::default(), &said("go", Some(Plan))).unwrap();
    let (after, cmds) = step(
        &snap,
        &Input::Agent {
            agent: AgentId::new("coder"),
            revision: None,
            update: AgentUpdate::TierRejected,
        },
    );
    assert_eq!(after, snap);
    assert!(cmds.is_empty());
}

#[test]
fn a_job_ledger_without_a_held_tier_writes_what_it_always_did() {
    let job = Job::default();
    let json = serde_json::to_value(&job).unwrap();
    assert!(json.get("after_stop_tier").is_none());
    // a ledger stored before the field existed reads as it always did
    let old: Job = serde_json::from_value(json).unwrap();
    assert_eq!(old.after_stop_tier, None);
    // and a held tier round-trips
    let held = Job {
        after_stop: Some("x".into()),
        after_stop_tier: Some(Bypass),
        ..Job::default()
    };
    let back: Job = serde_json::from_value(serde_json::to_value(&held).unwrap()).unwrap();
    assert_eq!(back.after_stop_tier, Some(Bypass));
}

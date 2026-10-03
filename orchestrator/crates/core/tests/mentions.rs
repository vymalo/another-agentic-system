//! Mentions (ADR 0026, `mentions/v1`): what the core keeps of a message's references and which
//! agents the job may ask. The checks of a reference that arrives are the application's
//! (`orch_app::mentions`); here they are taken as made. One test per rule of the contract's
//! section 3 that the core decides: the event, the delegation, the job's set, and what a Stop & send
//! carries to the next job.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;

use orch_core::*;
use serde_json::json;

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};

fn user() -> UserId {
    UserId::new("me@example.com")
}
fn agent(id: &str) -> AgentId {
    AgentId::new(id)
}
fn mention(id: &str, label: &str, start: u32, end: u32) -> Mention {
    Mention {
        agent_id: agent(id),
        label: label.into(),
        start,
        end,
        card_url: None,
    }
}
fn set(ids: &[&str]) -> BTreeSet<AgentId> {
    ids.iter().map(|id| agent(id)).collect()
}
fn said_with(text: &str, mentions: Vec<Mention>) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: Some("m-1".into()),
        run_id: Some("r-1".into()),
        origin: Origin::Agui,
        catalog: None,
        mentions,
    }
}
fn stop_with(text: &str, mentions: Vec<Mention>) -> Input {
    Input::StopAndSend {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions,
    }
}
fn agent_status(state: AgentTaskState) -> Input {
    Input::Agent {
        agent: agent("coder"),
        revision: None,
        update: AgentUpdate::Status {
            state,
            detail: None,
        },
    }
}
fn step(snap: &Snapshot, input: &Input) -> (Snapshot, Vec<Command>) {
    transition(snap, input).unwrap()
}
fn running(state: ThreadState) -> Snapshot {
    Snapshot::new(state)
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
/// The mentions of the one delegation or steer in `cmds`.
fn sent(cmds: &[Command]) -> &[Mention] {
    cmds.iter()
        .find_map(|c| match c {
            Command::Delegate { mentions, .. } | Command::Steer { mentions, .. } => {
                Some(mentions.as_slice())
            }
            _ => None,
        })
        .expect("a delegation or a steer")
}

const TEXT: &str = "so @researcher check it, then @coder plot it";

fn both() -> Vec<Mention> {
    vec![
        mention("mock-researcher", "@researcher", 3, 14),
        mention("mock-coder", "@coder", 30, 36),
    ]
}

#[test]
fn the_first_message_records_its_mentions_as_sent_delegates_them_and_opens_the_set() {
    let (snap, cmds) = start_thread(GatePolicy::default(), &said_with(TEXT, both())).unwrap();
    assert_eq!(logged(&cmds).mentions, both(), "stored as sent");
    assert_eq!(sent(&cmds), both(), "the delegation carries them");
    assert_eq!(snap.job.mentioned, set(&["mock-coder", "mock-researcher"]));
    assert_eq!(snap.state, Queued);
}

#[test]
fn a_message_without_mentions_writes_the_event_it_always_did() {
    let (snap, cmds) = start_thread(GatePolicy::default(), &said_with("hi", vec![])).unwrap();
    let event = serde_json::to_value(logged(&cmds)).unwrap();
    assert_eq!(
        event,
        json!({"text": "hi", "messageId": "m-1", "runId": "r-1"})
    );
    assert!(sent(&cmds).is_empty());
    assert!(snap.job.mentioned.is_empty());
}

#[test]
fn the_same_agent_twice_is_in_the_set_once() {
    let twice = vec![
        mention("mock-coder", "@coder", 0, 6),
        mention("mock-coder", "@coder", 10, 16),
    ];
    let (snap, cmds) = start_thread(GatePolicy::default(), &said_with("x", twice.clone())).unwrap();
    assert_eq!(snap.job.mentioned, set(&["mock-coder"]));
    assert_eq!(
        sent(&cmds),
        twice,
        "both stay: they are two places in the text"
    );
}

#[test]
fn an_answer_to_a_blocked_job_adds_to_the_set_of_the_job_it_continues() {
    for state in [Blocked, Verifying] {
        let mut snap = running(state);
        snap.job.mentioned = set(&["mock-coder"]);
        let (next, cmds) = step(
            &snap,
            &said_with(
                "ask @researcher",
                vec![mention("mock-researcher", "@researcher", 4, 15)],
            ),
        );
        assert_eq!(next.job.number, 1, "the same job: {state:?}");
        assert_eq!(next.job.mentioned, set(&["mock-coder", "mock-researcher"]));
        assert_eq!(sent(&cmds).len(), 1);
    }
}

#[test]
fn a_message_on_a_finished_thread_starts_a_job_with_the_mentions_of_its_own_message() {
    for state in [Done, Failed, Cancelled] {
        let mut snap = running(state);
        snap.job.mentioned = set(&["mock-coder"]);
        let (next, cmds) = step(
            &snap,
            &said_with(
                "ask @researcher",
                vec![mention("mock-researcher", "@researcher", 4, 15)],
            ),
        );
        assert_eq!(next.job.number, 2);
        assert_eq!(
            next.job.mentioned,
            set(&["mock-researcher"]),
            "the set belongs to the job: {state:?}"
        );
        assert_eq!(logged(&cmds).mentions.len(), 1);
    }
    // and one that mentions nobody starts a job with an empty set
    let mut snap = running(Done);
    snap.job.mentioned = set(&["mock-coder"]);
    let (next, _) = step(&snap, &said_with("thanks", vec![]));
    assert!(next.job.mentioned.is_empty());
}

#[test]
fn a_message_sent_while_the_job_runs_adds_to_its_set_and_is_steered_with_them() {
    for state in [Queued, Working] {
        let mut snap = running(state);
        snap.job.mentioned = set(&["mock-coder"]);
        let (next, cmds) = step(
            &snap,
            &said_with(
                "and @researcher",
                vec![mention("mock-researcher", "@researcher", 4, 15)],
            ),
        );
        assert_eq!(logged(&cmds).delivery, Some(Delivery::Steer));
        assert_eq!(logged(&cmds).mentions.len(), 1);
        assert_eq!(next.job.mentioned, set(&["mock-coder", "mock-researcher"]));
        assert!(
            matches!(cmds.last(), Some(Command::Steer { mentions, .. }) if mentions.len() == 1),
            "{cmds:?}"
        );
    }
}

#[test]
fn a_stop_and_send_carries_its_mentions_to_the_next_job_and_not_to_the_one_it_stops() {
    let mut snap = running(Working);
    snap.job.mentioned = set(&["mock-coder"]);
    let (held, cmds) = step(
        &snap,
        &stop_with(
            "@researcher first",
            vec![mention("mock-researcher", "@researcher", 0, 11)],
        ),
    );
    assert_eq!(logged(&cmds).delivery, Some(Delivery::Interrupt));
    assert_eq!(logged(&cmds).mentions.len(), 1, "logged as sent");
    assert_eq!(
        held.job.mentioned,
        set(&["mock-coder"]),
        "the job that is being stopped is not told of agents it will never ask"
    );
    assert_eq!(held.job.after_stop_mentions.len(), 1);

    let (next, cmds) = step(&held, &agent_status(AgentTaskState::Canceled));
    assert_eq!(next.job.number, 2);
    assert_eq!(next.job.mentioned, set(&["mock-researcher"]));
    assert!(next.job.after_stop_mentions.is_empty());
    assert_eq!(
        sent(&cmds),
        [mention("mock-researcher", "@researcher", 0, 11)],
        "the delegation of the next job"
    );
}

#[test]
fn messages_joined_behind_a_stop_move_their_mentions_by_what_stands_in_front_in_utf16_units() {
    // the first message ends in an emoji: two UTF-16 units, one scalar value, four UTF-8 bytes
    let first = "go \u{1F680}";
    let (held, _) = step(&running(Working), &stop_with(first, vec![]));
    let (held, _) = step(
        &held,
        &said_with("@coder now", vec![mention("mock-coder", "@coder", 0, 6)]),
    );
    // "go 🚀" is 5 units, then "\n\n" is 2: the second message starts at 7
    assert_eq!(
        held.job.after_stop.as_deref(),
        Some("go \u{1F680}\n\n@coder now")
    );
    assert_eq!(utf16_len(first), 5);
    assert_eq!(
        held.job.after_stop_mentions,
        [mention("mock-coder", "@coder", 7, 13)]
    );
    // a third, with a mention of its own, is moved by the two in front of it
    let (held, _) = step(
        &held,
        &stop_with(
            "then @researcher",
            vec![mention("mock-researcher", "@researcher", 5, 16)],
        ),
    );
    let text = held.job.after_stop.clone().unwrap();
    for m in &held.job.after_stop_mentions {
        // every reference is the label at its offsets, counted the way a JavaScript string counts
        let units: Vec<u16> = text.encode_utf16().collect();
        let at = String::from_utf16(&units[m.start as usize..m.end as usize]).unwrap();
        assert_eq!(at, m.label, "{m:?} in {text:?}");
    }
    let (next, cmds) = step(&held, &agent_status(AgentTaskState::Canceled));
    assert_eq!(next.job.mentioned, set(&["mock-coder", "mock-researcher"]));
    assert_eq!(sent(&cmds), held.job.after_stop_mentions.as_slice());
}

#[test]
fn what_a_stop_holds_keeps_at_most_sixteen_mentions() {
    let mut snap = running(Working);
    let one = |n: usize| {
        mention(
            "mock-coder",
            "@c",
            u32::try_from(n * 3).unwrap(),
            u32::try_from(n * 3 + 2).unwrap(),
        )
    };
    let ten: Vec<Mention> = (0..10).map(one).collect();
    let (held, _) = step(&snap, &stop_with(&"@c ".repeat(10), ten.clone()));
    snap = held;
    let (held, _) = step(&snap, &stop_with(&"@c ".repeat(10), ten));
    assert_eq!(
        held.job.after_stop_mentions.len(),
        MAX_MENTIONS,
        "the first sixteen"
    );
}

#[test]
fn a_cancel_forgets_what_a_stop_held_including_its_mentions() {
    let (held, _) = step(
        &running(Working),
        &stop_with("@coder", vec![mention("mock-coder", "@coder", 0, 6)]),
    );
    let (after, _) = step(&held, &Input::Cancel { user: user() });
    assert!(after.job.after_stop.is_none());
    assert!(after.job.after_stop_mentions.is_empty());
}

#[test]
fn an_agent_that_cannot_be_stopped_gets_the_held_message_and_its_mentions_as_a_steer() {
    let (held, _) = step(
        &running(Working),
        &stop_with("@coder go", vec![mention("mock-coder", "@coder", 0, 6)]),
    );
    let (next, cmds) = step(
        &held,
        &Input::CancelRejected {
            agent: agent("coder"),
            reason: "no".into(),
            retryable: false,
        },
    );
    assert_eq!(next.job.number, 1, "the job goes on");
    assert_eq!(next.job.mentioned, set(&["mock-coder"]));
    assert!(next.job.after_stop_mentions.is_empty());
    assert!(
        cmds.iter().any(|c| matches!(
            c,
            Command::Steer { mentions, .. } if *mentions == [mention("mock-coder", "@coder", 0, 6)]
        )),
        "{cmds:?}"
    );
}

#[test]
fn a_stop_and_send_on_a_thread_nothing_runs_on_is_a_plain_message() {
    let (next, cmds) = step(
        &running(Done),
        &stop_with("@coder", vec![mention("mock-coder", "@coder", 0, 6)]),
    );
    assert_eq!(next.job.number, 2);
    assert_eq!(logged(&cmds).delivery, None);
    assert_eq!(next.job.mentioned, set(&["mock-coder"]));
    assert!(next.job.after_stop_mentions.is_empty());
}

#[test]
fn a_redelivery_carries_the_mentions_into_the_job_it_lands_in() {
    let carried = vec![mention("mock-coder", "@coder", 0, 6)];
    for (state, number) in [
        (Queued, 1),
        (Working, 1),
        (Blocked, 1),
        (Done, 2),
        (Failed, 2),
    ] {
        let (next, cmds) = step(
            &running(state),
            &Input::Redeliver {
                text: "@coder".into(),
                sent: false,
                mentions: carried.clone(),
            },
        );
        assert_eq!(next.job.number, number, "{state:?}");
        assert_eq!(next.job.mentioned, set(&["mock-coder"]), "{state:?}");
        assert_eq!(sent(&cmds), carried);
    }
}

/// ADR 0036 (a message sent while a job runs is a steer) with ADR 0026: the steer carries the
/// references of **its** text, as written, and the one `Steer` the commit holds is the only
/// delivery of them (the `user_message` is logged with `delivery: steer` and the same references).
#[test]
fn a_message_with_mentions_sent_while_a_run_is_open_is_a_steer_that_carries_them() {
    let mentioned = vec![mention("mock-researcher", "@researcher", 4, 15)];
    for state in [Queued, Working] {
        let (next, cmds) = step(
            &running(state),
            &said_with("and @researcher", mentioned.clone()),
        );
        assert_eq!(next.state, state);
        assert_eq!(logged(&cmds).delivery, Some(Delivery::Steer), "{state:?}");
        assert_eq!(logged(&cmds).mentions, mentioned, "{state:?}");
        let steers: Vec<_> = cmds
            .iter()
            .filter(|c| matches!(c, Command::Steer { .. }))
            .collect();
        assert_eq!(steers.len(), 1, "{cmds:?}");
        assert!(
            matches!(
                steers[0],
                Command::Steer { text, mentions, .. }
                    if text == "and @researcher" && *mentions == mentioned
            ),
            "{state:?}: {steers:?}"
        );
        assert!(
            !cmds.iter().any(|c| matches!(c, Command::Delegate { .. })),
            "{state:?}: a steer is not also a delegation"
        );
    }
}

/// Open question 33: a message the agent took as a task of its own while the thread ended is
/// applied as a redelivery that was **sent**: nothing is delegated, and the agents it mentioned
/// are the job's all the same.
#[test]
fn a_redelivery_that_was_sent_delegates_nothing_and_still_opens_the_set() {
    let carried = vec![mention("mock-coder", "@coder", 0, 6)];
    for (state, number) in [(Done, 2), (Failed, 2), (Working, 1), (Blocked, 1)] {
        let (next, cmds) = step(
            &running(state),
            &Input::Redeliver {
                text: "@coder".into(),
                sent: true,
                mentions: carried.clone(),
            },
        );
        assert_eq!(next.job.number, number, "{state:?}");
        assert_eq!(next.job.mentioned, set(&["mock-coder"]), "{state:?}");
        assert!(
            !cmds
                .iter()
                .any(|c| matches!(c, Command::Delegate { .. } | Command::Steer { .. })),
            "{state:?}: {cmds:?}"
        );
    }
}

#[test]
fn the_first_job_of_a_thread_the_rework_and_a_fork_mention_nobody() {
    // the gate's rework is the core's own words
    let gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    let (snap, _) = start_thread(
        gate,
        &said_with(
            "fix it @coder",
            vec![mention("mock-coder", "@coder", 7, 13)],
        ),
    )
    .unwrap();
    let snap = Snapshot {
        state: Working,
        ..snap
    };
    let (_, cmds) = step(
        &snap,
        &Input::Agent {
            agent: agent("coder"),
            revision: None,
            update: AgentUpdate::Artifact {
                name: "checks".into(),
                mime_type: Some("application/json".into()),
                uri: None,
                text: Some(json!({"passed": false, "findings": ["x"]}).to_string()),
            },
        },
    );
    let (_, cmds2) = step(&snap, &agent_status(AgentTaskState::Completed));
    for cmds in [cmds, cmds2] {
        for c in &cmds {
            if let Command::Delegate { mentions, .. } = c {
                assert!(mentions.is_empty(), "a rework is not the person's message");
            }
        }
    }
}

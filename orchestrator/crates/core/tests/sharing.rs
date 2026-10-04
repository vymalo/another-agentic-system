//! Sharing a thread (ADR 0040): the two events, the pure transitions, and a fork that is private.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::*;
use serde_json::json;
use uuid::Uuid;

const STATES: [ThreadState; 7] = [Queued, Working, Verifying, Blocked, Done, Failed, Cancelled];

fn owner() -> UserId {
    UserId::new("owner@example.com")
}

fn nonce(n: u8) -> ShareNonce {
    ShareNonce::new([n; NONCE_LEN])
}

fn share(level: ShareLevel, n: u8) -> Input {
    Input::Share {
        user: owner(),
        level,
        nonce: nonce(n),
    }
}

fn appended(cmds: &[Command]) -> Vec<(&Actor, &EventBody)> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some((&d.actor, &d.body)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_share_is_valid_in_every_state_and_says_the_digest_not_the_nonce() {
    for state in STATES {
        let before = Snapshot::new(state);
        let (after, cmds) = transition(&before, &share(ShareLevel::Internal, 7)).unwrap();
        assert_eq!(
            after, before,
            "{state:?}: a share touches nothing of the job"
        );
        assert_eq!(
            cmds.len(),
            2,
            "{state:?}: the event and the row, nothing else"
        );
        let events = appended(&cmds);
        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0].0,
            &Actor::user(&owner()),
            "the actor is the owner"
        );
        assert_eq!(
            events[0].1,
            &EventBody::ThreadShared(ThreadSharedData {
                visibility: ShareLevel::Internal,
                nonce_sha256: nonce(7).sha256_hex(),
            })
        );
        assert!(cmds.contains(&Command::SetSharing {
            level: ShareLevel::Internal,
            nonce: nonce(7),
        }));
    }
}

#[test]
fn a_new_link_is_another_thread_shared_with_another_digest() {
    let before = Snapshot::new(Done);
    let digest = |n| {
        let (_, cmds) = transition(&before, &share(ShareLevel::Public, n)).unwrap();
        match appended(&cmds)[0].1 {
            EventBody::ThreadShared(d) => d.nonce_sha256.clone(),
            other => panic!("{other:?}"),
        }
    };
    assert_ne!(digest(1), digest(2));
    assert_eq!(digest(1), digest(1));
}

#[test]
fn an_unshare_is_valid_in_every_state_and_clears_the_row() {
    for state in STATES {
        let before = Snapshot::new(state);
        let (after, cmds) = transition(&before, &Input::Unshare { user: owner() }).unwrap();
        assert_eq!(after, before, "{state:?}");
        assert_eq!(
            appended(&cmds),
            vec![(
                &Actor::user(&owner()),
                &EventBody::ThreadUnshared(ThreadUnsharedData {})
            )],
            "{state:?}"
        );
        assert!(cmds.contains(&Command::ClearSharing), "{state:?}");
        assert_eq!(cmds.len(), 2, "{state:?}");
    }
}

#[test]
fn a_share_touches_nothing_of_a_job_in_flight() {
    let mut gate = GatePolicy::requiring([CheckSource::Verifier]);
    gate.max_attempts = 3;
    let before = Snapshot {
        state: Blocked,
        job: Job {
            number: 2,
            gate,
            attempt: 2,
            task: Some("the task".into()),
            hold: Some(Hold::VerifierFailed),
            ..Job::default()
        },
    };
    for input in [
        share(ShareLevel::Public, 1),
        Input::Unshare { user: owner() },
    ] {
        let (after, _) = transition(&before, &input).unwrap();
        assert_eq!(after, before);
    }
}

#[test]
fn the_input_and_the_command_never_print_the_nonce() {
    let input = share(ShareLevel::Public, 0xab);
    let (_, cmds) = transition(&Snapshot::new(Done), &input).unwrap();
    let printed = format!("{input:?} {cmds:?}");
    assert!(!printed.contains("171"), "{printed}");
    assert!(
        !printed.contains("0xab") && !printed.contains("ab, ab"),
        "{printed}"
    );
    assert!(printed.contains("ShareNonce(..)"));
}

fn event(body: EventBody, actor: Actor) -> Event {
    Event {
        seq: 1,
        thread_id: ThreadId(Uuid::from_u128(1)),
        at: Timestamp::from_second(1_790_000_000).unwrap(),
        actor,
        body,
    }
}

/// The two events' wire shapes (ADR 0040): `thread_shared {visibility, nonce_sha256}` and
/// `thread_unshared {}`, attributed to the owner, with no nonce anywhere.
#[test]
fn the_two_events_have_their_wire_shapes() {
    let digest = nonce(9).sha256_hex();
    let shared = event(
        EventBody::ThreadShared(ThreadSharedData {
            visibility: ShareLevel::Public,
            nonce_sha256: digest.clone(),
        }),
        Actor::user(&owner()),
    );
    assert_eq!(shared.kind(), EventKind::ThreadShared);
    assert_eq!(shared.kind().as_str(), "thread_shared");
    let v = serde_json::to_value(&shared).unwrap();
    assert_eq!(v["kind"], "thread_shared");
    assert_eq!(
        v["data"],
        json!({"visibility": "public", "nonce_sha256": digest})
    );
    assert_eq!(serde_json::from_value::<Event>(v.clone()).unwrap(), shared);
    for bad in [json!("private"), json!("world"), json!(null)] {
        let mut v = v.clone();
        v["data"]["visibility"] = bad;
        assert!(serde_json::from_value::<Event>(v).is_err());
    }

    let unshared = event(
        EventBody::ThreadUnshared(ThreadUnsharedData {}),
        Actor::user(&owner()),
    );
    assert_eq!(unshared.kind().as_str(), "thread_unshared");
    let v = serde_json::to_value(&unshared).unwrap();
    assert_eq!(v["kind"], "thread_unshared");
    assert_eq!(v["data"], json!({}));
    assert_eq!(
        serde_json::from_value::<Event>(v.clone()).unwrap(),
        unshared
    );
    let mut extra = v;
    extra["data"]["nonce"] = json!("x");
    assert!(
        serde_json::from_value::<Event>(extra).is_err(),
        "an unshare carries nothing"
    );
}

fn record(share: Option<ThreadShare>) -> ThreadRecord {
    ThreadRecord {
        id: ThreadId(Uuid::from_u128(1)),
        owner: owner(),
        title: "T".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
        state: Done,
        job: Job::default(),
        version: 1,
        forked_from: None,
        share,
        pinned_at: None,
        archived_at: None,
        rail_parent: None,
        rail_rank: "i".to_owned(),
        last_seq: 0,
        created_at: Timestamp::from_second(1_790_000_000).unwrap(),
        updated_at: Timestamp::from_second(1_790_000_000).unwrap(),
    }
}

#[test]
fn a_thread_is_private_until_it_has_a_share_and_never_serialises_it() {
    assert_eq!(record(None).visibility(), Visibility::Private);
    let shared = record(Some(ThreadShare {
        level: ShareLevel::Internal,
        nonce: nonce(3),
        shared_at: Timestamp::from_second(1_790_000_100).unwrap(),
    }));
    assert_eq!(shared.visibility(), Visibility::Internal);
    // The thread resource is the owner's own view of the thread; the link is said by the
    // application, never by the record: nothing of the share is in its JSON.
    assert_eq!(
        serde_json::to_value(&shared).unwrap(),
        serde_json::to_value(record(None)).unwrap()
    );
}

// ---- a fork is private --------------------------------------------------------------------

fn log_with_sharing() -> Vec<Event> {
    let at = |seq: i64| Timestamp::from_second(1_790_000_000 + seq).unwrap();
    let ev = |seq: i64, actor: Actor, body: EventBody| Event {
        seq,
        thread_id: ThreadId(Uuid::from_u128(1)),
        at: at(seq),
        actor,
        body,
    };
    vec![
        ev(
            1,
            Actor::user(&owner()),
            EventBody::UserMessage(UserMessageData::new("fix the build")),
        ),
        ev(
            2,
            Actor::agent(&AgentId::new("coder"), None),
            EventBody::AgentMessage(AgentMessageData {
                text: "done".into(),
                message_id: "m2".into(),
                is_final: true,
                purpose: None,
                via: None,
            }),
        ),
        ev(
            3,
            Actor::system(),
            EventBody::ThreadState(ThreadStateData { state: Done }),
        ),
        ev(
            4,
            Actor::user(&owner()),
            EventBody::ThreadShared(ThreadSharedData {
                visibility: ShareLevel::Public,
                nonce_sha256: nonce(1).sha256_hex(),
            }),
        ),
        ev(
            5,
            Actor::user(&owner()),
            EventBody::ThreadUnshared(ThreadUnsharedData {}),
        ),
    ]
}

#[test]
fn a_fork_of_a_shared_thread_starts_private_and_its_history_ignores_the_sharing() {
    let log = log_with_sharing();
    let plain: Vec<Event> = log[..3].to_vec();
    // The copy keeps the sharing events as history (same seq, same data: ADR 0029)...
    assert_eq!(copied(&log, 5).len(), 5);
    // ...but they say nothing to the fork's agent...
    assert_eq!(fork_history(&log), fork_history(&plain));
    // ...and the fork's snapshot is the same as one cut before them.
    let title = TitleLedger::default();
    let description = DescriptionLedger::default();
    assert_eq!(
        forked_snapshot(&log, GatePolicy::default(), title, description),
        forked_snapshot(&plain, GatePolicy::default(), title, description)
    );
    // The commit that makes the fork writes the `thread_forked` event and nothing about sharing:
    // the fork's row starts private, with no nonce.
    let data = ThreadForkedData {
        from: ForkSource {
            thread_id: ThreadId(Uuid::from_u128(1)),
            seq: 5,
        },
        kind: ForkKind::Fork,
        title: "T".into(),
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new("coder"),
            release: None,
        },
    };
    let (_, cmds) = fork_commit(
        &owner(),
        data,
        &log,
        GatePolicy::default(),
        title,
        description,
        None,
    )
    .unwrap();
    assert!(
        !cmds
            .iter()
            .any(|c| matches!(c, Command::SetSharing { .. } | Command::ClearSharing)),
        "{cmds:?}"
    );
}

#[test]
fn a_shared_thread_has_no_message_to_replace_at_its_sharing_events() {
    let log = log_with_sharing();
    for seq in [4, 5] {
        assert_eq!(
            fork_cut(&log, Done, ForkPoint::Replace(seq)),
            Err(ForkError::NotAMessage)
        );
    }
}

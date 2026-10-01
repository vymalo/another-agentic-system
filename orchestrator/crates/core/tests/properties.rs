//! Property tests over random input sequences.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

use proptest::prelude::*;

fn arb_task_state() -> impl Strategy<Value = AgentTaskState> {
    prop_oneof![
        Just(AgentTaskState::Submitted),
        Just(AgentTaskState::Working),
        Just(AgentTaskState::InputRequired),
        Just(AgentTaskState::AuthRequired),
        Just(AgentTaskState::Completed),
        Just(AgentTaskState::Failed),
        Just(AgentTaskState::Canceled),
        Just(AgentTaskState::Rejected),
    ]
}

fn arb_input() -> impl Strategy<Value = Input> {
    let detail = proptest::option::of("[a-z ]{0,8}");
    prop_oneof![
        "[a-z]{1,8}".prop_map(|text| Input::UserMessage {
            user: UserId::new("u@x.io"),
            text,
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Agui,
            catalog: None,
        }),
        Just(Input::Cancel {
            user: UserId::new("u@x.io")
        }),
        (arb_task_state(), detail.clone()).prop_map(|(state, detail)| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Status { state, detail }
        }),
        Just(Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Artifact {
                name: "n".into(),
                mime_type: None,
                uri: None,
                text: None
            }
        }),
        Just(Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Message {
                message_id: "m".into(),
                text: "t".into(),
                is_final: true
            }
        }),
        (any::<bool>(), "[a-z]{1,5}")
            .prop_map(|(retryable, reason)| Input::DeliveryFailed { reason, retryable }),
        Just(Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Ui {
                operations: vec![
                    serde_json::json!({"version": "v0.9.1", "deleteSurface": {"surfaceId": "s"}})
                ]
            }
        }),
        "[a-z]{1,8}".prop_map(|reason| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::UiRejected { reason }
        }),
        "[a-z]{1,8}".prop_map(|name| Input::UiAction {
            user: UserId::new("u@x.io"),
            action: UiActionData {
                surface_id: "s".into(),
                name,
                source_component_id: "b".into(),
                context: serde_json::Map::new(),
                version: UiVersion::V0_9_1,
                run_id: None,
            },
            catalog: None,
        }),
        Just(Input::CancelledBeforeStart),
        (any::<bool>(), "[a-z]{1,5}")
            .prop_map(|(retryable, reason)| Input::CancelRejected { reason, retryable }),
        "[a-z]{1,8}".prop_map(|text| Input::Redeliver { text }),
    ]
}

fn complete() -> Input {
    Input::Agent {
        agent: AgentId::new("a"),
        revision: None,
        update: AgentUpdate::Status {
            state: AgentTaskState::Completed,
            detail: None,
        },
    }
}

proptest! {
    #[test]
    fn invariants_hold_over_random_sequences(inputs in proptest::collection::vec(arb_input(), 0..40)) {
        let mut snap = Snapshot::new(ThreadState::Queued);
        for input in inputs {
            let state = snap.state;
            match orch_core::transition(&snap, &input) {
                Ok((next_snap, cmds)) => {
                    let next = next_snap.state;
                    let starts_job = matches!(input, Input::UserMessage { .. } | Input::Redeliver { .. });
                    // (a) a finished job absorbs every input except a message, which starts
                    // the next job (ADR 0020): job n+1, attempt 1, the same gate, the
                    // verification count not reset.
                    if state.is_terminal() {
                        let restarts = starts_job && !(state == ThreadState::Cancelled && matches!(input, Input::Redeliver { .. }));
                        if restarts {
                            prop_assert_eq!(next, ThreadState::Queued);
                            prop_assert_eq!(next_snap.job.number, snap.job.number + 1);
                            prop_assert_eq!(next_snap.job.attempt, 1);
                            prop_assert_eq!(&next_snap.job.gate, &snap.job.gate);
                            prop_assert_eq!(next_snap.job.verification, snap.job.verification);
                            let started = cmds.iter().any(|c| matches!(c,
                                Command::Append(d) if d.body == EventBody::JobStarted(JobStartedData { job: next_snap.job.number })));
                            prop_assert!(started);
                            let delegations = cmds.iter().filter(|c| matches!(c, Command::Delegate { .. })).count();
                            prop_assert_eq!(delegations, 1);
                        } else {
                            prop_assert_eq!(next, state);
                            prop_assert_eq!(&next_snap.job, &snap.job);
                        }
                    } else {
                        // The job number only moves when a job starts from a finished thread.
                        prop_assert_eq!(next_snap.job.number, snap.job.number);
                    }
                    // `job_started` is appended exactly when the number moved.
                    let started = cmds.iter().filter(|c| matches!(c,
                        Command::Append(d) if matches!(d.body, EventBody::JobStarted(_)))).count();
                    prop_assert_eq!(started, usize::from(next_snap.job.number != snap.job.number));
                    // (b) thread_state events name the resulting state and only mark entry.
                    for cmd in &cmds {
                        if let Command::Append(d) = cmd
                            && let EventBody::ThreadState(t) = &d.body
                        {
                            prop_assert_eq!(t.state, next);
                            prop_assert!(next != state, "thread_state without a state change");
                            prop_assert!(matches!(
                                next,
                                ThreadState::Blocked | ThreadState::Done | ThreadState::Failed | ThreadState::Cancelled
                            ));
                        }
                    }
                    // Conversely: entering blocked/done/failed/cancelled always says so.
                    if next != state && !matches!(next, ThreadState::Queued | ThreadState::Working) {
                        let announced = cmds.iter().any(|c| matches!(c,
                            Command::Append(d) if matches!(&d.body, EventBody::ThreadState(t) if t.state == next)));
                        prop_assert!(announced);
                    }
                    snap = next_snap;
                }
                Err(TransitionError::Finished { state: s }) => {
                    // (c) only an action, which belongs to the finished job, is refused on a
                    // finished thread: a message starts the next job.
                    prop_assert!(s.is_terminal() && s == state);
                    let is_action = matches!(input, Input::UiAction { .. });
                    prop_assert!(is_action);
                }
                Err(TransitionError::InvalidInState { state: s, .. }) => {
                    prop_assert!(s.is_terminal() && s == state);
                    let is_agent = matches!(input, Input::Agent { .. });
                    prop_assert!(is_agent);
                }
                Err(other) => prop_assert!(false, "unexpected transition error: {other}"),
            }
        }
        // (d) from every non-terminal state, `Completed` reaches Done.
        if !snap.state.is_terminal() {
            let (next, _) = orch_core::transition(&snap, &complete()).unwrap();
            prop_assert_eq!(next.state, ThreadState::Done);
        }
    }
}

// ---- the UI catalog ledger (ADR 0023) ---------------------------------------------------------

/// Five catalogs: versions 1, 2, 2 (another digest), 3, and a second version 1.
fn catalog_pool() -> Vec<UiCatalogData> {
    let id = "https://agents.vymalo.com/a2ui/catalogs/chat";
    [(1, "a"), (2, "b"), (2, "c"), (3, "d"), (1, "e")]
        .into_iter()
        .map(|(version, tag)| {
            let catalog = serde_json::json!({
                "catalogId": id,
                "components": {"Note": {"type": "object", "title": tag}},
            });
            UiCatalogData {
                catalog_id: id.into(),
                version,
                digest: catalog_digest(&catalog).unwrap(),
                catalog,
            }
        })
        .collect()
}

/// Inputs that may carry one of the pool's catalogs, among the ones that move the thread.
fn arb_catalog_input() -> impl Strategy<Value = Input> {
    let pool = catalog_pool();
    let carried = proptest::option::of((0..pool.len()).prop_map(move |i| pool[i].clone()));
    prop_oneof![
        4 => ("[a-z]{1,8}", carried.clone()).prop_map(|(text, catalog)| Input::UserMessage {
            user: UserId::new("u@x.io"),
            text,
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Agui,
            catalog,
        }),
        2 => carried.prop_map(|catalog| Input::UiAction {
            user: UserId::new("u@x.io"),
            action: UiActionData {
                surface_id: "s".into(),
                name: "go".into(),
                source_component_id: "b".into(),
                context: serde_json::Map::new(),
                version: UiVersion::V0_9_1,
                run_id: None,
            },
            catalog,
        }),
        2 => "[a-z]{1,8}".prop_map(|text| Input::Redeliver { text }),
        3 => arb_task_state().prop_map(|state| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Status { state, detail: None },
        }),
        1 => Just(Input::Cancel { user: UserId::new("u@x.io") }),
    ]
}

proptest! {
    /// Whatever the order the screens' catalogs arrive in: a digest is recorded once per thread,
    /// the current version never goes down, an inline delivery is made only by the commit that
    /// made its catalog current (and a commit that changes the current catalog delivers it
    /// inline), every other delivery names the current catalog, and the events alone rebuild
    /// the ledger, which is what the projection does.
    #[test]
    fn the_catalog_ledger_follows_the_events(inputs in proptest::collection::vec(arb_catalog_input(), 0..40)) {
        let mut snap = Snapshot::new(ThreadState::Queued);
        let mut recorded: Vec<UiCatalogData> = Vec::new();
        for input in inputs {
            let Ok((next, cmds)) = orch_core::transition(&snap, &input) else { continue };

            let events: Vec<&UiCatalogData> = cmds.iter().filter_map(|c| match c {
                Command::Append(d) => match &d.body {
                    EventBody::UiCatalog(data) => Some(data),
                    _ => None,
                },
                _ => None,
            }).collect();
            prop_assert!(events.len() <= 1, "one catalog per input");
            if let Some(event) = events.first() {
                // first in its commit, the person's, and new to the thread
                prop_assert!(matches!(&cmds[0], Command::Append(d)
                    if matches!(d.body, EventBody::UiCatalog(_)) && d.actor.r#type == ActorType::User));
                prop_assert!(!snap.job.catalog.knows(&event.digest));
                prop_assert!(!recorded.iter().any(|r| r.digest == event.digest), "recorded twice");
                recorded.push((*event).clone());
            }

            let before = snap.job.catalog.current().cloned();
            let after = next.job.catalog.current().cloned();
            if let Some(b) = &before {
                let a = after.as_ref().expect("a thread that has a current catalog keeps one");
                prop_assert!(a.version >= b.version, "the current version went down");
            }

            let delivery = cmds.iter().find_map(|c| match c {
                Command::Delegate { catalog, .. } | Command::DelegateAction { catalog, .. } => Some(catalog),
                _ => None,
            });
            if let Some(delivery) = delivery {
                match delivery {
                    Some(UiDelivery::Inline(data)) => {
                        prop_assert_eq!(events.first().map(|e| &e.digest), Some(&data.digest));
                        prop_assert_eq!(after.as_ref(), Some(&data.reference()));
                        prop_assert!(before.as_ref() != Some(&data.reference()), "inline when nothing changed");
                    }
                    Some(UiDelivery::Ref(r)) => {
                        prop_assert_eq!(after.as_ref(), Some(r));
                        prop_assert_eq!(before.as_ref(), after.as_ref(), "a reference when the current catalog changed");
                    }
                    None => prop_assert!(after.is_none(), "a thread with a catalog tells the agent"),
                }
            }
            snap = next;
        }
        let mut replay = UiCatalogLedger::default();
        for data in &recorded {
            replay.observe(&data.reference());
        }
        prop_assert_eq!(replay, snap.job.catalog);
    }
}

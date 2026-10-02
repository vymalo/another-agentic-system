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
                is_final: true,
                purpose: None,
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
        (any::<bool>(), "[a-z]{1,5}").prop_map(|(retryable, reason)| Input::CancelRejected {
            agent: AgentId::new("a"),
            reason,
            retryable
        }),
        "[a-z]{1,8}".prop_map(|text| Input::Redeliver { text }),
        arb_step().prop_map(|report| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Step(report)
        }),
        "[a-z]{1,8}".prop_map(|title| Input::Rename {
            user: UserId::new("u@x.io"),
            title
        }),
        "[a-z ]{0,8}".prop_map(|description| Input::SetDescription {
            user: UserId::new("u@x.io"),
            description
        }),
        (1..4_u32, "[a-z ]{0,8}")
            .prop_map(|(job, description)| Input::Described { job, description }),
        (1..4_u32).prop_map(|job| Input::DescriptionDeclined { job }),
    ]
}

/// A step report out of a small pool of ids, so that steps start, update, end and restart.
fn arb_step() -> impl Strategy<Value = StepReport> {
    let id = || (0..6_u8).prop_map(|n| format!("t/s{n}"));
    (
        id(),
        proptest::option::of(id()),
        prop_oneof![
            Just(StepKind::Subagent),
            Just(StepKind::Tool),
            Just(StepKind::Command),
            Just(StepKind::Message)
        ],
        prop_oneof![
            4 => Just(StepState::Running),
            1 => Just(StepState::Waiting),
            2 => Just(StepState::Completed),
            1 => Just(StepState::Failed),
            1 => Just(StepState::Canceled)
        ],
    )
        .prop_map(|(id, parent, kind, state)| StepReport {
            label: format!("do {id}"),
            id,
            parent,
            kind,
            state,
            icon: None,
            detail: None,
            input: None,
            output: None,
        })
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
                            // a rename and the description's inputs are the ones that change a
                            // finished thread's job: its ledgers
                            let ledger = if matches!(input, Input::Rename { .. }) {
                                next_snap.job.title
                            } else {
                                snap.job.title
                            };
                            let described = if matches!(
                                input,
                                Input::SetDescription { .. }
                                    | Input::Described { .. }
                                    | Input::DescriptionDeclined { .. }
                            ) {
                                next_snap.job.description
                            } else {
                                snap.job.description
                            };
                            prop_assert_eq!(
                                &next_snap.job,
                                &Job { title: ledger, description: described, ..snap.job.clone() }
                            );
                        }
                    } else {
                        // The job number only moves when a job starts from a finished thread.
                        prop_assert_eq!(next_snap.job.number, snap.job.number);
                    }
                    // `job_started` is appended exactly when the number moved.
                    let started = cmds.iter().filter(|c| matches!(c,
                        Command::Append(d) if matches!(d.body, EventBody::JobStarted(_)))).count();
                    prop_assert_eq!(started, usize::from(next_snap.job.number != snap.job.number));
                    // (c) the description is asked for only when a job stops (done, blocked) by a
                    // transition that gets there, at most once per job, and never after a
                    // person's description.
                    let asks = cmds.iter().filter(|c| matches!(c, Command::RequestDescription { .. })).count();
                    prop_assert!(asks <= 1);
                    if asks == 1 {
                        prop_assert!(next != state);
                        prop_assert!(matches!(next, ThreadState::Done | ThreadState::Blocked));
                        prop_assert!(snap.job.description.may_ask(snap.job.number));
                        prop_assert!(snap.job.description.source() != DescriptionSource::User);
                        prop_assert!(!next_snap.job.description.may_ask(next_snap.job.number));
                    }
                    // a person's description is final: nothing but a person changes it
                    if snap.job.description.source() == DescriptionSource::User
                        && !matches!(input, Input::SetDescription { .. })
                    {
                        prop_assert_eq!(next_snap.job.description.source(), DescriptionSource::User);
                        prop_assert!(!cmds.iter().any(|c| matches!(c, Command::SetDescription(_))));
                    }
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

// ---- nested steps (ADR 0025) ------------------------------------------------------------------

/// Inputs that carry steps, from the agent and from the orchestrator, among the ones that move the
/// thread.
fn arb_step_input() -> impl Strategy<Value = Input> {
    prop_oneof![
        8 => arb_step().prop_map(|report| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Step(report),
        }),
        3 => arb_step().prop_map(|report| Input::Step { actor: Actor::system(), report }),
        2 => arb_task_state().prop_map(|state| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Status { state, detail: None },
        }),
        1 => "[a-z]{1,8}".prop_map(|text| Input::UserMessage {
            user: UserId::new("u@x.io"),
            text,
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Agui,
            catalog: None,
        }),
        1 => Just(Input::Cancel { user: UserId::new("u@x.io") }),
    ]
}

proptest! {
    /// Whatever steps arrive and in whatever order, the log of a step is a start, at most
    /// `MAX_STEP_UPDATES` updates and an end (a one-shot step is an end alone); a step starts
    /// only when it is not open and ends only with the path it started with; the ledger says
    /// exactly what the events leave open; steps are only logged while the thread works and
    /// leave it working; and a new job forgets them all.
    #[test]
    fn the_log_of_a_step_is_bounded_and_the_ledger_follows_the_events(
        inputs in proptest::collection::vec(arb_step_input(), 0..80)
    ) {
        use std::collections::BTreeMap;
        let mut snap = Snapshot::new(ThreadState::Queued);
        // what the events say: open id -> (path of its start, updates logged)
        let mut open: BTreeMap<String, (Vec<String>, u8)> = BTreeMap::new();
        for input in inputs {
            let before = snap.clone();
            let Ok((next, cmds)) = orch_core::transition(&snap, &input) else { continue };
            if next.job.number != before.job.number {
                open.clear();
            }
            let logged: Vec<&AgentStepData> = cmds.iter().filter_map(|c| match c {
                Command::Append(d) => match &d.body {
                    EventBody::AgentStep(s) => Some(s),
                    _ => None,
                },
                _ => None,
            }).collect();
            prop_assert!(logged.len() <= 1, "one event per report");
            for step in logged {
                prop_assert!(
                    matches!(before.state, ThreadState::Queued | ThreadState::Working),
                    "a step was logged in {:?}", before.state
                );
                prop_assert_eq!(next.state, ThreadState::Working);
                prop_assert!(step.path.len() <= MAX_STEP_DEPTH);
                prop_assert!(!step.path.contains(&step.id), "a step is not its own ancestor");
                match step.phase {
                    StepPhase::Start => {
                        prop_assert!(!step.state.is_end());
                        prop_assert!(open.insert(step.id.clone(), (step.path.clone(), 0)).is_none(),
                            "started while open");
                    }
                    StepPhase::Update => {
                        prop_assert!(!step.state.is_end());
                        let entry = open.get_mut(&step.id);
                        prop_assert!(entry.is_some(), "an update of a step that is not open");
                        let (path, updates) = entry.unwrap();
                        prop_assert_eq!(&*path, &step.path);
                        *updates += 1;
                        prop_assert!(*updates <= MAX_STEP_UPDATES, "too many updates");
                    }
                    StepPhase::End => {
                        prop_assert!(step.state.is_end());
                        if let Some((path, _)) = open.remove(&step.id) {
                            prop_assert_eq!(&path, &step.path, "the end has the path of the start");
                        }
                    }
                }
            }
            // a task that ended leaves nothing open; a new job starts empty
            if matches!(&input, Input::Agent { update: AgentUpdate::Status { state, .. }, .. } if state.is_terminal()) {
                open.clear();
            }
            let tracked: Vec<&str> = next.job.steps.open_ids().collect();
            let said: Vec<&str> = open.keys().map(String::as_str).collect();
            prop_assert_eq!(tracked, said, "the ledger and the log agree");
            prop_assert!(next.job.steps.open_count() <= MAX_OPEN_STEPS);
            prop_assert!(next.job.steps.started() <= MAX_STEPS_PER_JOB);
            snap = next;
        }
    }
}

// ---- the turn's announced answer (ADR 0031) ----------------------------------------------------

fn arb_turn_input() -> impl Strategy<Value = Input> {
    let purpose = prop_oneof![
        Just(None),
        Just(Some(orch_core::MessagePurpose::Working)),
        Just(Some(orch_core::MessagePurpose::Answer)),
    ];
    prop_oneof![
        4 => ("[a-c]{1,2}", 1u32..3, "j[12]").prop_map(|(text, job, token)| Input::Answer {
            actor: Actor::agent(&AgentId::new("a"), None),
            text,
            job,
            token,
        }),
        6 => ("[a-c]{1,2}", "[a-c]{1,2}", purpose).prop_map(|(id, text, purpose)| Input::Agent {
            agent: AgentId::new("a"),
            revision: None,
            update: AgentUpdate::Message { message_id: id, text, is_final: true, purpose },
        }),
        4 => (arb_task_state(), proptest::option::of("[a-c]{1,2}")).prop_map(|(state, detail)| {
            Input::Agent {
                agent: AgentId::new("a"),
                revision: None,
                update: AgentUpdate::Status { state, detail },
            }
        }),
        2 => "[a-z]{1,8}".prop_map(|text| Input::UserMessage {
            user: UserId::new("u@x.io"),
            text,
            message_id: None,
            run_id: None,
            origin: orch_core::Origin::Agui,
            catalog: None,
        }),
    ]
}

proptest! {
    /// Once a turn has an announced answer, nothing else the agent says in that turn is marked as
    /// the answer except another announcement; an announcement is only ever written while the
    /// thread is `queued` or `working` and moves nothing; and the ledger says a turn has an
    /// announced answer exactly when the turn's log has an announcement in it.
    #[test]
    fn after_an_announced_answer_no_other_words_of_the_turn_are_the_answer(
        inputs in proptest::collection::vec(arb_turn_input(), 0..60)
    ) {
        use orch_core::{AnswerVia, MessagePurpose};
        let mut snap = Snapshot::new(ThreadState::Queued);
        let mut announced_in_turn = false;
        for input in inputs {
            let before = snap.clone();
            let Ok((next, cmds)) = transition(&snap, &input) else { continue };
            let new_turn = matches!(input, Input::UserMessage { .. });
            if new_turn {
                announced_in_turn = false;
            }
            for cmd in &cmds {
                let Command::Append(draft) = cmd else { continue };
                let EventBody::AgentMessage(m) = &draft.body else { continue };
                if m.via == Some(AnswerVia::TurnOutput) {
                    let by_the_tool = matches!(input, Input::Answer { .. });
                    prop_assert!(by_the_tool);
                    let open = matches!(before.state, ThreadState::Queued | ThreadState::Working);
                    prop_assert!(open);
                    prop_assert_eq!(next.state, before.state);
                    prop_assert_eq!(m.purpose, Some(MessagePurpose::Answer));
                    announced_in_turn = true;
                } else if announced_in_turn {
                    prop_assert_eq!(m.purpose, Some(MessagePurpose::Working), "{:?}", m);
                }
            }
            prop_assert_eq!(next.job.answer.is_announced(), announced_in_turn);
            snap = next;
        }
    }
}

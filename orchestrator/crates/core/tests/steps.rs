//! Nested steps (ADR 0025): the decision table of `record_step`, the state gate, the path, the
//! caps, the door (`sanitize`) and the ledger's life in the job.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::*;
use serde_json::json;

fn agent() -> AgentId {
    AgentId::new("coder")
}

fn report(id: &str, parent: Option<&str>, state: StepState) -> StepReport {
    StepReport {
        id: id.into(),
        parent: parent.map(Into::into),
        kind: StepKind::Command,
        label: format!("run {id}"),
        state,
        icon: None,
        detail: None,
    }
}

fn step_input(r: StepReport) -> Input {
    Input::Agent {
        agent: agent(),
        revision: Some("rev-1".into()),
        update: AgentUpdate::Step(r),
    }
}

fn status_input(state: AgentTaskState) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Status {
            state,
            detail: None,
        },
    }
}

/// Feeds `inputs` to the snapshot, one after the other; returns the end and every command.
fn feed(mut snap: Snapshot, inputs: &[Input]) -> (Snapshot, Vec<Command>) {
    let mut all = Vec::new();
    for input in inputs {
        let (next, cmds) = transition(&snap, input).unwrap();
        snap = next;
        all.extend(cmds);
    }
    (snap, all)
}

fn steps(cmds: &[Command]) -> Vec<&AgentStepData> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => match &d.body {
                EventBody::AgentStep(s) => Some(s),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn phases(cmds: &[Command]) -> Vec<StepPhase> {
    steps(cmds).iter().map(|s| s.phase).collect()
}

fn working() -> Snapshot {
    Snapshot::new(Working)
}

// ---- the decision table ---------------------------------------------------------------------

#[test]
fn row1_a_new_step_that_runs_starts_and_is_tracked() {
    for state in [StepState::Running, StepState::Waiting] {
        let (snap, cmds) = feed(working(), &[step_input(report("t/a", None, state))]);
        let [step] = steps(&cmds)[..] else {
            panic!("one event: {cmds:?}")
        };
        assert_eq!(step.phase, StepPhase::Start);
        assert_eq!(step.state, state);
        assert_eq!(step.path, Vec::<String>::new());
        assert!(snap.job.steps.is_open("t/a"));
        assert_eq!(snap.job.steps.started(), 1);
    }
}

#[test]
fn row2_a_new_step_that_is_reported_ended_is_one_event_and_is_not_tracked() {
    for state in [StepState::Completed, StepState::Failed, StepState::Canceled] {
        let (snap, cmds) = feed(working(), &[step_input(report("t/a", None, state))]);
        assert_eq!(phases(&cmds), [StepPhase::End]);
        assert!(!snap.job.steps.is_open("t/a"));
        assert_eq!(snap.job.steps.started(), 1);
    }
}

#[test]
fn row3_updates_of_an_open_step_are_logged_up_to_the_bound_and_then_dropped() {
    let mut inputs = vec![step_input(report("t/a", None, StepState::Running))];
    inputs.extend((0..10).map(|_| step_input(report("t/a", None, StepState::Running))));
    let (snap, cmds) = feed(working(), &inputs);
    let kinds = phases(&cmds);
    assert_eq!(kinds[0], StepPhase::Start);
    assert_eq!(
        kinds.iter().filter(|p| **p == StepPhase::Update).count(),
        usize::from(MAX_STEP_UPDATES)
    );
    assert_eq!(kinds.len(), 1 + usize::from(MAX_STEP_UPDATES));
    assert!(snap.job.steps.is_open("t/a"));
}

#[test]
fn a_dropped_update_changes_nothing_at_all() {
    let mut snap = working();
    for _ in 0..=MAX_STEP_UPDATES {
        let (next, _) =
            transition(&snap, &step_input(report("t/a", None, StepState::Running))).unwrap();
        snap = next;
    }
    // start + MAX updates logged; the next one is dropped
    let (after, cmds) =
        transition(&snap, &step_input(report("t/a", None, StepState::Waiting))).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, snap, "no event, no ledger change, no state change");
}

#[test]
fn row4_the_end_of_an_open_step_is_always_logged_with_the_path_it_started_with() {
    // the cap on updates never swallows the end
    let mut inputs = vec![
        step_input(report("t/p", None, StepState::Running)),
        step_input(report("t/a", Some("t/p"), StepState::Running)),
    ];
    inputs.extend((0..9).map(|_| step_input(report("t/a", Some("t/p"), StepState::Running))));
    // the end names another parent: the path of its start stands
    inputs.push(step_input(report(
        "t/a",
        Some("t/other"),
        StepState::Failed,
    )));
    let (snap, cmds) = feed(working(), &inputs);
    let end = steps(&cmds).pop().unwrap();
    assert_eq!(end.phase, StepPhase::End);
    assert_eq!(end.state, StepState::Failed);
    assert_eq!(end.path, ["t/p"]);
    assert!(!snap.job.steps.is_open("t/a"));
    assert!(snap.job.steps.is_open("t/p"));
}

#[test]
fn a_step_that_started_again_after_its_end_is_a_retry() {
    let (snap, cmds) = feed(
        working(),
        &[
            step_input(report("t/a", None, StepState::Running)),
            step_input(report("t/a", None, StepState::Failed)),
            step_input(report("t/a", None, StepState::Running)),
            step_input(report("t/a", None, StepState::Completed)),
        ],
    );
    assert_eq!(
        phases(&cmds),
        [
            StepPhase::Start,
            StepPhase::End,
            StepPhase::Start,
            StepPhase::End
        ]
    );
    assert_eq!(snap.job.steps.open_count(), 0);
    assert_eq!(snap.job.steps.started(), 2);
}

#[test]
fn the_log_of_one_step_is_bounded_whatever_the_agent_sends() {
    let mut inputs = vec![step_input(report("t/a", None, StepState::Running))];
    inputs.extend((0..500).map(|_| step_input(report("t/a", None, StepState::Running))));
    inputs.push(step_input(report("t/a", None, StepState::Completed)));
    let (_, cmds) = feed(working(), &inputs);
    assert_eq!(steps(&cmds).len(), 2 + usize::from(MAX_STEP_UPDATES));
}

// ---- the path ------------------------------------------------------------------------------

#[test]
fn the_path_of_a_child_is_its_parents_path_and_the_parent() {
    let (_, cmds) = feed(
        working(),
        &[
            step_input(report("t/a", None, StepState::Running)),
            step_input(report("t/b", Some("t/a"), StepState::Running)),
            step_input(report("t/c", Some("t/b"), StepState::Running)),
        ],
    );
    let paths: Vec<_> = steps(&cmds).iter().map(|s| s.path.clone()).collect();
    assert_eq!(paths, [vec![], vec!["t/a"], vec!["t/a", "t/b"]]);
}

#[test]
fn a_parent_that_is_not_open_is_still_the_last_of_the_path() {
    let (_, cmds) = feed(
        working(),
        &[step_input(report(
            "t/b",
            Some("t/gone"),
            StepState::Running,
        ))],
    );
    assert_eq!(steps(&cmds)[0].path, ["t/gone"]);
}

#[test]
fn a_path_keeps_the_nearest_ancestors_when_it_is_too_long() {
    let mut inputs = Vec::new();
    for i in 0..12 {
        let parent = (i > 0).then(|| format!("t/{}", i - 1));
        inputs.push(step_input(report(
            &format!("t/{i}"),
            parent.as_deref(),
            StepState::Running,
        )));
    }
    let (_, cmds) = feed(working(), &inputs);
    let last = steps(&cmds).pop().unwrap();
    assert_eq!(last.path.len(), MAX_STEP_DEPTH);
    assert_eq!(last.path.last().map(String::as_str), Some("t/10"));
    assert_eq!(last.path.first().map(String::as_str), Some("t/3"));
}

#[test]
fn a_parent_chain_that_loops_back_gives_the_step_no_path() {
    // a runs under b (not open yet); then b is reported under a: b's path would hold b itself
    let (_, cmds) = feed(
        working(),
        &[
            step_input(report("t/a", Some("t/b"), StepState::Running)),
            step_input(report("t/b", Some("t/a"), StepState::Running)),
        ],
    );
    let all = steps(&cmds);
    assert_eq!(all[0].path, ["t/b"]);
    assert_eq!(all[1].path, Vec::<String>::new());
}

#[test]
fn a_step_is_never_its_own_parent() {
    let (_, cmds) = feed(
        working(),
        &[step_input(report("t/a", Some("t/a"), StepState::Running))],
    );
    assert_eq!(steps(&cmds)[0].path, Vec::<String>::new());
}

// ---- the state gate ------------------------------------------------------------------------

#[test]
fn a_queued_thread_whose_agent_reports_a_step_is_working_with_no_event_besides_the_step() {
    let (snap, cmds) = feed(
        Snapshot::new(Queued),
        &[step_input(report("t/a", None, StepState::Running))],
    );
    assert_eq!(snap.state, Working);
    assert_eq!(cmds.len(), 1, "no thread_state, no agent_status: {cmds:?}");
}

#[test]
fn a_report_that_is_dropped_does_not_move_a_queued_thread() {
    let mut r = report("", None, StepState::Running); // no usable id
    r.label = "x".into();
    let (snap, cmds) = feed(Snapshot::new(Queued), &[step_input(r)]);
    assert!(cmds.is_empty());
    assert_eq!(snap.state, Queued);
}

#[test]
fn a_step_in_blocked_or_verifying_is_dropped() {
    for s in [Blocked, Verifying] {
        let (snap, cmds) = feed(
            Snapshot::new(s),
            &[step_input(report("t/a", None, StepState::Running))],
        );
        assert!(cmds.is_empty(), "{s:?}");
        assert_eq!(snap, Snapshot::new(s), "{s:?}: nothing changes");
    }
}

#[test]
fn a_step_for_a_finished_thread_is_a_late_update() {
    for s in [Done, Failed, Cancelled] {
        let err = transition(
            &Snapshot::new(s),
            &step_input(report("t/a", None, StepState::Running)),
        )
        .unwrap_err();
        assert!(matches!(err, TransitionError::InvalidInState { state, .. } if state == s));
    }
}

#[test]
fn the_end_of_the_task_closes_the_open_steps_without_an_event() {
    for ending in [
        AgentTaskState::Completed,
        AgentTaskState::Failed,
        AgentTaskState::Canceled,
        AgentTaskState::Rejected,
    ] {
        let (open, _) = feed(
            working(),
            &[
                step_input(report("t/a", None, StepState::Running)),
                step_input(report("t/b", Some("t/a"), StepState::Waiting)),
            ],
        );
        assert_eq!(open.job.steps.open_count(), 2);
        let (snap, cmds) = feed(open, &[status_input(ending)]);
        assert!(steps(&cmds).is_empty(), "{ending:?}");
        assert_eq!(snap.job.steps.open_count(), 0, "{ending:?}");
    }
}

#[test]
fn a_wait_for_the_user_keeps_the_open_steps() {
    let (open, _) = feed(
        working(),
        &[step_input(report("t/a", None, StepState::Waiting))],
    );
    let (snap, _) = feed(open, &[status_input(AgentTaskState::InputRequired)]);
    assert_eq!(snap.state, Blocked);
    assert!(snap.job.steps.is_open("t/a"));
    // and the step may end after the answer
    let (snap, cmds) = feed(
        Snapshot {
            state: Working,
            job: snap.job,
        },
        &[step_input(report("t/a", None, StepState::Completed))],
    );
    assert_eq!(phases(&cmds), [StepPhase::End]);
    assert!(!snap.job.steps.is_open("t/a"));
}

// ---- the caps ------------------------------------------------------------------------------

#[test]
fn at_most_max_open_steps_are_tracked_and_a_new_one_is_dropped() {
    let mut snap = working();
    for i in 0..MAX_OPEN_STEPS {
        snap = transition(
            &snap,
            &step_input(report(&format!("t/{i}"), None, StepState::Running)),
        )
        .unwrap()
        .0;
    }
    assert_eq!(snap.job.steps.open_count(), MAX_OPEN_STEPS);
    let (after, cmds) = transition(
        &snap,
        &step_input(report("t/over", None, StepState::Running)),
    )
    .unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, snap);
    // ending one makes room
    let (snap, _) = transition(
        &snap,
        &step_input(report("t/0", None, StepState::Completed)),
    )
    .unwrap();
    let (_, cmds) = transition(
        &snap,
        &step_input(report("t/over", None, StepState::Running)),
    )
    .unwrap();
    assert_eq!(phases(&cmds), [StepPhase::Start]);
}

#[test]
fn a_job_logs_at_most_max_steps_per_job_starts() {
    let mut snap = working();
    for i in 0..MAX_STEPS_PER_JOB {
        // a one-shot step each: the open set stays empty
        snap = transition(
            &snap,
            &step_input(report(&format!("t/{i}"), None, StepState::Completed)),
        )
        .unwrap()
        .0;
    }
    assert_eq!(snap.job.steps.started(), MAX_STEPS_PER_JOB);
    let (after, cmds) = transition(
        &snap,
        &step_input(report("t/more", None, StepState::Running)),
    )
    .unwrap();
    assert!(cmds.is_empty());
    assert_eq!(after, snap);
    let (_, cmds) = transition(
        &snap,
        &step_input(report("t/more", None, StepState::Completed)),
    )
    .unwrap();
    assert!(
        cmds.is_empty(),
        "an end of a step that never started is dropped as well"
    );
}

#[test]
fn a_step_open_when_the_cap_is_reached_can_still_end() {
    let mut snap = working();
    snap = transition(
        &snap,
        &step_input(report("t/open", None, StepState::Running)),
    )
    .unwrap()
    .0;
    for i in 1..MAX_STEPS_PER_JOB {
        snap = transition(
            &snap,
            &step_input(report(&format!("t/{i}"), None, StepState::Completed)),
        )
        .unwrap()
        .0;
    }
    assert_eq!(snap.job.steps.started(), MAX_STEPS_PER_JOB);
    let (snap, cmds) = transition(
        &snap,
        &step_input(report("t/open", None, StepState::Completed)),
    )
    .unwrap();
    assert_eq!(phases(&cmds), [StepPhase::End]);
    assert!(!snap.job.steps.is_open("t/open"));
}

// ---- the door ------------------------------------------------------------------------------

fn clean(r: &StepReport) -> Option<StepReport> {
    r.sanitize(StepSource::Agent)
}

#[test]
fn sanitize_drops_a_report_without_a_usable_id_or_label() {
    let ok = report("t/a", None, StepState::Running);
    assert!(clean(&ok).is_some());
    for bad_id in ["", &"x".repeat(MAX_STEP_ID_BYTES + 1), "a\nb", "a\u{0}b"] {
        let mut r = ok.clone();
        r.id = bad_id.into();
        assert!(clean(&r).is_none(), "{bad_id:?}");
    }
    assert!(
        clean(&StepReport {
            id: "x".repeat(MAX_STEP_ID_BYTES),
            ..ok.clone()
        })
        .is_some()
    );
    for bad_label in ["", "   ", "\n\t"] {
        let mut r = ok.clone();
        r.label = bad_label.into();
        assert!(clean(&r).is_none(), "{bad_label:?}");
    }
}

#[test]
fn sanitize_makes_the_label_one_line_and_cuts_it() {
    let mut r = report("t/a", None, StepState::Running);
    r.label = "  npm\ttest \n  --watch\u{0} ".into();
    assert_eq!(clean(&r).unwrap().label, "npm test --watch");
    r.label = "é".repeat(MAX_STEP_LABEL_CHARS + 50);
    let cut = clean(&r).unwrap().label;
    assert_eq!(cut.chars().count(), MAX_STEP_LABEL_CHARS);
    assert!(cut.ends_with('\u{2026}'));
    r.label = "é".repeat(MAX_STEP_LABEL_CHARS);
    assert_eq!(
        clean(&r).unwrap().label.chars().count(),
        MAX_STEP_LABEL_CHARS
    );
}

#[test]
fn sanitize_keeps_the_lines_of_a_detail_and_cuts_it() {
    let mut r = report("t/a", None, StepState::Failed);
    r.detail = Some("  1 failed\nin src/lib.rs\u{0}\u{7} ".into());
    assert_eq!(
        clean(&r).unwrap().detail.as_deref(),
        Some("1 failed\nin src/lib.rs")
    );
    r.detail = Some("   ".into());
    assert_eq!(clean(&r).unwrap().detail, None);
    r.detail = Some("x".repeat(MAX_STEP_DETAIL_CHARS + 1));
    let cut = clean(&r).unwrap().detail.unwrap();
    assert_eq!(cut.chars().count(), MAX_STEP_DETAIL_CHARS);
    assert!(cut.ends_with('\u{2026}'));
}

#[test]
fn sanitize_drops_an_icon_outside_the_vocabulary_but_keeps_the_step() {
    let mut r = report("t/a", None, StepState::Running);
    for icon in STEP_ICONS {
        r.icon = Some(icon.into());
        assert_eq!(clean(&r).unwrap().icon.as_deref(), Some(icon));
    }
    for icon in [
        "Agent",
        "rocket",
        "",
        "mcp-server:github",
        "https://x/y.png",
    ] {
        r.icon = Some(icon.into());
        let kept = clean(&r).unwrap();
        assert_eq!(kept.icon, None, "{icon:?}");
        assert_eq!(kept.id, "t/a");
    }
}

#[test]
fn only_the_orchestrator_may_name_an_mcp_server_as_the_icon() {
    let mut r = report("tool-1", None, StepState::Running);
    r.icon = Some("mcp-server:websearch".into());
    assert_eq!(r.sanitize(StepSource::Agent).unwrap().icon, None);
    assert_eq!(
        r.sanitize(StepSource::Orchestrator)
            .unwrap()
            .icon
            .as_deref(),
        Some("mcp-server:websearch")
    );
    for bad in [
        "mcp-server:",
        "mcp-server:a b",
        "mcp-server:a/b",
        &format!("mcp-server:{}", "a".repeat(65)),
    ] {
        r.icon = Some(bad.into());
        assert_eq!(
            r.sanitize(StepSource::Orchestrator).unwrap().icon,
            None,
            "{bad}"
        );
    }
}

#[test]
fn sanitize_drops_an_unusable_parent_and_keeps_the_step() {
    let mut r = report("t/a", Some("t/p"), StepState::Running);
    r.parent = Some("x".repeat(MAX_STEP_ID_BYTES + 1));
    assert_eq!(clean(&r).unwrap().parent, None);
    r.parent = Some(String::new());
    assert_eq!(clean(&r).unwrap().parent, None);
    r.parent = Some("t/a".into());
    assert_eq!(clean(&r).unwrap().parent, None);
}

#[test]
fn an_agent_icon_from_an_unchecked_adapter_never_reaches_the_log() {
    let mut r = report("t/a", None, StepState::Running);
    r.icon = Some("javascript:alert(1)".into());
    r.label = "line\nbreak".into();
    let (_, cmds) = feed(working(), &[step_input(r)]);
    let step = steps(&cmds)[0];
    assert_eq!(step.icon, None);
    assert_eq!(step.label, "line break");
}

// ---- the orchestrator's own steps ------------------------------------------------------------

#[test]
fn an_input_step_goes_through_the_same_rules_by_its_own_actor() {
    let mut r = report("tool-1", None, StepState::Running);
    r.kind = StepKind::Tool;
    r.icon = Some("mcp-server:websearch".into());
    let input = |r: StepReport| Input::Step {
        actor: Actor::agent(&agent(), None),
        report: r,
    };
    let (snap, cmds) = feed(working(), &[input(r.clone())]);
    let [Command::Append(d)] = &cmds[..] else {
        panic!("{cmds:?}")
    };
    assert_eq!(d.actor, Actor::agent(&agent(), None));
    assert!(matches!(&d.body, EventBody::AgentStep(s)
        if s.icon.as_deref() == Some("mcp-server:websearch") && s.phase == StepPhase::Start));
    assert!(snap.job.steps.is_open("tool-1"));
    // the end, coalesced like any other
    r.state = StepState::Completed;
    let (snap, cmds) = feed(snap, &[input(r)]);
    assert_eq!(phases(&cmds), [StepPhase::End]);
    assert!(!snap.job.steps.is_open("tool-1"));
}

#[test]
fn an_input_step_for_a_finished_thread_is_refused_and_in_blocked_it_is_dropped() {
    let input = Input::Step {
        actor: Actor::system(),
        report: report("x", None, StepState::Running),
    };
    for s in [Done, Failed, Cancelled] {
        assert!(matches!(
            transition(&Snapshot::new(s), &input),
            Err(TransitionError::InvalidInState { .. })
        ));
    }
    let (snap, cmds) = transition(&Snapshot::new(Blocked), &input).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(snap, Snapshot::new(Blocked));
}

// ---- the ledger in the job -----------------------------------------------------------------

#[test]
fn the_next_job_starts_with_no_steps() {
    let (snap, _) = feed(
        working(),
        &[
            step_input(report("t/a", None, StepState::Running)),
            step_input(report("t/b", None, StepState::Completed)),
        ],
    );
    assert!(!snap.job.steps.is_empty());
    assert!(snap.job.next().steps.is_empty());
}

#[test]
fn a_ledger_stored_before_steps_existed_reads_as_one_with_none() {
    let old = json!({"number": 2, "attempt": 1});
    let job: Job = serde_json::from_value(old).unwrap();
    assert!(job.steps.is_empty());
    // and an empty one is not written
    assert!(
        serde_json::to_value(Job::default())
            .unwrap()
            .get("steps")
            .is_none()
    );
}

#[test]
fn the_ledger_round_trips_through_its_json() {
    let (snap, _) = feed(
        working(),
        &[
            step_input(report("t/a", None, StepState::Running)),
            step_input(report("t/b", Some("t/a"), StepState::Running)),
            step_input(report("t/b", Some("t/a"), StepState::Running)),
        ],
    );
    let value = serde_json::to_value(&snap.job).unwrap();
    assert_eq!(
        value["steps"],
        json!({"open": {
            "t/a": {"path": [], "updates": 0},
            "t/b": {"path": ["t/a"], "updates": 1}
        }, "started": 2})
    );
    let back: Job = serde_json::from_value(value).unwrap();
    assert_eq!(back, snap.job);
    // a replica that reads the ledger back decides as the first one did
    let more = step_input(report("t/b", Some("t/a"), StepState::Completed));
    assert_eq!(
        transition(&snap, &more).unwrap(),
        transition(
            &Snapshot {
                state: snap.state,
                job: back
            },
            &more
        )
        .unwrap()
    );
}

#[test]
fn a_step_does_not_disturb_the_gates_ledger() {
    let gate = GatePolicy::requiring([CheckSource::AgentChecks]);
    let before = Snapshot {
        state: Working,
        job: Job::with_gate(gate),
    };
    let (after, _) = feed(
        before.clone(),
        &[step_input(report("t/a", None, StepState::Running))],
    );
    assert_eq!(after.job.gate, before.job.gate);
    assert_eq!(after.job.attempt, before.job.attempt);
    assert_eq!(after.job.pushed, before.job.pushed);
    assert_eq!(after.job.results, before.job.results);
}

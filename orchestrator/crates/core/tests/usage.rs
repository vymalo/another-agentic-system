//! Token usage (`usage/v1`, ADR 0056): a call report is one `model_usage` of the agent, with the path
//! of the step it ran under; a task's totals are one `model_usage_total`; an asked agent's are under
//! its ask; a job logs at most `MAX_USAGE_CALLS_PER_JOB` calls; nothing moves the thread's state.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::*;

fn agent() -> AgentId {
    AgentId::new("coder")
}

fn tokens(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

fn call(call: &str, step: Option<&str>) -> UsageCall {
    UsageCall {
        task: "t1".into(),
        call: call.into(),
        step: step.map(Into::into),
        provider: Some("openai".into()),
        model: "glm-5.3".into(),
        tokens: tokens(41_250, 812),
        context_window: Some(131_072),
    }
}

fn totals() -> UsageTotals {
    UsageTotals {
        task: "t1".into(),
        totals: vec![ModelTokens {
            provider: Some("openai".into()),
            model: "glm-5.3".into(),
            tokens: tokens(512_000, 9_100),
        }],
    }
}

fn usage(update: UsageUpdate) -> Input {
    Input::Agent {
        agent: agent(),
        revision: Some("rev-1".into()),
        update: AgentUpdate::Usage(update),
    }
}

fn step(id: &str, parent: Option<&str>, kind: StepKind, state: StepState) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Step(StepReport {
            id: id.into(),
            parent: parent.map(Into::into),
            kind,
            label: format!("step {id}"),
            state,
            icon: None,
            detail: None,
            input: None,
            output: None,
        }),
    }
}

fn feed(mut snap: Snapshot, inputs: &[Input]) -> (Snapshot, Vec<Command>) {
    let mut all = Vec::new();
    for input in inputs {
        let (next, cmds) = transition(&snap, input).unwrap();
        snap = next;
        all.extend(cmds);
    }
    (snap, all)
}

fn bodies(cmds: &[Command]) -> Vec<&EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(&d.body),
            _ => None,
        })
        .collect()
}

fn calls(cmds: &[Command]) -> Vec<&ModelUsageData> {
    bodies(cmds)
        .into_iter()
        .filter_map(|b| match b {
            EventBody::ModelUsage(d) => Some(d.as_ref()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_call_report_is_a_model_usage_of_the_agent_and_moves_nothing() {
    for state in [Queued, Working, Blocked, Verifying] {
        let before = Snapshot::new(state);
        let (after, cmds) =
            transition(&before, &usage(UsageUpdate::Call(call("c1", None)))).unwrap();
        assert_eq!(after.state, state, "no state moves in {state:?}");
        let [Command::Append(draft)] = cmds.as_slice() else {
            panic!("exactly one event: {cmds:?}")
        };
        assert_eq!(draft.actor, Actor::agent(&agent(), Some("rev-1".into())));
        assert_eq!(
            draft.body,
            EventBody::ModelUsage(Box::new(ModelUsageData {
                job: 1,
                agent: "coder".into(),
                task: "t1".into(),
                call: "c1".into(),
                path: vec![],
                provider: Some("openai".into()),
                model: "glm-5.3".into(),
                tokens: tokens(41_250, 812),
                context_window: Some(131_072),
            }))
        );
        assert_eq!(after.job.usage.calls(), 1);
    }
}

#[test]
fn a_finished_thread_refuses_a_report_as_a_late_update() {
    for state in [Done, Failed, Cancelled] {
        for update in [
            UsageUpdate::Call(call("c1", None)),
            UsageUpdate::Total(totals()),
        ] {
            let err = transition(&Snapshot::new(state), &usage(update)).unwrap_err();
            assert!(
                matches!(err, TransitionError::InvalidInState { .. }),
                "{state:?}: {err:?}"
            );
        }
    }
}

#[test]
fn a_call_under_an_open_step_has_the_path_a_child_of_that_step_would_have() {
    let (_, cmds) = feed(
        Snapshot::new(Working),
        &[
            step("t1/tool:c2", None, StepKind::Subagent, StepState::Running),
            step(
                "t1/acp:1",
                Some("t1/tool:c2"),
                StepKind::Subagent,
                StepState::Running,
            ),
            usage(UsageUpdate::Call(call("c1", Some("t1/tool:c2")))),
            usage(UsageUpdate::Call(call("c2", Some("t1/acp:1")))),
            usage(UsageUpdate::Call(call("c3", None))),
        ],
    );
    let paths: Vec<Vec<String>> = calls(&cmds).iter().map(|c| c.path.clone()).collect();
    assert_eq!(
        paths,
        vec![
            vec!["t1/tool:c2".to_owned()],
            vec!["t1/tool:c2".to_owned(), "t1/acp:1".to_owned()],
            vec![],
        ]
    );
}

#[test]
fn an_unknown_or_ended_step_is_the_agents_own_call() {
    let (_, cmds) = feed(
        Snapshot::new(Working),
        &[
            usage(UsageUpdate::Call(call("c1", Some("t1/never")))),
            step("t1/tool:c2", None, StepKind::Subagent, StepState::Running),
            step("t1/tool:c2", None, StepKind::Subagent, StepState::Completed),
            usage(UsageUpdate::Call(call("c2", Some("t1/tool:c2")))),
        ],
    );
    assert!(calls(&cmds).iter().all(|c| c.path.is_empty()));
}

#[test]
fn a_path_keeps_the_eight_nearest_steps() {
    let mut inputs = Vec::new();
    let mut parent: Option<String> = None;
    for i in 0..12 {
        let id = format!("t1/s{i}");
        inputs.push(step(
            &id,
            parent.as_deref(),
            StepKind::Subagent,
            StepState::Running,
        ));
        parent = Some(id);
    }
    inputs.push(usage(UsageUpdate::Call(call("c1", Some("t1/s11")))));
    let (_, cmds) = feed(Snapshot::new(Working), &inputs);
    let path = &calls(&cmds)[0].path;
    assert_eq!(path.len(), MAX_STEP_DEPTH);
    assert_eq!(path.last().map(String::as_str), Some("t1/s11"));
}

#[test]
fn totals_are_a_model_usage_total_and_are_never_capped() {
    let mut snap = Snapshot::new(Working);
    for i in 0..MAX_USAGE_CALLS_PER_JOB {
        let (next, cmds) = transition(
            &snap,
            &usage(UsageUpdate::Call(call(&format!("c{i}"), None))),
        )
        .unwrap();
        assert_eq!(calls(&cmds).len(), 1);
        snap = next;
    }
    // the job has logged as many calls as it may: the next is dropped and counted
    let (snap, cmds) = transition(&snap, &usage(UsageUpdate::Call(call("over", None)))).unwrap();
    assert!(cmds.is_empty());
    assert_eq!(snap.job.usage.calls(), MAX_USAGE_CALLS_PER_JOB);
    assert_eq!(snap.job.usage.dropped(), 1);
    // the totals are the record, and still logged
    let (snap, cmds) = transition(&snap, &usage(UsageUpdate::Total(totals()))).unwrap();
    assert_eq!(
        bodies(&cmds),
        vec![&EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "coder".into(),
            task: "t1".into(),
            path: vec![],
            totals: totals().totals,
        })]
    );
    assert_eq!(snap.state, Working);
}

#[test]
fn the_next_job_forgets_the_count() {
    let mut job = Job::default();
    job.usage = transition(
        &Snapshot::new(Working),
        &usage(UsageUpdate::Call(call("c1", None))),
    )
    .unwrap()
    .0
    .job
    .usage;
    assert_eq!(job.usage.calls(), 1);
    assert!(job.next().usage.is_empty());
}

#[test]
fn a_report_that_fails_the_door_or_was_refused_logs_nothing() {
    let mut bad = call("c1", None);
    bad.tokens.total_tokens += 1;
    let mut blank = call("c2", None);
    blank.model = " ".into();
    let mut empty_task = totals();
    empty_task.task = String::new();
    for input in [
        usage(UsageUpdate::Call(bad)),
        usage(UsageUpdate::Call(blank)),
        usage(UsageUpdate::Total(empty_task)),
        Input::Agent {
            agent: agent(),
            revision: None,
            update: AgentUpdate::UsageRejected(UsageInvalid::TotalNotSum),
        },
    ] {
        let (after, cmds) = transition(&Snapshot::new(Working), &input).unwrap();
        assert!(cmds.is_empty(), "{input:?}");
        assert!(after.job.usage.is_empty());
    }
}

fn with_ask(state: ThreadState) -> Snapshot {
    let mut snap = Snapshot::new(state);
    snap.job.asks.push(Ask {
        n: 1,
        by: Caller::Main,
        agent: AgentId::new("researcher"),
        depth: 1,
        call_key: None,
        fingerprint: None,
        task_id: Some("ask-task".into()),
        context_id: None,
        outcome: None,
    });
    snap
}

fn ask_usage(job: u32, ask: u32, update: UsageUpdate) -> Input {
    Input::AskUsage {
        job,
        ask,
        revision: Some("r9".into()),
        usage: update,
    }
}

#[test]
fn an_asked_agents_usage_is_its_own_under_its_ask() {
    let mut on_ask_task = call("a1", Some("ask-task/tool:x"));
    on_ask_task.task = "ask-task".into();
    let mut ask_totals = totals();
    ask_totals.task = "ask-task".into();
    let (after, cmds) = feed(
        with_ask(Working),
        &[
            ask_usage(1, 1, UsageUpdate::Call(on_ask_task)),
            ask_usage(1, 1, UsageUpdate::Total(ask_totals)),
        ],
    );
    let Command::Append(first) = &cmds[0] else {
        panic!("{cmds:?}")
    };
    assert_eq!(
        first.actor,
        Actor::agent(&AgentId::new("researcher"), Some("r9".into()))
    );
    let EventBody::ModelUsage(d) = &first.body else {
        panic!("{first:?}")
    };
    assert_eq!(d.agent, "researcher");
    assert_eq!(d.task, "ask-task");
    assert_eq!(
        d.path,
        vec!["ask-1".to_owned()],
        "an ask's step, not the agent's own"
    );
    let Command::Append(second) = &cmds[1] else {
        panic!("{cmds:?}")
    };
    let EventBody::ModelUsageTotal(t) = &second.body else {
        panic!("{second:?}")
    };
    assert_eq!(t.path, vec!["ask-1".to_owned()]);
    assert_eq!(t.agent, "researcher");
    assert_eq!(after.state, Working);
    assert_eq!(
        after.job.usage.calls(),
        1,
        "an ask's calls count for the job"
    );
}

#[test]
fn an_ask_of_another_job_or_one_the_ledger_has_not_logs_nothing() {
    for input in [
        ask_usage(2, 1, UsageUpdate::Call(call("a1", None))),
        ask_usage(1, 7, UsageUpdate::Call(call("a1", None))),
    ] {
        let (_, cmds) = transition(&with_ask(Working), &input).unwrap();
        assert!(cmds.is_empty(), "{input:?}");
    }
    let err = transition(
        &with_ask(Done),
        &ask_usage(1, 1, UsageUpdate::Call(call("a1", None))),
    )
    .unwrap_err();
    assert!(matches!(err, TransitionError::InvalidInState { .. }));
}

#[test]
fn the_events_read_back_from_the_log() {
    let (_, cmds) = feed(
        Snapshot::new(Working),
        &[
            usage(UsageUpdate::Call(call("c1", None))),
            usage(UsageUpdate::Total(totals())),
        ],
    );
    for body in bodies(&cmds) {
        let back = EventBody::from_parts(body.kind(), body.data_value()).unwrap();
        assert_eq!(&back, body);
    }
    assert_eq!(EventKind::ModelUsage.as_str(), "model_usage");
    assert_eq!(EventKind::ModelUsageTotal.as_str(), "model_usage_total");
}

#[test]
fn the_extension_is_known_by_its_exact_uri() {
    assert_eq!(
        KnownExtension::from_uri("https://agents.vymalo.com/a2a/extensions/usage/v1"),
        Some(KnownExtension::Usage)
    );
    assert_eq!(KnownExtension::Usage.uri(), USAGE_EXTENSION);
    assert_eq!(
        KnownExtension::from_uri("https://agents.vymalo.com/a2a/extensions/usage/v1/"),
        None
    );
}

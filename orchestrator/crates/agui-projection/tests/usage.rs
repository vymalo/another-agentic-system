//! Token usage (ADR 0056, `docs/api/agui.md`, "Token usage") on hand-written logs: a model call is a
//! `CUSTOM` `vymalo.usage` attributed to the subagent its path names, a task's totals a `CUSTOM`
//! `vymalo.usage_total`, and a run's terminal event says AG-UI's accounting of the thread's agent:
//! the task's latest totals for a run of its own, only what a run added for one that resumes it, the
//! sum of the calls without totals, nothing without usage; an asked agent's is under its own
//! subagent. Usage opens no run, and a reconnect says the same as the live stream.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Connect, Follow, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, AgentStepData, AskFinishedData, AskOutcome,
    AskStartedData, Caller, ErrorData, Event, EventBody, ModelTokens, ModelUsageData,
    ModelUsageTotalData, StepKind, StepPhase, StepState, ThreadState, ThreadStateData, Timestamp,
    TokenCounts, UserId, UserMessageData,
};
use serde_json::{Value, json};
use support::log::{meta, thread_id};
use support::{lines, verify};

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn plain() -> Actor {
    Actor::agent(&AgentId::new("plain"), None)
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData::new(text)),
    )
}

fn status(seq: i64, status: AgentStatus, detail: Option<&str>) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: detail.map(str::to_owned),
        }),
    )
}

fn thread(seq: i64, state: ThreadState) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData { state }),
    )
}

fn counts(input: u64, output: u64) -> TokenCounts {
    TokenCounts {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..TokenCounts::default()
    }
}

/// A call of `task` on model `m` under `path`.
fn call(seq: i64, task: &str, call: &str, path: &[&str], input: u64, output: u64) -> Event {
    ev(
        seq,
        plain(),
        EventBody::ModelUsage(Box::new(ModelUsageData {
            job: 1,
            agent: "plain".into(),
            task: task.into(),
            call: call.into(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            provider: Some("openai".into()),
            model: "m".into(),
            tokens: counts(input, output),
            context_window: Some(1000),
        })),
    )
}

fn total(seq: i64, task: &str, path: &[&str], input: u64, output: u64) -> Event {
    ev(
        seq,
        plain(),
        EventBody::ModelUsageTotal(ModelUsageTotalData {
            job: 1,
            agent: "plain".into(),
            task: task.into(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            totals: vec![ModelTokens {
                provider: Some("openai".into()),
                model: "m".into(),
                tokens: counts(input, output),
            }],
        }),
    )
}

fn subagent_step(seq: i64, id: &str, state: StepState, phase: StepPhase) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStep(AgentStepData {
            id: id.to_owned(),
            path: vec![],
            kind: StepKind::Subagent,
            label: "Researcher".into(),
            state,
            phase,
            icon: None,
            detail: None,
            input: None,
            output: None,
            io_dropped: false,
        }),
    )
}

fn project(events: &[Event]) -> Vec<Frame> {
    let mut projector = Projector::new(meta());
    let frames: Vec<Frame> = events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    verify::check(&frames).unwrap();
    frames
}

/// The values of the `CUSTOM` events named `name`.
fn customs(frames: &[Frame], name: &str) -> Vec<Value> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::Custom(e) if e.name == name => Some(e.value.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_call_is_a_custom_of_the_agents_invocation_and_the_run_says_its_tasks_totals() {
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        call(3, "t1", "c1", &[], 100, 10),
        call(4, "t1", "c2", &[], 200, 20),
        total(5, "t1", &[], 300, 30),
        status(6, AgentStatus::Completed, None),
        thread(7, ThreadState::Done),
    ]);
    let got = lines(&frames);
    assert!(
        got.contains(&"CUSTOM vymalo.usage c1 @sub-2  id:3".to_owned()),
        "{got:#?}"
    );
    assert!(
        got.contains(&"CUSTOM vymalo.usage_total total @sub-2  id:5".to_owned()),
        "{got:#?}"
    );
    assert_eq!(
        got.last().unwrap(),
        "RUN_FINISHED run-1 success usage=[m:300/30]  id:7"
    );
    // the value is the event's data, with who spent it and when
    assert_eq!(
        customs(&frames, "vymalo.usage")[0],
        json!({"job": 1, "agent": "plain", "task": "t1", "call": "c1", "path": [],
               "provider": "openai", "model": "m", "inputTokens": 100, "outputTokens": 10,
               "totalTokens": 110, "contextWindow": 1000,
               "by": {"kind": "agent", "name": "plain"}, "at": "2027-01-15T08:00:03Z"})
    );
    let total = &customs(&frames, "vymalo.usage_total")[0];
    assert_eq!(total["totals"][0]["inputTokens"], 300);
    assert_eq!(total["by"], json!({"kind": "agent", "name": "plain"}));
}

#[test]
fn a_call_under_a_sub_agent_step_is_said_under_its_subagent_by_its_name() {
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        subagent_step(3, "t1/tool:c2", StepState::Running, StepPhase::Start),
        call(4, "t1", "c1", &["t1/tool:c2"], 50, 5),
        subagent_step(5, "t1/tool:c2", StepState::Completed, StepPhase::End),
        status(6, AgentStatus::Completed, None),
        thread(7, ThreadState::Done),
    ]);
    let got = lines(&frames);
    assert!(
        got.contains(&"CUSTOM vymalo.usage c1 @sub-step-3  id:4".to_owned()),
        "{got:#?}"
    );
    assert_eq!(
        customs(&frames, "vymalo.usage")[0]["by"],
        json!({"kind": "subagent", "name": "Researcher"})
    );
    // without totals, the run says the sum of its calls, its sub-agents' included
    assert_eq!(
        got.last().unwrap(),
        "RUN_FINISHED run-1 success usage=[m:50/5]  id:7"
    );
}

#[test]
fn a_run_without_usage_says_none_and_a_failed_run_says_what_it_spent() {
    let none = project(&[
        user(1, "go"),
        status(2, AgentStatus::Completed, None),
        thread(3, ThreadState::Done),
    ]);
    assert_eq!(
        lines(&none).last().unwrap(),
        "RUN_FINISHED run-1 success  id:3"
    );
    let failed = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        call(3, "t1", "c1", &[], 70, 7),
        status(4, AgentStatus::Failed, Some("boom")),
        thread(5, ThreadState::Failed),
    ]);
    assert_eq!(
        lines(&failed).last().unwrap(),
        "RUN_ERROR agent_failed \"boom\" usage=[m:70/7]  id:5"
    );
}

#[test]
fn a_run_that_resumes_the_task_says_only_what_it_added() {
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        call(3, "t1", "c1", &[], 100, 10),
        total(4, "t1", &[], 100, 10),
        status(5, AgentStatus::InputRequired, Some("Which branch?")),
        thread(6, ThreadState::Blocked),
        user(7, "main"),
        status(8, AgentStatus::Working, None),
        call(9, "t1", "c2", &[], 40, 4),
        total(10, "t1", &[], 140, 14),
        status(11, AgentStatus::Completed, None),
        thread(12, ThreadState::Done),
    ]);
    let ends: Vec<String> = lines(&frames)
        .into_iter()
        .filter(|l| l.starts_with("RUN_FINISHED"))
        .collect();
    assert_eq!(ends.len(), 2, "{ends:?}");
    assert!(ends[0].contains("usage=[m:100/10]"), "{ends:?}");
    assert!(ends[1].contains("usage=[m:40/4]"), "{ends:?}");
}

#[test]
fn an_asked_agents_usage_is_under_its_ask_and_not_in_its_askers_run() {
    let coder = Actor::agent(&AgentId::new("coder"), None);
    let asked = |seq, body| ev(seq, coder.clone(), body);
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        ev(
            3,
            plain(),
            EventBody::AskStarted(AskStartedData {
                ask: 1,
                agent: AgentId::new("coder"),
                by: Caller::Main,
                depth: 1,
                text: "find it".into(),
                step_id: "ask-1".into(),
                parent_step_id: None,
            }),
        ),
        {
            let mut e = call(4, "ask-task", "a1", &["ask-1"], 900, 90);
            e.actor = coder.clone();
            if let EventBody::ModelUsage(d) = &mut e.body {
                d.agent = "coder".into();
            }
            e
        },
        {
            let mut e = total(5, "ask-task", &["ask-1"], 900, 90);
            e.actor = coder.clone();
            if let EventBody::ModelUsageTotal(d) = &mut e.body {
                d.agent = "coder".into();
            }
            e
        },
        asked(
            6,
            EventBody::AskFinished(AskFinishedData {
                ask: 1,
                state: AskOutcome::Completed,
                text: Some("found".into()),
                question: None,
                artifacts: vec![],
                error: None,
            }),
        ),
        call(7, "t1", "c1", &[], 10, 1),
        status(8, AgentStatus::Completed, None),
        thread(9, ThreadState::Done),
    ]);
    let got = lines(&frames);
    assert!(
        got.contains(&"CUSTOM vymalo.usage a1 @sub-ask-1  id:4".to_owned()),
        "{got:#?}"
    );
    assert!(
        got.contains(&"CUSTOM vymalo.usage_total total @sub-ask-1  id:5".to_owned()),
        "{got:#?}"
    );
    assert_eq!(
        customs(&frames, "vymalo.usage")[0]["by"],
        json!({"kind": "ask", "name": "coder"})
    );
    assert_eq!(
        got.last().unwrap(),
        "RUN_FINISHED run-1 success usage=[m:10/1]  id:9",
        "the thread's agent's run, not the asked agent's"
    );
}

#[test]
fn usage_opens_no_run_and_an_error_before_it_still_explains_the_state() {
    // a late report on a waiting thread: folded, nothing said
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::InputRequired, Some("Which branch?")),
        thread(3, ThreadState::Blocked),
        call(4, "t1", "late", &[], 5, 1),
    ]);
    assert!(customs(&frames, "vymalo.usage").is_empty());
    assert!(
        !lines(&frames)
            .iter()
            .any(|l| l.starts_with("RUN_STARTED run-4"))
    );
    // an error, a usage event the same commit did not write, and the state the error explains
    let frames = project(&[
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        ev(
            3,
            Actor::system(),
            EventBody::Error(ErrorData {
                message: "the agent cannot be reached".into(),
                retryable: false,
            }),
        ),
        call(4, "t1", "c1", &[], 5, 1),
        thread(5, ThreadState::Failed),
    ]);
    assert!(
        lines(&frames)
            .last()
            .unwrap()
            .starts_with("RUN_ERROR delivery_failed"),
        "{:#?}",
        lines(&frames)
    );
}

#[test]
fn a_client_that_reconnects_mid_run_is_told_the_same_usage_at_its_end() {
    let log = vec![
        user(1, "go"),
        status(2, AgentStatus::Working, None),
        call(3, "t1", "c1", &[], 100, 10),
        call(4, "t1", "c2", &[], 20, 2),
        status(5, AgentStatus::Completed, None),
        thread(6, ThreadState::Done),
    ];
    let live = project(&log);
    for cursor in 1..=5 {
        let mut connect = Connect::new(meta(), cursor, 6, Follow::Forever);
        let frames: Vec<Frame> = log.iter().flat_map(|e| connect.feed(e)).collect();
        assert_eq!(
            lines(&frames).last(),
            lines(&live).last(),
            "from cursor {cursor}"
        );
        // and the calls it had not read are said to it as they were live
        let after: Vec<Value> = customs(&frames, "vymalo.usage");
        let live_after: Vec<Value> = customs(&live, "vymalo.usage")
            .into_iter()
            .skip(usize::try_from((cursor - 2).clamp(0, 2)).unwrap())
            .collect();
        assert_eq!(after, live_after, "from cursor {cursor}");
    }
}

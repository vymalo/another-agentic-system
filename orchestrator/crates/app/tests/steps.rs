//! Nested steps through the application (ADR 0025): the steps an agent reports reach the log as
//! `agent_step` events with their path, a chatty step is coalesced, the orchestrator can report
//! steps of its own, and a user cannot forge one.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, ApplyOutcome};
use orch_core::{
    Actor, AgentId, AgentStepData, EventBody, Input, MAX_STEP_UPDATES, StepKind, StepOutput,
    StepPhase, StepReport, StepState, ThreadState, TransitionError,
};
use orch_ports::ThreadStore;
use support::*;

fn steps_of(events: &[orch_core::Event]) -> Vec<&AgentStepData> {
    events
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::AgentStep(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn report(id: &str, parent: Option<&str>, state: StepState) -> StepReport {
    StepReport {
        id: id.into(),
        parent: parent.map(Into::into),
        kind: StepKind::Tool,
        label: format!("tool {id}"),
        state,
        icon: Some("mcp-server:websearch".into()),
        detail: None,
        input: None,
        output: None,
    }
}

#[tokio::test]
async fn the_steps_an_agent_reports_are_logged_nested_between_its_status_and_its_end() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "steps now").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_step",
            "agent_step",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ]
    );
    assert_contiguous(&ev);
    let got: Vec<_> = steps_of(&ev)
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                s.path.iter().map(String::as_str).collect::<Vec<_>>(),
                s.phase,
                s.state,
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                "task-1/tool:call_1",
                vec![],
                StepPhase::Start,
                StepState::Running
            ),
            (
                "task-1/acp:1",
                vec!["task-1/tool:call_1"],
                StepPhase::Start,
                StepState::Running
            ),
            (
                "task-1/acp:1",
                vec!["task-1/tool:call_1"],
                StepPhase::End,
                StepState::Completed
            ),
            (
                "task-1/acp:2",
                vec!["task-1/tool:call_1"],
                StepPhase::Start,
                StepState::Running
            ),
            (
                "task-1/acp:2",
                vec!["task-1/tool:call_1"],
                StepPhase::End,
                StepState::Failed
            ),
            (
                "task-1/tool:call_1",
                vec![],
                StepPhase::End,
                StepState::Completed
            ),
        ]
    );
    // attributed to the agent that reported them, with the detail it gave
    let all = steps_of(&ev);
    assert_eq!(all[4].detail.as_deref(), Some("1 failed"));
    assert!(
        ev.iter()
            .filter(|e| matches!(e.body, EventBody::AgentStep(_)))
            .all(|e| e.actor.name == "plain")
    );
    // the task ended: nothing is left open in the job's ledger
    let record = w.store.get_thread(None, t.id).await.unwrap().unwrap();
    assert_eq!(record.job.steps.open_count(), 0);
    run.shutdown().await;
}

#[tokio::test]
async fn the_orchestrator_reports_a_step_of_its_own_and_it_is_coalesced_like_any_other() {
    let w = World::new();
    let app = w.app();
    // a thread that is working: a `gate` agent holds its task open
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let actor = Actor::agent(&AgentId::new("plain"), None);

    let outcome = app
        .record_step(
            t.id,
            actor.clone(),
            report("tool-1", None, StepState::Running),
            Some("relay:tool-1:start".into()),
        )
        .await
        .unwrap();
    let ApplyOutcome::Applied { events, .. } = outcome else {
        panic!("{outcome:?}")
    };
    assert_eq!(events.len(), 1);
    // a replay of the same report is the same input
    assert!(matches!(
        app.record_step(
            t.id,
            actor.clone(),
            report("tool-1", None, StepState::Running),
            Some("relay:tool-1:start".into())
        )
        .await
        .unwrap(),
        ApplyOutcome::Duplicate
    ));
    // a chatty step: only so many updates are logged
    for n in 0..20 {
        app.record_step(
            t.id,
            actor.clone(),
            report("tool-1", None, StepState::Running),
            Some(format!("relay:tool-1:update:{n}")),
        )
        .await
        .unwrap();
    }
    app.record_step(
        t.id,
        actor,
        report("tool-1", None, StepState::Completed),
        Some("relay:tool-1:end".into()),
    )
    .await
    .unwrap();

    let ev = events_of(&app, t.id).await;
    let phases: Vec<_> = steps_of(&ev).iter().map(|s| s.phase).collect();
    assert_eq!(phases.len(), 2 + usize::from(MAX_STEP_UPDATES));
    assert_eq!(phases[0], StepPhase::Start);
    assert_eq!(phases.last(), Some(&StepPhase::End));
    // the icon the orchestrator named is kept
    assert_eq!(
        steps_of(&ev)[0].icon.as_deref(),
        Some("mcp-server:websearch")
    );
    assert_contiguous(&ev);
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

async fn events_of(app: &TestApp, id: orch_core::ThreadId) -> Vec<orch_core::Event> {
    events(app, &alice(), id).await
}

#[tokio::test]
async fn a_step_for_a_finished_thread_is_refused() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let before = events(&app, &alice(), t.id).await.len();
    let err = app
        .record_step(
            t.id,
            Actor::system(),
            report("late", None, StepState::Running),
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        AppError::Transition(TransitionError::InvalidInState { .. })
    ));
    assert_eq!(events(&app, &alice(), t.id).await.len(), before);
    run.shutdown().await;
}

#[tokio::test]
async fn a_user_cannot_submit_a_step() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let err = app
        .submit(
            &alice(),
            t.id,
            Input::Step {
                actor: Actor::system(),
                report: report("forged", None, StepState::Running),
            },
            None,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    assert!(steps_of(&events(&app, &alice(), t.id).await).is_empty());
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

/// A report of a tool call the orchestrator relays, with what it was called with and what it
/// returned (ADR 0030; plan 05's "arguments and results are not logged" is overridden).
fn relayed(state: StepState) -> StepReport {
    let mut r = report("relay-1", None, state);
    if state == StepState::Running {
        r.input = serde_json::json!({"query": "node 24", "api_key": "k-123456"})
            .as_object()
            .cloned();
    } else {
        r.output = Some(StepOutput {
            text: "two results\nAuthorization: Bearer abcdefghijklmnop".into(),
            ..StepOutput::default()
        });
    }
    r
}

#[tokio::test]
async fn a_step_the_orchestrator_reports_carries_its_input_and_output_redacted() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let actor = Actor::agent(&AgentId::new("plain"), None);
    for (state, key) in [(StepState::Running, "start"), (StepState::Completed, "end")] {
        app.record_step(
            t.id,
            actor.clone(),
            relayed(state),
            Some(format!("relay:{key}")),
        )
        .await
        .unwrap();
    }
    let ev = events_of(&app, t.id).await;
    let steps = steps_of(&ev);
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[0]
            .input
            .as_ref()
            .map(|i| serde_json::Value::Object(i.clone())),
        Some(serde_json::json!({"query": "node 24", "api_key": "[redacted]"}))
    );
    assert_eq!(steps[0].output, None);
    assert_eq!(
        steps[1].input, None,
        "the input is logged once, with the start"
    );
    let output = steps[1].output.as_ref().unwrap();
    assert!(output.text.contains("two results"));
    assert!(!output.text.contains("abcdefghijklmnop"), "{}", output.text);
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn with_the_switch_off_no_step_carries_input_or_output() {
    let w = World::new();
    let app = w.app_with(orch_app::AppConfig {
        record_step_io: false,
        ..orch_app::AppConfig::default()
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    // the steps an agent reports: the scripted agent's `npm test` has an input and an output
    let t = create(&app, &alice(), "plain", "steps now").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events_of(&app, t.id).await;
    let steps = steps_of(&ev);
    assert_eq!(steps.len(), 6, "every step is still logged");
    assert!(
        steps
            .iter()
            .all(|s| s.input.is_none() && s.output.is_none() && !s.io_dropped)
    );
    // and the steps the orchestrator reports itself
    let t = create(&app, &alice(), "plain", "gate go").await;
    wait_state(&app, &alice(), t.id, ThreadState::Working).await;
    let actor = Actor::agent(&AgentId::new("plain"), None);
    for (state, key) in [(StepState::Running, "start"), (StepState::Completed, "end")] {
        app.record_step(
            t.id,
            actor.clone(),
            relayed(state),
            Some(format!("relay:{key}")),
        )
        .await
        .unwrap();
    }
    let ev = events_of(&app, t.id).await;
    let steps = steps_of(&ev);
    assert_eq!(steps.len(), 2);
    assert!(
        steps
            .iter()
            .all(|s| s.input.is_none() && s.output.is_none())
    );
    w.agent.release_gate();
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
}

#[tokio::test]
async fn by_default_the_steps_an_agent_reports_carry_theirs() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "steps now").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events_of(&app, t.id).await;
    let steps = steps_of(&ev);
    let start = steps
        .iter()
        .find(|s| s.id.ends_with("acp:2") && s.phase == StepPhase::Start);
    assert_eq!(
        start
            .unwrap()
            .input
            .as_ref()
            .map(|i| serde_json::Value::Object(i.clone())),
        Some(serde_json::json!({"command": "npm test"}))
    );
    let end = steps
        .iter()
        .find(|s| s.id.ends_with("acp:2") && s.phase == StepPhase::End);
    assert_eq!(
        end.unwrap().output,
        Some(StepOutput {
            text: "1 failed".into(),
            error: true,
            ..StepOutput::default()
        })
    );
    run.shutdown().await;
}

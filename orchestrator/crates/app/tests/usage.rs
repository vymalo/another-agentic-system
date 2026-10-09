//! Token usage through the application (ADR 0056, `usage/v1`): what the agent's adapter reports
//! reaches the log as `model_usage` and `model_usage_total`, a call under a sub-agent step with the
//! step's path, once however often it is replayed; a report that broke the contract is counted and
//! nothing else; a user cannot forge an asked agent's usage.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{AppError, UsageDrop};
use orch_core::{
    Actor, AgentId, EventBody, Input, ModelUsageData, ThreadState, TokenCounts, UsageCall,
    UsageUpdate,
};
use support::*;

#[tokio::test]
async fn the_calls_and_the_totals_an_agent_reports_are_logged_with_their_path_once() {
    let w = World::new();
    let app = w.app();
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "usage now").await;
    wait_state(&app, &alice(), t.id, ThreadState::Done).await;
    let ev = events(&app, &alice(), t.id).await;
    assert_eq!(
        shape(&ev),
        [
            "user_message",
            "agent_status:working",
            "model_usage",
            "agent_step",
            "model_usage",
            "agent_step",
            "model_usage_total",
            "artifact",
            "agent_status:completed",
            "thread_state:done"
        ],
        "the replayed call is logged once, the refused report not at all"
    );
    assert_contiguous(&ev);
    let calls: Vec<&ModelUsageData> = ev
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::ModelUsage(d) => Some(d.as_ref()),
            _ => None,
        })
        .collect();
    let task = &calls[0].task;
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.call.as_str(), c.path.clone()))
            .collect::<Vec<_>>(),
        [("c1", vec![]), ("c2", vec![format!("{task}/tool:call_1")]),]
    );
    assert!(calls.iter().all(|c| c.agent == "plain" && c.job == 1));
    let usage = ev
        .iter()
        .find(|e| matches!(e.body, EventBody::ModelUsage(_)))
        .unwrap();
    assert_eq!(usage.actor.name, "plain");
    // the refused report is counted
    let stats = app.usage_stats();
    assert_eq!(stats.reports_dropped[0], (UsageDrop::Invalid, 1));
    assert_eq!(stats.reports_dropped[1], (UsageDrop::JobLimit, 0));
    // and the job counted its two calls
    let thread = app.get_thread(&alice(), t.id).await.unwrap();
    assert_eq!(thread.job.usage.calls(), 2);
    run.shutdown().await;
}

#[tokio::test]
async fn a_user_cannot_submit_an_asked_agents_usage() {
    let w = World::new();
    let app = w.app();
    let t = create(&app, &alice(), "plain", "slow").await;
    let forged = Input::AskUsage {
        job: 1,
        ask: 1,
        revision: None,
        usage: UsageUpdate::Call(UsageCall {
            task: "t".into(),
            call: "c".into(),
            step: None,
            provider: None,
            model: "m".into(),
            tokens: TokenCounts::default(),
            context_window: None,
        }),
    };
    let err = app.submit(&alice(), t.id, forged, None).await.unwrap_err();
    assert!(matches!(err, AppError::Invalid(_)), "{err:?}");
    let _ = (Actor::system(), AgentId::new("plain"));
}

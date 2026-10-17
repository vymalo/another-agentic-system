//! The cap of a person's role (ADR 0061, `maxTier`) when the agent's card offered no tier at the time
//! a message was accepted: the application pins the cap beside the tier, every row the job writes
//! carries it, and the dispatcher decides from the **live** card each time it sends. An agent that is
//! redeployed with tiers between the message and the send is never given its own default for a
//! person capped below it; one that has no tier within the cap, or lists the extension badly, is not
//! sent the message, and the job fails with the reason.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentDirectory, AgentEntry, AgentScope, App, AppConfig, AppError, Creation, Inbound, NewThread,
    Permission, Policy, RoleGrant,
};
use orch_core::{
    AccessTier, AgentId, EventBody, Input, KnownExtension, Origin, ThreadId, ThreadState, TierOffer,
};
use orch_ports::memory::Call;
use orch_ports::{
    AgentEndpoint, OutboxPayload, OutboxStatus, PortSet, Role, SystemClock, ThreadStore,
};
use serde_json::json;
use support::*;

use AccessTier::Edits;

fn offer(tiers: &[&str], default: &str) -> TierOffer {
    TierOffer::from_params(&json!({"tiers": tiers, "default": default})).unwrap()
}

/// The four tiers, working under `auto` when none is named: what a coder redeployed with the
/// extension lists.
fn all_tiers() -> TierOffer {
    offer(&["plan", "edits", "auto", "bypass"], "auto")
}

fn grant(cap: Option<AccessTier>) -> RoleGrant {
    RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::All,
        max_tier: cap,
    }
}

fn config(cap: Option<AccessTier>) -> AppConfig {
    AppConfig {
        policy: Policy::new(
            BTreeMap::from([(Role::new("careful"), grant(cap))]),
            Some(Role::new("careful")),
        )
        .unwrap(),
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    }
}

/// An application whose every person is capped at `cap`.
fn capped_at(w: &World, cap: Option<AccessTier>) -> Arc<TestApp> {
    w.app_with(config(cap))
}

fn id() -> ThreadId {
    ThreadId(uuid::Uuid::now_v7())
}

async fn start(app: &TestApp, text: &str) -> Result<ThreadId, AppError> {
    let thread = id();
    match app
        .create_thread_as(
            &alice(),
            thread,
            NewThread {
                title: None,
                target: target("plain"),
                text: text.to_owned(),
            },
            Inbound::default(),
        )
        .await?
    {
        Creation::Created { .. } => Ok(thread),
        Creation::Exists => panic!("a fresh id"),
    }
}

fn message(text: &str) -> Input {
    Input::UserMessage {
        tier_cap: None,
        user: alice(),
        text: text.to_owned(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
        mentions: Vec::new(),
        tier: None,
    }
}

/// What reached the agent `plain` as messages: the text, whether it was a steer, and the tier.
fn sent(w: &World) -> Vec<(String, bool, Option<AccessTier>)> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|call| match call {
            Call::Send {
                text, steer, tier, ..
            } => Some((text, steer, tier)),
            _ => None,
        })
        .collect()
}

/// The tier and the cap each `user_message` of the thread was written with.
async fn pins(app: &TestApp, thread: ThreadId) -> Vec<(Option<AccessTier>, Option<AccessTier>)> {
    events(app, &alice(), thread)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::UserMessage(m) => Some((m.tier, m.tier_cap)),
            _ => None,
        })
        .collect()
}

/// The tier and the cap of the one open delegation of the thread.
async fn row_pin(w: &World, thread: ThreadId) -> (Option<AccessTier>, Option<AccessTier>) {
    let rows = w.store.list_open_outbox(thread).await.unwrap();
    let [row] = rows.as_slice() else {
        panic!("{rows:?}")
    };
    let OutboxPayload::Delegate { tier, tier_cap, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    (*tier, *tier_cap)
}

/// Everything the log said as an error or a status detail, joined.
async fn said(app: &TestApp, thread: ThreadId) -> String {
    events(app, &alice(), thread)
        .await
        .iter()
        .filter_map(|e| match &e.body {
            EventBody::Error(m) => Some(m.message.clone()),
            EventBody::AgentStatus(s) => s.detail.clone(),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

// ---- the cap is pinned when the card offers nothing --------------------------------------

#[tokio::test]
async fn a_capped_person_and_an_agent_that_lists_no_tier_get_the_cap_pinned_on_the_message_and_the_job()
 {
    let w = World::new();
    let app = capped_at(&w, Some(Edits));
    let thread = start(&app, "echo go").await.unwrap();
    assert_eq!(
        pins(&app, thread).await,
        [(None, Some(Edits))],
        "the log says that a cap applied"
    );
    assert_eq!(
        row_pin(&w, thread).await,
        (None, Some(Edits)),
        "so does the row the dispatcher sends"
    );
    let job = app.get_thread(&alice(), thread).await.unwrap().job;
    assert_eq!((job.tier, job.tier_cap), (None, Some(Edits)));
    // a person with no cap has nothing to pin
    let thread = start(&w.app(), "echo go").await.unwrap();
    assert_eq!(pins(&app, thread).await, [(None, None)]);
    assert_eq!(row_pin(&w, thread).await, (None, None));
}

#[tokio::test]
async fn a_delegation_queued_before_the_agent_lists_tiers_is_sent_under_the_cap() {
    let w = World::new();
    let app = capped_at(&w, Some(Edits));
    let thread = start(&app, "echo go").await.unwrap();
    // the agent is redeployed with tiers, working under `auto`, before the dispatcher sends
    w.agent.set_access_tier("plain", Some(all_tiers()));
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(
        sent(&w),
        [("echo go".to_owned(), false, Some(Edits))],
        "never the agent's `auto`"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn the_rework_of_a_capped_person_is_sent_under_the_cap_when_the_agent_gained_tiers_meanwhile()
{
    let w = World::new();
    let app = w.app_with(AppConfig {
        gate: orch_core::GatePolicy::requiring([orch_core::CheckSource::AgentChecks]),
        ..config(Some(Edits))
    });
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, "gate-push one").await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    assert_eq!(
        sent(&w)[0].2,
        None,
        "the card listed nothing when it was sent"
    );
    // the coder is redeployed with the extension while the first attempt works
    w.agent.set_access_tier("plain", Some(all_tiers()));
    w.agent.release_gate();
    eventually("the rework reaches the agent", || async {
        (sent(&w).len() >= 2).then_some(())
    })
    .await;
    assert_eq!(
        sent(&w)[1].2,
        Some(Edits),
        "the rework is under the cap, not the agent's `auto`"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_gains_tiers_none_of_which_the_cap_allows_is_not_sent_the_message() {
    let w = World::new();
    let app = capped_at(&w, Some(Edits));
    let thread = start(&app, "echo go").await.unwrap();
    let rows: Vec<_> = w
        .store
        .list_open_outbox(thread)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    w.agent
        .set_access_tier("plain", Some(offer(&["auto", "bypass"], "auto")));
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Failed).await;
    assert!(sent(&w).is_empty(), "nothing is sent above the cap");
    let said = said(&app, thread).await;
    assert!(
        said.contains(
            "this agent has no access tier at or below 'edits', the most your role allows"
        ),
        "the job says why: {said}"
    );
    for id in rows {
        let row = w.store.get_outbox(id).await.unwrap().unwrap();
        assert_eq!(row.status, OutboxStatus::Dead, "{row:?}");
    }
    run.shutdown().await;
}

#[tokio::test]
async fn a_card_that_lists_the_extension_badly_is_refused_to_a_capped_person_at_the_message() {
    let w = World::new();
    w.agent.set_access_tier_malformed("plain");
    let app = capped_at(&w, Some(Edits));
    let error = start(&app, "echo go").await.unwrap_err();
    assert!(
        matches!(&error, AppError::Unprocessable(m) if m.contains("parameters that cannot be read") && m.contains("'edits'")),
        "{error:?}"
    );
    let listed = app
        .list_threads(&alice(), orch_ports::ThreadListing::recent(None, 50, false))
        .await
        .unwrap();
    assert!(listed.is_empty(), "nothing was written: {listed:?}");
}

#[tokio::test]
async fn a_card_that_comes_to_list_the_extension_badly_fails_the_job_of_a_capped_person_closed() {
    let w = World::new();
    let app = capped_at(&w, Some(Edits));
    let thread = start(&app, "echo go").await.unwrap();
    w.agent.set_access_tier_malformed("plain");
    let run = spawn_dispatcher(&app, fast(), "d1");
    wait_state(&app, &alice(), thread, ThreadState::Failed).await;
    assert!(sent(&w).is_empty());
    let said = said(&app, thread).await;
    assert!(said.contains("parameters that cannot be read"), "{said}");
    run.shutdown().await;
}

#[tokio::test]
async fn a_person_with_no_cap_is_sent_the_plain_message_whatever_the_card_lists() {
    // the cap is what makes the card matter: with none, a message that names no tier is the one it
    // always was, and no card is read for its tier
    let w = World::new();
    w.agent.set_access_tier_malformed("plain");
    let app = capped_at(&w, None);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, "echo go").await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    assert_eq!(sent(&w), [("echo go".to_owned(), false, None)]);
    run.shutdown().await;
}

#[tokio::test]
async fn a_message_sent_while_the_job_works_is_steered_under_the_cap_the_live_card_gives() {
    let w = World::new();
    w.agent.set_extensions("plain", &[KnownExtension::Steer]);
    let app = capped_at(&w, Some(Edits));
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, "gate go").await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    // the dispatcher goes away; the steer is written while the card still lists no tier, and
    // waits for the next one
    run.kill();
    app.submit(&alice(), thread, message("faster"), None)
        .await
        .unwrap();
    assert_eq!(
        pins(&app, thread).await,
        [(None, Some(Edits)), (None, Some(Edits))]
    );
    w.agent.set_access_tier("plain", Some(all_tiers()));
    let run = spawn_dispatcher(&app, fast(), "d2");
    eventually("the steer reached the agent", || async {
        sent(&w).iter().any(|(_, steer, _)| *steer).then_some(())
    })
    .await;
    assert_eq!(
        sent(&w)[1],
        ("faster".to_owned(), true, Some(Edits)),
        "into the running task, under the cap and not the agent's `auto`"
    );
    w.agent.release_gate();
    run.shutdown().await;
}

#[tokio::test]
async fn a_steer_the_card_cannot_give_a_tier_within_the_cap_is_not_sent_into_the_task() {
    let w = World::new();
    w.agent.set_extensions("plain", &[KnownExtension::Steer]);
    let app = capped_at(&w, Some(Edits));
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, "gate go").await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Working).await;
    run.kill();
    app.submit(&alice(), thread, message("faster"), None)
        .await
        .unwrap();
    w.agent
        .set_access_tier("plain", Some(offer(&["auto", "bypass"], "auto")));
    let run = spawn_dispatcher(&app, fast(), "d2");
    // the steer is not sent into the task: it becomes the delegation it stands for, which waits for
    // the end of the turn
    eventually("the steer became a delegation", || async {
        let rows = w.store.list_open_outbox(thread).await.unwrap();
        rows.iter()
            .any(|r| {
                matches!(
                    r.payload,
                    OutboxPayload::Delegate {
                        tier_cap: Some(Edits),
                        ..
                    }
                ) && r.sent_at.is_none()
            })
            .then_some(())
    })
    .await;
    assert_eq!(
        sent(&w).len(),
        1,
        "nothing went into the task: {:?}",
        sent(&w)
    );
    // the turn ends; the delegation is sent, the live card has no tier within the cap, and the job
    // fails with the reason instead of running the agent at its own default
    w.agent.release_gate();
    wait_state(&app, &alice(), thread, ThreadState::Failed).await;
    assert_eq!(sent(&w).len(), 1, "never sent: {:?}", sent(&w));
    let said = said(&app, thread).await;
    assert!(
        said.contains("no access tier at or below 'edits'"),
        "{said}"
    );
    run.shutdown().await;
}

// ---- every way in is resolved (m2) ---------------------------------------------------------

#[tokio::test]
async fn post_message_and_stop_and_send_are_resolved_as_any_message_of_a_person() {
    let w = World::new();
    w.agent.set_access_tier("plain", Some(all_tiers()));
    let app = capped_at(&w, Some(Edits));
    let run = spawn_dispatcher(&app, fast(), "d1");
    let thread = start(&app, "echo go").await.unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    app.post_message(&alice(), thread, "echo again".to_owned())
        .await
        .unwrap();
    wait_state(&app, &alice(), thread, ThreadState::Done).await;
    app.stop_and_send(&alice(), thread, "echo last".to_owned())
        .await
        .unwrap();
    eventually("the third message reaches the agent", || async {
        (sent(&w).len() == 3).then_some(())
    })
    .await;
    assert_eq!(
        pins(&app, thread).await,
        [
            (Some(Edits), None),
            (Some(Edits), None),
            (Some(Edits), None)
        ],
        "none of them is the agent's `auto`"
    );
    assert!(sent(&w).iter().all(|(_, _, tier)| *tier == Some(Edits)));
    run.shutdown().await;
}

#[tokio::test]
async fn post_message_to_an_agent_the_person_may_not_use_is_still_refused() {
    let w = World::new();
    let app = w.app_with(AppConfig {
        policy: Policy::new(
            BTreeMap::from([(
                Role::new("careful"),
                RoleGrant {
                    agents: AgentScope::from_patterns(["coder"]),
                    ..grant(Some(Edits))
                },
            )]),
            Some(Role::new("careful")),
        )
        .unwrap(),
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    });
    // a thread of someone with the right to use `plain`
    let thread = start(&w.app(), "echo go").await.unwrap();
    let error = app
        .post_message(&alice(), thread, "echo again".to_owned())
        .await
        .unwrap_err();
    assert!(
        matches!(error, AppError::Forbidden { .. }),
        "refused by the roles: {error:?}"
    );
}

// ---- an agent the registry no longer places (m3) ---------------------------------------------

#[tokio::test]
async fn an_answer_to_an_agent_the_registry_lost_keeps_the_cap_instead_of_the_jobs_uncapped_tier() {
    let w = World::new();
    w.agent.set_access_tier("plain", Some(all_tiers()));
    let open = w.app();
    let run = spawn_dispatcher(&open, fast(), "d1");
    let thread = start(&open, "ui pick one").await.unwrap();
    wait_state(&open, &alice(), thread, ThreadState::Blocked).await;
    run.shutdown().await;
    // the person's role is capped now, and the registry no longer lists the thread's agent
    let lost = AgentDirectory::new(vec![AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new("coder"),
            "https://coder.example.com/.well-known/agent-card.json",
            None,
        ),
        name: "Coder".to_owned(),
    }]);
    let app = Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: SystemClock,
                ids: w.ids.clone(),
                model: w.model.clone(),
                auth: orch_ports::RefuseAll,
                registry: lost.fixed_registry(),
                user_tools: orch_ports::NoUserTools::off(),
            },
            lost,
            config(Some(Edits)),
        )
        .unwrap(),
    );
    app.submit(&alice(), thread, message("yes"), None)
        .await
        .unwrap();
    assert_eq!(
        pins(&app, thread).await.last(),
        Some(&(None, Some(Edits))),
        "the cap is kept, to be decided when the agent can be asked"
    );
    let job = app.get_thread(&alice(), thread).await.unwrap().job;
    assert_eq!((job.tier, job.tier_cap), (None, Some(Edits)));
}

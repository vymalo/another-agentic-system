//! A mention and the access tier (ADR 0061): the tier of a parent does not pass down to an asked
//! agent (question 1), but **the cap of the person's role on the mentioned agent does**. The
//! application pins it on the mention when the message is accepted; the ask row carries it; the
//! dispatcher sends the ask under the highest tier the asked agent's live card offers within the
//! lower of its default and the cap, and fails the ask (no task started, `ask_finished` says why)
//! when the card has none that low or lists the extension badly.
//!
//! The roles of the tests: `planner` may use `coder` and goes no higher than `plan` there;
//! `chatter` may use `plain`, the thread's agent, with no cap; `free` may use `coder` with no cap.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentScope, App, AppConfig, Creation, Inbound, NewThread, Permission, Policy, RoleGrant,
};
use orch_core::{
    AccessTier, Actor, AgentId, AgentTaskState, AgentUpdate, AskFinishedData, AskLimits,
    AskOutcome, Caller, EventBody, Input, Mention, ThreadId, ThreadRecord, TierOffer,
};
use orch_ports::memory::Call;
use orch_ports::{
    Clock as _, OutboxItem, OutboxKind, OutboxPayload, OutboxStatus, PortSet, Principal, Role,
    SystemClock, ThreadStore,
};
use serde_json::json;
use support::*;

use AccessTier::{Bypass, Edits, Plan};

/// `@coder` is at 4..10.
const TEXT: &str = "ask @coder to help";

fn offer(tiers: &[&str], default: &str) -> TierOffer {
    TierOffer::from_params(&json!({"tiers": tiers, "default": default})).unwrap()
}

fn grant(agents: &[&str], cap: Option<AccessTier>) -> RoleGrant {
    RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::from_patterns(agents.iter().copied()),
        max_tier: cap,
    }
}

fn reader() -> RoleGrant {
    RoleGrant {
        permissions: BTreeSet::from([Permission::AgentRead, Permission::ThreadRead]),
        agents: AgentScope::All,
        max_tier: None,
    }
}

fn app_with_roles(w: &World) -> Arc<TestApp> {
    Arc::new(
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
                registry: directory().fixed_registry(),
                user_tools: orch_ports::NoUserTools::off(),
            },
            directory(),
            AppConfig {
                policy: Policy::new(
                    BTreeMap::from([
                        (Role::new("planner"), grant(&["coder"], Some(Plan))),
                        (Role::new("chatter"), grant(&["plain"], None)),
                        (Role::new("free"), grant(&["coder"], None)),
                        // what a person without a role of the ones above may do: read their own
                        (Role::new("reader"), reader()),
                    ]),
                    Some(Role::new("reader")),
                )
                .unwrap(),
                stream_poll: Duration::from_millis(100),
                ..AppConfig::default()
            },
        )
        .expect("a valid gate"),
    )
}

fn person(roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(alice())
    }
}

fn mention() -> Mention {
    Mention {
        agent_id: AgentId::new("coder"),
        label: "@coder".to_owned(),
        start: 4,
        end: 10,
        card_url: None,
        tier_cap: None,
    }
}

/// A thread of `who` addressed to `plain`, whose first message mentions `coder`, with `plain`
/// working on it. Nothing is delivered to `plain`.
async fn working_thread(
    w: &World,
    app: &TestApp,
    who: &Principal,
    tier: Option<AccessTier>,
) -> ThreadRecord {
    let id = ThreadId(uuid::Uuid::now_v7());
    let Creation::Created { .. } = app
        .create_thread_as(
            who,
            id,
            NewThread {
                title: None,
                target: target("plain"),
                text: TEXT.to_owned(),
            },
            Inbound {
                mentions: vec![mention()],
                tier,
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("a new thread");
    };
    // the thread's own delegation is not what these tests are about
    w.store
        .skip_unsent_delegates(id, SystemClock.now())
        .await
        .unwrap();
    app.apply(
        id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Status {
                state: AgentTaskState::Working,
                detail: None,
                tier: None,
            },
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
    app.get_thread(&alice(), id).await.unwrap()
}

/// Puts an ask of `coder` on the thread and returns the row that sends it.
async fn put(app: &TestApp, w: &World, thread: ThreadId) -> OutboxItem {
    app.apply(
        thread,
        Input::Ask {
            actor: Actor::agent(&AgentId::new("plain"), None),
            caller: Caller::Main,
            agent: AgentId::new("coder"),
            text: "echo find the data".to_owned(),
            call_key: Some("k1".to_owned()),
            parent_step: None,
            limits: AskLimits::default(),
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let mut rows: Vec<_> = w
        .store
        .list_open_outbox(thread)
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.kind == OutboxKind::Ask)
        .collect();
    assert_eq!(rows.len(), 1, "the ask wrote one row");
    rows.remove(0)
}

fn row_cap(row: &OutboxItem) -> Option<AccessTier> {
    let OutboxPayload::Ask { tier_cap, .. } = &row.payload else {
        panic!("{:?}", row.payload)
    };
    *tier_cap
}

async fn finished(app: &TestApp, thread: ThreadId) -> AskFinishedData {
    eventually("the ask is finished", || async {
        events(app, &alice(), thread)
            .await
            .into_iter()
            .find_map(|e| match e.body {
                EventBody::AskFinished(d) => Some(d),
                _ => None,
            })
    })
    .await
}

/// The tier of each message that reached `coder`.
fn coder_tiers(w: &World) -> Vec<Option<AccessTier>> {
    w.agent
        .sends()
        .into_iter()
        .filter_map(|c| match c {
            Call::Send { agent, tier, .. } if agent.as_str() == "coder" => Some(tier),
            _ => None,
        })
        .collect()
}

fn all_tiers() -> TierOffer {
    offer(&["plan", "edits", "auto", "bypass"], "auto")
}

// ---- the cap is pinned on the mention -------------------------------------------------------

#[tokio::test]
async fn a_mention_pins_the_cap_of_the_persons_roles_on_the_mentioned_agent() {
    let w = World::new();
    let app = app_with_roles(&w);
    let capped = working_thread(&w, &app, &person(&["planner", "chatter"]), None).await;
    assert_eq!(
        capped.job.mentioned_caps,
        BTreeMap::from([(AgentId::new("coder"), Plan)]),
        "the cap of `planner` on `coder`; `chatter` has none on `plain` and does not lift it"
    );
    let logged: Vec<_> = events(&app, &alice(), capped.id)
        .await
        .into_iter()
        .filter_map(|e| match e.body {
            EventBody::UserMessage(m) => Some(m.mentions),
            _ => None,
        })
        .collect();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0][0].tier_cap, Some(Plan), "and the log says so");

    // a person whose role on `coder` has no cap has nothing to pin
    let free = working_thread(&w, &app, &person(&["free", "chatter"]), None).await;
    assert!(free.job.mentioned_caps.is_empty(), "{:?}", free.job);
    assert!(free.job.mentioned.contains(&AgentId::new("coder")));
}

#[tokio::test]
async fn what_a_client_puts_in_a_mention_as_its_cap_is_never_read() {
    // a reference with a `tierCap` member is refused as any unknown member is
    let error = orch_app::mentions::parse(&json!([{
        "agentId": "coder", "label": "@coder", "start": 4, "end": 10, "tierCap": "bypass"
    }]))
    .unwrap_err();
    assert!(error.to_string().contains("tierCap"), "{error}");
}

// ---- the ask is sent under it ---------------------------------------------------------------

#[tokio::test]
async fn an_ask_of_a_mentioned_agent_is_sent_under_the_cap_of_the_persons_role_on_it() {
    let w = World::new();
    w.agent.set_access_tier("coder", Some(all_tiers()));
    let app = app_with_roles(&w);
    let t = working_thread(&w, &app, &person(&["planner", "chatter"]), None).await;
    let row = put(&app, &w, t.id).await;
    assert_eq!(row_cap(&row), Some(Plan), "the row carries the cap");
    let run = spawn_dispatcher(&app, fast(), "d1");
    let done = finished(&app, t.id).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(
        coder_tiers(&w),
        [Some(Plan)],
        "never the coder's own `auto`: the highest tier within the lower of its default and the cap"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn the_cap_picks_the_highest_tier_the_card_lists_within_it() {
    let w = World::new();
    // the coder skips edits and defaults to auto: within `plan` the highest it lists is `plan`
    w.agent
        .set_access_tier("coder", Some(offer(&["plan", "auto"], "auto")));
    let app = app_with_roles(&w);
    let t = working_thread(&w, &app, &person(&["planner", "chatter"]), None).await;
    put(&app, &w, t.id).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    finished(&app, t.id).await;
    assert_eq!(coder_tiers(&w), [Some(Plan)]);
    run.shutdown().await;
}

#[tokio::test]
async fn a_coder_with_no_tier_within_the_cap_is_not_asked_and_the_ask_says_why() {
    let w = World::new();
    w.agent
        .set_access_tier("coder", Some(offer(&["edits", "auto"], "auto")));
    let app = app_with_roles(&w);
    let t = working_thread(&w, &app, &person(&["planner", "chatter"]), None).await;
    let row = put(&app, &w, t.id).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let done = finished(&app, t.id).await;
    assert_eq!(done.state, AskOutcome::Failed);
    assert_eq!(
        done.error.as_deref(),
        Some("this agent has no access tier at or below 'plan', the most your role allows")
    );
    assert!(coder_tiers(&w).is_empty(), "no task was started");
    let row = eventually("the row ends", || async {
        let now = w.store.get_outbox(row.id).await.unwrap().unwrap();
        (!now.status.is_open()).then_some(now)
    })
    .await;
    assert_eq!(row.status, OutboxStatus::Dead);
    run.shutdown().await;
}

#[tokio::test]
async fn a_coder_whose_card_lists_the_extension_badly_is_not_asked_under_a_cap() {
    let w = World::new();
    w.agent.set_access_tier_malformed("coder");
    let app = app_with_roles(&w);
    let t = working_thread(&w, &app, &person(&["planner", "chatter"]), None).await;
    put(&app, &w, t.id).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    let done = finished(&app, t.id).await;
    assert_eq!(done.state, AskOutcome::Failed);
    assert!(
        done.error
            .as_deref()
            .is_some_and(|e| e.contains("parameters that cannot be read")),
        "{done:?}"
    );
    assert!(coder_tiers(&w).is_empty());
    run.shutdown().await;
}

#[tokio::test]
async fn a_coder_that_lists_no_tier_is_asked_as_it_always_was_and_the_cap_waits_for_the_day_it_does()
 {
    let w = World::new();
    let app = app_with_roles(&w);
    let t = working_thread(&w, &app, &person(&["planner", "chatter"]), None).await;
    let first = put(&app, &w, t.id).await;
    assert_eq!(row_cap(&first), Some(Plan));
    let run = spawn_dispatcher(&app, fast(), "d1");
    let done = finished(&app, t.id).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(
        coder_tiers(&w),
        [None],
        "no tier to give, the plain message"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_person_with_no_cap_on_the_coder_asks_it_under_none_and_the_parents_tier_does_not_pass_down()
 {
    let w = World::new();
    w.agent.set_access_tier("coder", Some(all_tiers()));
    w.agent.set_access_tier("plain", Some(all_tiers()));
    let app = app_with_roles(&w);
    // the thread is at `bypass`, the coder lists every tier: no cap, no tier for the ask
    let t = working_thread(&w, &app, &person(&["free", "chatter"]), Some(Bypass)).await;
    assert_eq!(t.job.tier, Some(Bypass));
    let row = put(&app, &w, t.id).await;
    assert_eq!(row_cap(&row), None);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let done = finished(&app, t.id).await;
    assert_eq!(done.state, AskOutcome::Completed);
    assert_eq!(coder_tiers(&w), [None], "ADR 0061, question 1");
    run.shutdown().await;
}

#[tokio::test]
async fn the_parents_tier_does_not_pass_down_even_when_the_person_is_capped_on_the_coder() {
    let w = World::new();
    w.agent.set_access_tier("coder", Some(all_tiers()));
    w.agent.set_access_tier("plain", Some(all_tiers()));
    let app = app_with_roles(&w);
    // the parent works under `edits` (named); the ask is under the person's cap on the coder, not
    // the parent's tier
    let t = working_thread(&w, &app, &person(&["planner", "chatter"]), Some(Edits)).await;
    put(&app, &w, t.id).await;
    let run = spawn_dispatcher(&app, fast(), "d1");
    finished(&app, t.id).await;
    assert_eq!(coder_tiers(&w), [Some(Plan)]);
    run.shutdown().await;
}

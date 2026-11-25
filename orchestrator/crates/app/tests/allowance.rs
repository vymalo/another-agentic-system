//! The month's token allowance of a role (ADR 0081, `limits.tokensPerPeriod`): a person who has
//! used it is refused before anything is written or sent, and a role with none changes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use jiff::Timestamp;
use orch_app::{
    AgentScope, AppConfig, AppError, Creation, Inbound, NewThread, Permission, Policy, RoleGrant,
};
use orch_core::{
    Actor, AgentId, AgentTarget, EventBody, ModelUsageData, ThreadId, ThreadState, TokenCounts,
    UserId, UserMessageData,
};
use orch_ports::memory::MemoryStore;
use orch_ports::{Commit, NewEvent, NewThreadRecord, Role, ThreadListing, ThreadStore};
use support::*;

fn grant(allowance: Option<u64>) -> RoleGrant {
    RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::All,
        max_tier: None,
        tokens_per_period: allowance,
    }
}

/// An application whose default role is `roles[0]`.
fn app_of(w: &World, roles: &[(&str, Option<u64>)]) -> Arc<TestApp> {
    w.app_with(AppConfig {
        policy: Policy::new(
            roles
                .iter()
                .map(|(name, allowance)| (Role::new(*name), grant(*allowance)))
                .collect::<BTreeMap<_, _>>(),
            Some(Role::new(roles[0].0)),
        )
        .unwrap(),
        ..AppConfig::default()
    })
}

/// A thread of `owner` whose log holds one call of `tokens` tokens made now: what the ledger
/// counts in the current month.
async fn spend(store: &MemoryStore, owner: &UserId, tokens: u64) -> ThreadId {
    let now = Timestamp::now();
    let id = ThreadId(uuid::Uuid::now_v7());
    let events = vec![
        NewEvent {
            at: now,
            actor: Actor::user(owner),
            body: EventBody::UserMessage(UserMessageData::new("earlier")),
            idempotency_key: None,
        },
        NewEvent {
            at: now,
            actor: Actor::agent(&AgentId::new("plain"), None),
            body: EventBody::ModelUsage(Box::new(ModelUsageData {
                job: 1,
                agent: "plain".into(),
                task: "t".into(),
                call: "c".into(),
                path: Vec::new(),
                provider: Some("openai".into()),
                model: "glm".into(),
                tokens: TokenCounts {
                    input_tokens: tokens,
                    total_tokens: tokens,
                    ..TokenCounts::default()
                },
                context_window: None,
                counted: true,
            })),
            idempotency_key: None,
        },
    ];
    store
        .create_thread(
            NewThreadRecord {
                id,
                owner: owner.clone(),
                title: "t".into(),
                description: None,
                target: AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                },
                context_id: None,
                rail_parent: None,
                child_of: None,
                now,
            },
            Commit {
                new_state: ThreadState::Done,
                job: None,
                events,
                outbox: Vec::new(),
                binding: None,
                now,
                lease: None,
                watches: Vec::new(),
                timers: Vec::new(),
                inbox: None,
                finishes_outbox: None,
                title: None,
                description: None,
                sharing: None,
                skip_unsent_delegates: false,
            },
        )
        .await
        .unwrap();
    id
}

async fn start(app: &TestApp, text: &str) -> Result<ThreadId, AppError> {
    let thread = ThreadId(uuid::Uuid::now_v7());
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

fn spent(error: &AppError, limit: u64) -> bool {
    matches!(error, AppError::AllowanceSpent { limit: l } if *l == limit)
}

#[tokio::test]
async fn a_spent_allowance_refuses_a_new_thread_and_a_message_and_nothing_reaches_an_agent() {
    let w = World::new();
    let app = app_of(&w, &[("limited", Some(100))]);
    let earlier = spend(&w.store, &alice(), 100).await;
    let before = events(&app, &alice(), earlier).await.len();

    let refused = start(&app, "one more").await.unwrap_err();
    assert!(spent(&refused, 100), "{refused:?}");
    let refused = app
        .post_message(&alice(), earlier, "and another".to_owned())
        .await
        .unwrap_err();
    assert!(spent(&refused, 100), "{refused:?}");

    // nothing written: one thread, the same log; nothing sent
    assert_eq!(events(&app, &alice(), earlier).await.len(), before);
    assert_eq!(
        app.list_threads(&alice(), ThreadListing::recent(None, 50, false))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(w.agent.sends().is_empty(), "no request reached an agent");
}

#[tokio::test]
async fn below_the_allowance_a_message_passes() {
    let w = World::new();
    let app = app_of(&w, &[("limited", Some(100))]);
    spend(&w.store, &alice(), 99).await;
    start(&app, "still fine").await.unwrap();
}

#[tokio::test]
async fn with_no_allowance_the_same_person_passes() {
    let w = World::new();
    let app = app_of(&w, &[("open", None)]);
    spend(&w.store, &alice(), 1_000_000).await;
    start(&app, "no limit set").await.unwrap();
}

#[tokio::test]
async fn another_persons_usage_is_not_counted() {
    let w = World::new();
    let app = app_of(&w, &[("limited", Some(100))]);
    spend(&w.store, &bob(), 1_000).await;
    start(&app, "mine is empty").await.unwrap();
}

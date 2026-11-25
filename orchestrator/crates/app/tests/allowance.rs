//! The token allowance of a role (ADR 0081, `limits.tokensPerPeriod`): a person who has used what
//! their roles allow in the month is refused with `allowance_spent` before anything is written or
//! sent to an agent, and a role that sets none changes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use orch_app::{
    AgentScope, AppConfig, AppError, Creation, Inbound, NewThread, Permission, Policy, RoleGrant,
};
use orch_core::{
    Actor, AgentId, AgentTarget, Classify, ErrorClass, EventBody, ModelUsageData, ThreadId,
    ThreadState, TokenCounts, UserId, UserMessageData,
};
use orch_ports::memory::Call;
use orch_ports::{Commit, NewEvent, NewThreadRecord, Role, ThreadStore};
use support::*;
use uuid::Uuid;

fn grant() -> RoleGrant {
    RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::All,
        max_tier: None,
    }
}

/// An application whose only role `member` has `allowance` tokens a month, or none.
fn app_with(w: &World, allowance: Option<u64>) -> std::sync::Arc<TestApp> {
    let allowances: BTreeMap<Role, u64> = allowance
        .into_iter()
        .map(|tokens| (Role::new("member"), tokens))
        .collect();
    let policy = Policy::new(
        BTreeMap::from([(Role::new("member"), grant())]),
        Some(Role::new("member")),
    )
    .unwrap()
    .with_token_allowances(allowances);
    w.app_with(AppConfig {
        policy,
        stream_poll: Duration::from_millis(100),
        ..AppConfig::default()
    })
}

/// A finished thread of `owner` whose log holds one call of `total` tokens made now.
async fn spend(w: &World, owner: &UserId, total: u64) -> ThreadId {
    let id = ThreadId(Uuid::now_v7());
    let now = jiff::Timestamp::now();
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
                task: "t1".into(),
                call: "c1".into(),
                path: Vec::new(),
                provider: Some("openai".into()),
                model: "glm".into(),
                tokens: TokenCounts {
                    input_tokens: total,
                    output_tokens: 0,
                    total_tokens: total,
                    ..TokenCounts::default()
                },
                context_window: None,
                counted: true,
            })),
            idempotency_key: None,
        },
    ];
    w.store
        .create_thread(
            NewThreadRecord {
                id,
                owner: owner.clone(),
                title: "earlier".into(),
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

fn requests(w: &World) -> usize {
    w.agent
        .calls()
        .iter()
        .filter(|c| matches!(c, Call::Send(_)))
        .count()
}

async fn start(app: &TestApp, user: &UserId) -> Result<Creation, AppError> {
    app.create_thread_as(
        user,
        ThreadId(Uuid::now_v7()),
        NewThread {
            title: None,
            target: target("plain"),
            text: "hello".into(),
        },
        Inbound::default(),
    )
    .await
}

fn spent(err: &AppError, limit: u64, used: u64) {
    match err {
        AppError::AllowanceSpent {
            limit: l, used: u, ..
        } => assert_eq!((*l, *u), (limit, used)),
        other => panic!("expected AllowanceSpent, got {other:?}"),
    }
    assert_eq!(err.class(), ErrorClass::Forbidden);
    assert!(err.to_string().contains(&limit.to_string()));
}

#[tokio::test]
async fn a_spent_allowance_refuses_a_new_thread_and_a_message_and_no_agent_is_called() {
    let w = World::new();
    let app = app_with(&w, Some(1_000));
    spend(&w, &alice(), 1_000).await;

    let err = start(&app, &alice()).await.unwrap_err();
    spent(&err, 1_000, 1_000);

    // an earlier thread of the person: a message to it is refused as well
    let earlier = spend(&w, &alice(), 5).await;
    let err = app
        .post_message(&alice(), earlier, "again".into())
        .await
        .unwrap_err();
    spent(&err, 1_000, 1_005);

    assert_eq!(requests(&w), 0, "nothing reached an agent");
    // nothing was written for the refused message
    let log = events(&app, &alice(), earlier).await;
    assert_eq!(
        log.iter()
            .filter(|e| matches!(e.body, EventBody::UserMessage(_)))
            .count(),
        1
    );
}

#[tokio::test]
async fn an_allowance_that_is_not_spent_changes_nothing() {
    let w = World::new();
    let app = app_with(&w, Some(1_000));
    spend(&w, &alice(), 999).await;
    assert!(matches!(
        start(&app, &alice()).await.unwrap(),
        Creation::Created { .. }
    ));
}

#[tokio::test]
async fn the_allowance_is_the_persons_own() {
    let w = World::new();
    let app = app_with(&w, Some(1_000));
    spend(&w, &alice(), 5_000).await;
    assert!(matches!(
        start(&app, &bob()).await.unwrap(),
        Creation::Created { .. }
    ));
}

#[tokio::test]
async fn with_no_allowance_set_the_same_person_passes() {
    let w = World::new();
    let app = app_with(&w, None);
    spend(&w, &alice(), 5_000_000).await;
    assert!(matches!(
        start(&app, &alice()).await.unwrap(),
        Creation::Created { .. }
    ));
    let earlier = spend(&w, &alice(), 1).await;
    app.post_message(&alice(), earlier, "again".into())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_person_with_two_roles_has_the_highest_allowance_and_one_without_lifts_it() {
    let mut roles = BTreeMap::new();
    roles.insert(Role::new("small"), grant());
    roles.insert(Role::new("big"), grant());
    roles.insert(Role::new("open"), grant());
    let policy = Policy::new(roles, None)
        .unwrap()
        .with_token_allowances(BTreeMap::from([
            (Role::new("small"), 10),
            (Role::new("big"), 100),
        ]));
    let who = |names: &[&str]| {
        let mut principal = orch_ports::Principal::of(alice());
        principal.roles = names.iter().map(|n| Role::new(*n)).collect();
        principal
    };
    assert_eq!(
        policy.access(&who(&["small"])).tokens_per_period(),
        Some(10)
    );
    assert_eq!(
        policy.access(&who(&["small", "big"])).tokens_per_period(),
        Some(100)
    );
    assert_eq!(
        policy.access(&who(&["small", "open"])).tokens_per_period(),
        None
    );
}

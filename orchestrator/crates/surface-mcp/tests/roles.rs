//! A token is a principal (ADR 0033): its entry's role decides what its holder may do over MCP,
//! exactly as it does over HTTP. A job the token may not read is "no such job"; what its role does
//! not allow is "not permitted".
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_app::{AgentScope, Permission, Policy, RoleGrant};
use orch_core::{AccessTier, TierOffer};
use orch_ports::Role;
use serde_json::json;
use support::*;

/// `chat` may read and write its own threads and use `plain` only; `admin` is the built-in one;
/// no default role, so a token without a known role is refused.
fn policy() -> Policy {
    let chat = RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::from_patterns(["plain"]),
        max_tier: None,
    };
    let mut roles = orch_app::built_in_roles();
    roles.insert(Role::new("chat"), chat);
    Policy::new(roles, None).unwrap()
}

fn options() -> Options {
    Options {
        policy: Some(policy()),
        ..Options::default()
    }
}

#[tokio::test]
async fn a_token_with_a_role_lists_and_starts_only_what_the_role_names() {
    let h = Harness::start_with(options()).await;
    let carol = h.client(CAROL_TOKEN).await;
    let out = call(&carol, "list_agents", json!({})).await;
    assert!(!out.is_error);
    let ids: Vec<&str> = out.value["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["plain"]);

    // A named agent the role does not name is refused, and nothing is written.
    let out = call(
        &carol,
        "start_job",
        json!({"text": "echo hi", "agent": "coder"}),
    )
    .await;
    assert!(out.is_error);
    assert!(out.text.starts_with("not permitted: "), "{}", out.text);
    assert!(
        out.text.contains("agent.invoke") && out.text.contains("coder"),
        "{}",
        out.text
    );
    assert!(h.threads_of(CAROL).await.is_empty());
    // With no agent named, the default is the first agent the role may invoke.
    let out = call(&carol, "start_job", json!({"text": "echo hi"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(
        h.threads_of(CAROL).await[0].target.agent_id.as_str(),
        "plain"
    );
}

#[tokio::test]
async fn a_token_without_a_known_role_and_without_a_default_is_refused_by_every_tool() {
    // Alice's entry names no role and the policy has no default role.
    let h = Harness::start_with(options()).await;
    let alice = h.client(ALICE_TOKEN).await;
    for (tool, args) in [
        ("list_agents", json!({})),
        ("start_job", json!({"text": "echo hi"})),
    ] {
        let out = call(&alice, tool, args).await;
        assert!(out.is_error, "{tool}: {}", out.text);
        assert!(
            out.text.starts_with("not permitted: "),
            "{tool}: {}",
            out.text
        );
    }
    h.assert_nothing_written().await;
}

#[tokio::test]
async fn a_token_with_the_administrator_role_reads_and_changes_only_its_own_jobs() {
    // ADR 0039: the administrator role is operational. Carol's job is no more the administrator's
    // than any other user's: for them it does not exist.
    let h = Harness::start_with(options()).await;
    let carol = h.client(CAROL_TOKEN).await;
    let root = h.client(ROOT_TOKEN).await;
    let started = call(&carol, "start_job", json!({"text": "echo hi"})).await;
    let job = started.value["job_id"].as_str().unwrap().to_owned();
    let before = h.threads_of(CAROL).await[0].last_seq;

    // Read and change: "no such job", the answer for a job nobody has, and nothing is written.
    for (tool, args) in [
        ("get_job", json!({"job_id": job})),
        ("answer", json!({"job_id": job, "text": "more"})),
        ("cancel_job", json!({"job_id": job})),
        (
            "get_job",
            json!({"job_id": "0190aaaa-0000-7000-8000-000000000123"}),
        ),
    ] {
        let out = call(&root, tool, args).await;
        assert!(out.is_error, "{tool}");
        assert_eq!(out.text, "no such job", "{tool}");
    }
    assert_eq!(h.threads_of(CAROL).await[0].last_seq, before);
    assert!(h.threads_of(ROOT).await.is_empty());
    // The administrator's own jobs are theirs.
    let own = call(&root, "start_job", json!({"text": "echo mine"})).await;
    assert!(!own.is_error, "{}", own.text);
    let job_id = own.value["job_id"].as_str().unwrap();
    assert!(
        !call(&root, "get_job", json!({"job_id": job_id}))
            .await
            .is_error
    );
    assert!(
        !call(&root, "cancel_job", json!({"job_id": job_id}))
            .await
            .is_error
    );
}

#[tokio::test]
async fn a_user_cannot_read_a_job_of_another_token_whatever_its_role() {
    let h = Harness::start_with(Options {
        policy: Some(Policy::default()),
        ..Options::default()
    })
    .await;
    // With the built-in policy Carol's `chat` is unknown and Alice has none: both are `user`.
    let carol = h.client(CAROL_TOKEN).await;
    let alice = h.client(ALICE_TOKEN).await;
    let started = call(&carol, "start_job", json!({"text": "echo hi"})).await;
    assert!(!started.is_error, "{}", started.text);
    let out = call(
        &alice,
        "get_job",
        json!({"job_id": started.value["job_id"]}),
    )
    .await;
    assert!(out.is_error);
    assert_eq!(out.text, "no such job");
}

/// ADR 0061: a job started over MCP names no tier (the surface cannot), so a role with a cap is
/// given it, not the agent's default.
#[tokio::test]
async fn start_job_under_a_capped_role_runs_at_the_cap_not_at_the_agents_default() {
    let capped = RoleGrant {
        permissions: BTreeSet::from([
            Permission::AgentRead,
            Permission::AgentInvoke,
            Permission::ThreadRead,
            Permission::ThreadWrite,
        ]),
        agents: AgentScope::from_patterns(["plain"]),
        max_tier: Some(AccessTier::Edits),
    };
    let mut roles = orch_app::built_in_roles();
    roles.insert(Role::new("chat"), capped);
    let h = Harness::start_with(Options {
        policy: Some(Policy::new(roles, None).unwrap()),
        ..Options::default()
    })
    .await;
    h.agent.set_access_tier(
        "plain",
        Some(
            TierOffer::from_params(
                &json!({"tiers": ["plan", "edits", "auto", "bypass"], "default": "auto"}),
            )
            .unwrap(),
        ),
    );
    let carol = h.client(CAROL_TOKEN).await;
    let out = call(&carol, "start_job", json!({"text": "echo hi"})).await;
    assert!(!out.is_error, "{}", out.text);
    let thread = h.threads_of(CAROL).await[0].id;
    let events = h
        .app
        .list_events(
            &orch_ports::Principal {
                roles: [Role::new("chat")].into(),
                ..orch_ports::Principal::of(orch_core::UserId::new(CAROL))
            },
            thread,
            0,
            100,
        )
        .await
        .unwrap();
    let tiers: Vec<_> = events
        .iter()
        .filter_map(|e| match &e.body {
            orch_core::EventBody::UserMessage(m) => Some(m.tier),
            _ => None,
        })
        .collect();
    assert_eq!(tiers, [Some(AccessTier::Edits)]);
    // and the agent was sent it
    eventually_sent(&h, AccessTier::Edits).await;
}

async fn eventually_sent(h: &Harness, tier: AccessTier) {
    for _ in 0..200 {
        let sent: Vec<_> = h
            .agent
            .sends()
            .into_iter()
            .filter_map(|call| match call {
                orch_ports::memory::Call::Send { tier, .. } => Some(tier),
                _ => None,
            })
            .collect();
        if !sent.is_empty() {
            assert_eq!(sent, [Some(tier)]);
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("nothing was sent to the agent");
}

//! The agents of the registry on the AG-UI routes (ADR 0022): an agent the platform lists can be
//! run and described from the moment it is listed, one it no longer lists is a 404, and a
//! registry that cannot say whether an agent exists is a 503 with `Retry-After`, never a 404.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_app::{Policy, RoleGrant};
use orch_core::{AgentId, AgentSource};
use orch_ports::{AgentAudience, AgentEndpoint, RegistryEntry, Role};
use support::*;

fn platform_agent(id: &str) -> RegistryEntry {
    RegistryEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.agents.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: "Helper".to_owned(),
        tags: vec!["writing".to_owned()],
        origin: AgentSource::Registry,
        audience: AgentAudience::Everyone,
    }
}

#[tokio::test]
async fn an_agent_is_described_and_run_from_the_moment_the_registry_lists_it() {
    let h = Harness::start().await;
    let thread = new_thread_id();
    let body = input(&thread, "run-a", &[("msg-1", "echo hi")]);
    let r = h.refused("helper", Some(ALICE), &body).await;
    assert_eq!(r.status, 404, "not listed yet");
    let r = h.get("/agui/agents/helper/capabilities", Some(ALICE)).await;
    assert_eq!(r.status, 404);

    h.registry.add(platform_agent("helper"));
    let r = h.get("/agui/agents/helper/capabilities", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    assert_eq!(r.json()["identity"]["name"], "Helper");
    let frames = h.run("helper", ALICE, &body).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    h.wait_state(ALICE, &thread, "done").await;

    // And the platform takes it away: the next run says so.
    h.registry.remove(&AgentId::new("helper"));
    let other = input(&new_thread_id(), "run-b", &[("msg-1", "echo hi")]);
    assert_eq!(h.refused("helper", Some(ALICE), &other).await.status, 404);
}

#[tokio::test]
async fn a_registry_that_cannot_say_is_a_503_and_the_static_agents_still_run() {
    let h = Harness::start().await;
    h.registry.add(platform_agent("helper"));
    h.registry.set_down(true);

    for agent in ["helper", "nobody"] {
        let body = input(&new_thread_id(), "run-a", &[("msg-1", "echo hi")]);
        let r = h.refused(agent, Some(ALICE), &body).await;
        assert_eq!(
            r.status, 503,
            "{agent}: not a 404 while the registry is down"
        );
        assert!(r.headers.contains_key("retry-after"), "{agent}");
        assert_eq!(
            r.problem(503)["detail"],
            "the agent registry is unreachable",
            "{agent}"
        );
        let r = h
            .get(&format!("/agui/agents/{agent}/capabilities"), Some(ALICE))
            .await;
        assert_eq!(r.status, 503, "{agent}");
        assert!(r.headers.contains_key("retry-after"), "{agent}");
    }

    // The deployment's own agents do not depend on the registry.
    let body = input(&new_thread_id(), "run-b", &[("msg-1", "echo hi")]);
    let frames = h.run("plain", ALICE, &body).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");

    h.registry.set_down(false);
    let r = h.get("/agui/agents/helper/capabilities", Some(ALICE)).await;
    assert_eq!(r.status, 200);
}

/// `platform_agent(id)` for the people whose roles claim holds `agent.use:<id>`.
fn platform_agent_for(id: &str, audience: AgentAudience) -> RegistryEntry {
    RegistryEntry {
        audience,
        ..platform_agent(id)
    }
}

/// The built-in roles, and an administrator by default: in the proxy-header mode nobody carries
/// a roles claim, so the default role is how a test person is an administrator.
fn everyone_is_an_administrator() -> Policy {
    Policy::new(
        std::collections::BTreeMap::from([
            (Role::new("user"), RoleGrant::user()),
            (Role::new("admin"), RoleGrant::admin()),
        ]),
        Some(Role::new("admin")),
    )
    .unwrap()
}

#[tokio::test]
async fn an_agent_the_audience_hides_is_not_described_and_not_run_it_is_no_such_agent() {
    let h = Harness::start().await;
    h.registry.add(platform_agent("open"));
    h.registry
        .add(platform_agent_for("fresh", AgentAudience::AdminsOnly));
    h.registry.add(platform_agent_for(
        "theirs",
        AgentAudience::from_values(["agent.use:theirs"]),
    ));

    for agent in ["fresh", "theirs"] {
        let hidden = h
            .get(&format!("/agui/agents/{agent}/capabilities"), Some(ALICE))
            .await;
        let nowhere = h
            .get("/agui/agents/nowhere/capabilities", Some(ALICE))
            .await;
        assert_eq!(hidden.status, 404, "{agent}");
        assert_eq!(hidden.json(), nowhere.json(), "{agent}: the same problem");

        let body = input(&new_thread_id(), "run-a", &[("msg-1", "echo hi")]);
        let hidden = h.refused(agent, Some(ALICE), &body).await;
        let nowhere = h.refused("nowhere", Some(ALICE), &body).await;
        assert_eq!(hidden.status, 404, "{agent}");
        assert_eq!(hidden.json(), nowhere.json(), "{agent}: the same problem");
        assert_eq!(hidden.json()["detail"], "no such agent");
    }
    // nothing was started for it
    let r = h.get("/agui/agents/open/capabilities", Some(ALICE)).await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn an_administrator_describes_and_runs_the_agent_that_has_no_audience() {
    let h = Harness::start_with_policy(everyone_is_an_administrator()).await;
    h.registry
        .add(platform_agent_for("fresh", AgentAudience::AdminsOnly));
    let r = h.get("/agui/agents/fresh/capabilities", Some(ALICE)).await;
    assert_eq!(r.status, 200);
    let body = input(&new_thread_id(), "run-a", &[("msg-1", "echo hi")]);
    let frames = h.run("fresh", ALICE, &body).await.all().await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
}

#[tokio::test]
async fn a_message_to_a_thread_whose_agent_the_audience_took_away_is_a_403_not_a_404() {
    let h = Harness::start().await;
    h.registry.add(platform_agent("helper"));
    let thread = new_thread_id();
    let frames = h
        .run(
            "helper",
            ALICE,
            &input(&thread, "run-a", &[("msg-1", "echo hi")]),
        )
        .await
        .all()
        .await;
    assert_eq!(frames.last().unwrap().kind(), "RUN_FINISHED");
    h.wait_state(ALICE, &thread, "done").await;

    // the platform narrows the audience to somebody else
    h.registry.add(platform_agent_for(
        "helper",
        AgentAudience::from_values(["agent.use:somebody-else"]),
    ));

    // the thread is theirs, so it is read-only, as when `agent.invoke` stops covering its agent
    let more = input(
        &thread,
        "run-b",
        &[("msg-1", "echo hi"), ("msg-2", "echo more")],
    );
    let r = h.refused("helper", Some(ALICE), &more).await;
    assert_eq!(r.status, 403, "{}", r.json());
    assert_eq!(
        r.json()["detail"],
        "your roles do not grant agent.invoke for the agent helper"
    );
    // while a new thread on that agent is no such agent
    let fresh = input(&new_thread_id(), "run-c", &[("msg-1", "echo hi")]);
    assert_eq!(h.refused("helper", Some(ALICE), &fresh).await.status, 404);
    // and somebody else's thread id on it is not found either
    let r = h.refused("helper", Some("bob@example.com"), &more).await;
    assert_eq!(r.status, 404);
}

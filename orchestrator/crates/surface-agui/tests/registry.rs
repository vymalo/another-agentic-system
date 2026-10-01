//! The agents of the registry on the AG-UI routes (ADR 0022): an agent the platform lists can be
//! run and described from the moment it is listed, one it no longer lists is a 404, and a
//! registry that cannot say whether an agent exists is a 503 with `Retry-After`, never a 404.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_core::{AgentId, AgentSource};
use orch_ports::{AgentEndpoint, RegistryEntry};
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

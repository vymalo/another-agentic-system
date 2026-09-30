//! The gate's configuration layers (ADR 0018): the rules that put the deployment's gate, an
//! agent's entry and a thread's request on top of each other, and their use when a thread is
//! created. No dispatcher runs.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;

use orch_app::{
    AppConfig, AppError, Creation, GateError, GateLayer, GateRules, Inbound, Layer, NewThread,
    THREAD_GATE_KEY, known_sources, pending_reason,
};
use orch_core::{AgentId, CheckSource, Classify, ErrorClass, GatePolicy, ThreadId};
use serde_json::json;
use support::*;

fn layer(value: serde_json::Value) -> GateLayer {
    GateLayer::from_json(&value).unwrap().unwrap()
}

fn agent(id: &str) -> AgentId {
    AgentId::new(id)
}

/// The rules of a build that honours CI and the verifier too, for the rules those slices add.
fn everything() -> GateRules {
    GateRules::new(10).honouring(CheckSource::ALL)
}

#[test]
fn this_build_honours_agent_checks_and_nothing_else() {
    let rules = GateRules::default();
    assert!(rules.honours(CheckSource::AgentChecks));
    assert!(!rules.honours(CheckSource::Ci));
    assert!(!rules.honours(CheckSource::Verifier));
    // Every source that is not honoured says which slice enables it; the one that is says
    // nothing.
    assert!(pending_reason(CheckSource::AgentChecks).is_none());
    assert!(pending_reason(CheckSource::Ci).unwrap().contains("slice 5"));
    assert!(pending_reason(CheckSource::Ci).unwrap().contains("slice 6"));
    assert!(
        pending_reason(CheckSource::Verifier)
            .unwrap()
            .contains("slice 10")
    );
    assert_eq!(known_sources(), "ci, agent-checks, verifier");
}

#[test]
fn ci_and_the_verifier_are_refused_in_every_layer_and_the_message_names_the_slice() {
    let rules = GateRules::default();
    let above = GatePolicy::default();
    let refused = [
        (json!({"require": ["ci"]}), "slice 5"),
        (json!({"require": ["agent-checks", "ci"]}), "slice 6"),
        (json!({"require": ["verifier"]}), "slice 10"),
        (json!({"verifier": "reviewer"}), "slice 10"),
        (json!({"ci": {"required": ["build"]}}), "slice 5"),
        (json!({"ci": {"timeoutSecs": 60}}), "slice 6"),
    ];
    let layers = [
        Layer::Deployment,
        Layer::Target(agent("coder")),
        Layer::Thread,
    ];
    for at in &layers {
        for (value, slice) in &refused {
            let err = rules.apply(&above, &layer(value.clone()), at).unwrap_err();
            assert!(matches!(err, GateError::Unavailable { .. }), "{at} {value}");
            let message = err.to_string();
            assert!(message.contains(slice), "{at} {value}: {message}");
            assert!(
                message.contains("only agent-checks can be required"),
                "{message}"
            );
        }
    }
    // The one source this build honours passes in every layer.
    for at in &layers {
        let policy = rules
            .apply(&above, &layer(json!({"require": ["agent-checks"]})), at)
            .unwrap();
        assert_eq!(policy.require.len(), 1);
        assert!(policy.requires(CheckSource::AgentChecks));
    }
}

#[test]
fn a_layer_may_add_sources_and_change_attempts_within_the_cap() {
    let rules = everything();
    let deployment = GatePolicy::requiring([CheckSource::AgentChecks]);
    let target = rules
        .apply(
            &deployment,
            &layer(json!({"require": ["agent-checks", "ci"], "maxAttempts": 5})),
            &Layer::Target(agent("coder")),
        )
        .unwrap();
    assert_eq!(
        target.require,
        [CheckSource::Ci, CheckSource::AgentChecks].into()
    );
    assert_eq!(target.max_attempts, 5);

    let thread = rules
        .apply(
            &target,
            &layer(json!({"require": ["ci", "agent-checks"], "maxAttempts": 10})),
            &Layer::Thread,
        )
        .unwrap();
    assert_eq!(thread.max_attempts, 10, "up to the cap");
    let fewer = rules
        .apply(&target, &layer(json!({"maxAttempts": 1})), &Layer::Thread)
        .unwrap();
    assert_eq!(fewer.max_attempts, 1, "and down to one");
    assert_eq!(fewer.require, target.require, "an absent list inherits");
}

#[test]
fn attempts_outside_one_to_the_cap_are_refused() {
    let rules = GateRules::new(4);
    for value in [0, 5, 100] {
        let err = rules
            .apply(
                &GatePolicy::default(),
                &layer(json!({"maxAttempts": value})),
                &Layer::Thread,
            )
            .unwrap_err();
        assert_eq!(
            err,
            GateError::Attempts {
                layer: Layer::Thread,
                value,
                cap: 4
            }
        );
        assert!(err.to_string().contains("1..=4"), "{err}");
    }
    assert!(
        rules
            .apply(
                &GatePolicy::default(),
                &layer(json!({"maxAttempts": 4})),
                &Layer::Thread
            )
            .is_ok()
    );
}

#[test]
fn a_layer_may_not_remove_a_source_the_layer_above_requires() {
    let rules = everything();
    let above = GatePolicy::requiring([CheckSource::AgentChecks, CheckSource::Ci]);
    for value in [
        json!({"require": []}),
        json!({"require": ["ci"]}),
        json!({"require": ["agent-checks"]}),
    ] {
        let err = rules
            .apply(&above, &layer(value.clone()), &Layer::Thread)
            .unwrap_err();
        assert!(matches!(err, GateError::Removes { .. }), "{value}: {err}");
        assert!(err.to_string().contains("may add sources"), "{err}");
    }
    // The same for a target against the deployment.
    let err = rules
        .apply(
            &above,
            &layer(json!({"require": []})),
            &Layer::Target(agent("coder")),
        )
        .unwrap_err();
    assert!(err.to_string().contains("coder"), "{err}");
}

#[test]
fn a_thread_cannot_choose_the_verifier_or_the_ci_settings() {
    let rules = everything();
    for (value, setting) in [
        (json!({"verifier": "reviewer"}), "verifier"),
        (json!({"ci": {"required": ["build"]}}), "ci"),
    ] {
        let err = rules
            .apply(&GatePolicy::default(), &layer(value), &Layer::Thread)
            .unwrap_err();
        assert_eq!(
            err,
            GateError::NotPerThread {
                layer: Layer::Thread,
                setting
            }
        );
    }
    // A target may.
    let policy = rules
        .apply(
            &GatePolicy::default(),
            &layer(
                json!({"require": ["verifier", "ci"], "verifier": "reviewer",
                          "ci": {"required": ["build", "test"], "timeoutSecs": 120}}),
            ),
            &Layer::Target(agent("coder")),
        )
        .unwrap();
    assert_eq!(policy.verifier, Some(agent("reviewer")));
    assert_eq!(policy.ci.required.len(), 2);
    assert_eq!(policy.ci.timeout.as_secs(), 120);
    assert!(
        rules
            .apply(
                &GatePolicy::default(),
                &layer(json!({"ci": {"timeoutSecs": 0}})),
                &Layer::Deployment
            )
            .is_err()
    );
}

#[test]
fn a_layer_is_read_strictly() {
    for bad in [
        json!({"unknown": 1}),
        json!({"require": ["agent_checks"]}),
        json!({"require": ["nonsense"]}),
        json!({"require": "ci"}),
        json!({"maxAttempts": "three"}),
        json!({"maxAttempts": -1}),
        json!({"ci": {"require": ["build"]}}),
        json!("agent-checks"),
        json!([1]),
    ] {
        let err = GateLayer::from_json(&bad).unwrap_err();
        assert!(matches!(err, GateError::Malformed(_)), "{bad}");
    }
    let message = GateLayer::from_json(&json!({"require": ["nonsense"]}))
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("one of: ci, agent-checks, verifier"),
        "{message}"
    );
    // null is no layer; an empty object is a layer that says nothing.
    assert_eq!(GateLayer::from_json(&json!(null)).unwrap(), None);
    assert_eq!(
        GateLayer::from_json(&json!({})).unwrap(),
        Some(GateLayer::default())
    );
    assert_eq!(THREAD_GATE_KEY, "vymalo.gate");
}

#[test]
fn the_verifier_must_be_another_configured_agent() {
    let rules = everything();
    let agents = directory(); // coder, plain
    let with = |verifier: Option<&str>, required: bool| {
        let mut policy = if required {
            GatePolicy::requiring([CheckSource::Verifier])
        } else {
            GatePolicy::default()
        };
        policy.verifier = verifier.map(agent);
        policy
    };
    let check = |policy: &GatePolicy, target: &str| {
        rules.check_verifier(policy, &agent(target), &agents, &Layer::Deployment)
    };
    assert!(check(&with(Some("plain"), true), "coder").is_ok());
    assert!(check(&with(None, false), "coder").is_ok());
    assert!(matches!(
        check(&with(None, true), "coder"),
        Err(GateError::NoVerifier { .. })
    ));
    assert!(matches!(
        check(&with(Some("ghost"), true), "coder"),
        Err(GateError::UnknownVerifier { .. })
    ));
    assert!(
        matches!(
            check(&with(Some("ghost"), false), "coder"),
            Err(GateError::UnknownVerifier { .. })
        ),
        "a typo is caught even when the verifier is not required yet"
    );
    assert!(matches!(
        check(&with(Some("coder"), true), "coder"),
        Err(GateError::SelfVerifier { .. })
    ));
}

#[test]
fn startup_validation_resolves_every_agent() {
    let rules = everything();
    let agents = directory();
    let deployment = GatePolicy::requiring([CheckSource::AgentChecks]);
    let mut targets = BTreeMap::new();
    targets.insert(
        agent("coder"),
        layer(json!({"require": ["agent-checks", "verifier"], "verifier": "plain"})),
    );
    assert!(rules.validate(&deployment, &targets, &agents).is_ok());

    // `plain` verifies `coder`, and it would have to verify itself as the deployment's verifier.
    let mut with_verifier = deployment.clone();
    with_verifier.verifier = Some(agent("plain"));
    let err = rules
        .validate(&with_verifier, &targets, &agents)
        .unwrap_err();
    assert!(matches!(err, GateError::SelfVerifier { .. }), "{err}");

    targets.insert(agent("coder"), layer(json!({"verifier": "ghost"})));
    let err = rules.validate(&deployment, &targets, &agents).unwrap_err();
    assert!(err.to_string().contains("ghost"), "{err}");

    // The target that drops the deployment's source is refused, naming the agent.
    targets.insert(agent("coder"), layer(json!({"require": []})));
    let err = rules.validate(&deployment, &targets, &agents).unwrap_err();
    assert!(err.to_string().contains("coder"), "{err}");

    // And what this build cannot honour never gets as far as the references.
    let err = GateRules::default()
        .validate(
            &deployment,
            &BTreeMap::from([(agent("plain"), layer(json!({"verifier": "coder"})))]),
            &agents,
        )
        .unwrap_err();
    assert!(err.to_string().contains("slice 10"), "{err}");
}

// ---- a thread is created under the resolved gate ----------------------------------------

fn new_thread(agent_id: &str) -> NewThread {
    NewThread {
        title: None,
        target: target(agent_id),
        text: "work".to_owned(),
    }
}

async fn create_with(
    app: &TestApp,
    agent_id: &str,
    request: Option<GateLayer>,
) -> Result<orch_core::ThreadRecord, AppError> {
    let id = ThreadId(uuid::Uuid::now_v7());
    let inbound = Inbound {
        gate: request,
        ..Inbound::default()
    };
    match app
        .create_thread_as(&alice(), id, new_thread(agent_id), inbound)
        .await?
    {
        Creation::Created { thread, .. } => Ok(thread),
        Creation::Exists => panic!("fresh id"),
    }
}

#[tokio::test]
async fn a_thread_starts_under_the_deployment_then_the_target_then_the_request() {
    let w = World::new();
    let app = w.app_with(AppConfig {
        gate: GatePolicy::default(),
        target_gates: BTreeMap::from([(
            agent("coder"),
            layer(json!({"require": ["agent-checks"], "maxAttempts": 2})),
        )]),
        ..AppConfig::default()
    });

    // Nothing configured for `plain`: today's behaviour, the default job.
    let plain = create_with(&app, "plain", None).await.unwrap();
    assert_eq!(plain.job, orch_core::Job::default());

    // The target's entry applies to `coder`.
    let coder = create_with(&app, "coder", None).await.unwrap();
    assert_eq!(coder.job.gate.require, [CheckSource::AgentChecks].into());
    assert_eq!(coder.job.gate.max_attempts, 2);
    assert_eq!(coder.job.task.as_deref(), Some("work"));

    // The request changes attempts, up to the cap, and the thread keeps the result.
    let asked = create_with(&app, "coder", Some(layer(json!({"maxAttempts": 7}))))
        .await
        .unwrap();
    assert_eq!(asked.job.gate.max_attempts, 7);
    assert_eq!(asked.job.gate.require, [CheckSource::AgentChecks].into());
    let stored = app.get_thread(&alice(), asked.id).await.unwrap();
    assert_eq!(stored.job.gate, asked.job.gate);

    // A request may turn the gate on for an agent that has none.
    let on = create_with(
        &app,
        "plain",
        Some(layer(json!({"require": ["agent-checks"]}))),
    )
    .await
    .unwrap();
    assert!(on.job.gate.requires(CheckSource::AgentChecks));
    assert_eq!(on.job.gate.max_attempts, 3, "the default attempts");
}

#[tokio::test]
async fn a_request_that_weakens_or_overreaches_is_a_400_and_writes_nothing() {
    let w = World::new();
    let app = w.app_with(AppConfig {
        gate: GatePolicy::default(),
        target_gates: BTreeMap::from([(
            agent("coder"),
            layer(json!({"require": ["agent-checks"]})),
        )]),
        ..AppConfig::default()
    });
    let refused = [
        (json!({"require": []}), "may add sources"),
        (json!({"maxAttempts": 11}), "1..=10"),
        (json!({"maxAttempts": 0}), "1..=10"),
        (json!({"require": ["agent-checks", "ci"]}), "slice 5"),
        (json!({"require": ["verifier"]}), "slice 10"),
        (json!({"verifier": "plain"}), "slice 10"),
    ];
    for (request, why) in refused {
        let err = create_with(&app, "coder", Some(layer(request.clone())))
            .await
            .unwrap_err();
        assert_eq!(err.class(), ErrorClass::Invalid, "{request}");
        assert!(err.to_string().contains(why), "{request}: {err}");
    }
    assert!(
        app.list_threads(&alice(), None, 10)
            .await
            .unwrap()
            .is_empty(),
        "a refused request creates no thread"
    );
}

#[tokio::test]
async fn a_fault_in_the_targets_own_entry_is_not_the_callers() {
    let w = World::new();
    let app = w.app_with(AppConfig {
        target_gates: BTreeMap::from([(agent("coder"), layer(json!({"require": ["ci"]})))]),
        ..AppConfig::default()
    });
    let err = create_with(&app, "coder", None).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Internal, "{err}");
}

//! The gate's configuration layers (ADR 0018): the rules that put the deployment's gate, an
//! agent's entry and a thread's request on top of each other, and their use when a thread is
//! created. No dispatcher runs.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;

use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, AppError, Creation, GateError, GateLayer,
    GateRules, Inbound, Layer, NewThread, THREAD_GATE_KEY, known_sources, pending_reason,
};
use orch_core::{AgentId, CheckSource, Classify, ErrorClass, GatePolicy, ThreadId};
use orch_ports::{AgentEndpoint, PortSet, SystemClock, ThreadListing};
use serde_json::json;
use support::*;

fn layer(value: serde_json::Value) -> GateLayer {
    GateLayer::from_json(&value).unwrap().unwrap()
}

fn agent(id: &str) -> AgentId {
    AgentId::new(id)
}

/// The rules of a build that honours CI too, for the rules that slice adds (the verifier is
/// honoured by every build since slice 10).
fn everything() -> GateRules {
    GateRules::new(10).honouring(CheckSource::ALL)
}

#[test]
fn this_build_honours_every_source() {
    let rules = GateRules::default();
    assert!(rules.honours(CheckSource::AgentChecks));
    assert!(
        rules.honours(CheckSource::Ci),
        "slice 6 built the CI webhook"
    );
    assert!(
        rules.honours(CheckSource::Verifier),
        "slice 10 built the verifier"
    );
    // A source that is not built says which slice enables it; every one is built now.
    for source in CheckSource::ALL {
        assert!(pending_reason(source).is_none(), "{source:?}");
    }
    assert_eq!(known_sources(), "ci, agent-checks, verifier");
}

#[test]
fn a_source_the_build_does_not_honour_is_refused_in_every_layer_with_its_reason() {
    // `honouring` is how a build (or a test) with fewer sources says so: the refusal names what is
    // left, and the source it would need.
    let rules = GateRules::default().honouring([CheckSource::AgentChecks]);
    // Names for `ci` are in place above (a policy may name checks without requiring them), so a
    // thread can add the source.
    let mut above = GatePolicy::default();
    above.ci.required = ["build".to_owned()].into();
    let refused = [
        json!({"require": ["ci"]}),
        json!({"require": ["agent-checks", "verifier"]}),
        json!({"verifier": "reviewer"}),
        json!({"ci": {"required": ["build"]}}),
    ];
    for at in [
        Layer::Deployment,
        Layer::Target(agent("coder")),
        Layer::Thread,
    ] {
        for value in &refused {
            let err = rules.apply(&above, &layer(value.clone()), &at).unwrap_err();
            assert!(matches!(err, GateError::Unavailable { .. }), "{at} {value}");
            let message = err.to_string();
            assert!(
                message.contains("only agent-checks can be required"),
                "{message}"
            );
        }
    }
}

#[test]
fn the_sources_this_build_honours_pass_in_every_layer_that_may_say_them() {
    let rules = GateRules::default();
    // Names for `ci` are in place above (a policy may name checks without requiring them), so a
    // thread can add the source.
    let mut above = GatePolicy::default();
    above.ci.required = ["build".to_owned()].into();
    let layers = [
        Layer::Deployment,
        Layer::Target(agent("coder")),
        Layer::Thread,
    ];
    for at in &layers {
        for source in ["agent-checks", "ci", "verifier"] {
            let policy = rules
                .apply(&above, &layer(json!({"require": [source]})), at)
                .unwrap();
            assert_eq!(policy.require.len(), 1, "{at} {source}");
            assert_eq!(policy.require.iter().next().unwrap().config_name(), source);
        }
    }
    // The verifier setting passes where a thread is not the one choosing it.
    for at in [Layer::Deployment, Layer::Target(agent("coder"))] {
        let policy = rules
            .apply(
                &above,
                &layer(json!({"require": ["verifier"], "verifier": "reviewer"})),
                &at,
            )
            .unwrap();
        assert_eq!(policy.verifier, Some(agent("reviewer")));
    }
    let err = rules
        .apply(
            &above,
            &layer(json!({"verifier": "reviewer"})),
            &Layer::Thread,
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            GateError::NotPerThread {
                setting: "verifier",
                ..
            }
        ),
        "{err}"
    );
}

#[test]
fn the_ci_settings_are_honoured_for_a_deployment_or_a_target_and_never_per_thread() {
    let rules = GateRules::default();
    let above = GatePolicy::requiring([CheckSource::Ci]);
    let settings = layer(json!({"ci": {"required": ["build", "lint"], "timeoutSecs": 60}}));
    for at in [Layer::Deployment, Layer::Target(agent("coder"))] {
        let policy = rules.apply(&above, &settings, &at).unwrap();
        assert_eq!(
            policy.ci.required,
            ["build".to_owned(), "lint".to_owned()].into()
        );
        assert_eq!(policy.ci.timeout, jiff::SignedDuration::from_secs(60));
    }
    let err = rules.apply(&above, &settings, &Layer::Thread).unwrap_err();
    assert!(
        matches!(err, GateError::NotPerThread { setting: "ci", .. }),
        "{err}"
    );
    let err = rules
        .apply(
            &above,
            &layer(json!({"ci": {"timeoutSecs": 0}})),
            &Layer::Deployment,
        )
        .unwrap_err();
    assert!(matches!(err, GateError::Timeout { .. }), "{err}");
}

#[test]
fn a_layer_may_add_sources_and_change_attempts_within_the_cap() {
    let rules = everything();
    let deployment = GatePolicy::requiring([CheckSource::AgentChecks]);
    let target = rules
        .apply(
            &deployment,
            &layer(json!({
                "require": ["agent-checks", "ci"],
                "maxAttempts": 5,
                "ci": {"required": ["build"]}
            })),
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
    assert!(
        check(&with(Some("coder"), false), "coder").is_ok(),
        "the verifier's own gate does not require the verifier"
    );
}

#[test]
fn another_id_for_the_same_endpoint_is_no_second_pair_of_eyes() {
    let rules = everything();
    let entry = |id: &str, url: &str, bearer: Option<&str>| AgentEntry {
        endpoint: AgentEndpoint::a2a(AgentId::new(id), url, bearer.map(str::to_owned)),
        name: id.to_owned(),
    };
    let card = "https://coder.example.com/.well-known/agent-card.json";
    let agents = AgentDirectory::new(vec![
        entry("coder", card, Some("one")),
        // the same card behind another id, another token and a stray slash
        entry("alias", &format!("{card}/"), Some("two")),
        entry(
            "reviewer",
            "https://reviewer.example.com/.well-known/agent-card.json",
            None,
        ),
        AgentEntry {
            endpoint: AgentEndpoint::local(AgentId::new("local-a"), "adam"),
            name: "a".into(),
        },
        AgentEntry {
            endpoint: AgentEndpoint::local(AgentId::new("local-b"), "adam"),
            name: "b".into(),
        },
    ]);
    let check = |verifier: &str, target: &str| {
        let mut policy = GatePolicy::requiring([CheckSource::Verifier]);
        policy.verifier = Some(agent(verifier));
        rules.check_verifier(&policy, &agent(target), &agents, &Layer::Deployment)
    };
    let err = check("alias", "coder").unwrap_err();
    assert!(matches!(err, GateError::SelfVerifier { .. }), "{err}");
    assert!(
        err.to_string().contains("\"alias\" is the same endpoint"),
        "the message names the alias: {err}"
    );
    assert!(matches!(
        check("coder", "alias"),
        Err(GateError::SelfVerifier { .. })
    ));
    assert!(check("reviewer", "coder").is_ok());
    assert!(
        check("local-b", "local-a").is_ok(),
        "two local agents of one kind are told apart by id"
    );
}

#[test]
fn startup_validation_resolves_every_agent() {
    let rules = everything();
    let agents = directory(); // coder, plain
    let deployment = GatePolicy::requiring([CheckSource::AgentChecks]);
    let mut targets = BTreeMap::new();
    targets.insert(
        agent("coder"),
        layer(json!({"require": ["agent-checks", "verifier"], "verifier": "plain"})),
    );
    assert!(rules.validate(&deployment, &targets, &agents).is_ok());

    // A verifier that is set but not required by the agent's own gate is no self-verification:
    // `ORCH_VERIFIER=plain` must not fail startup when it visits the entry of `plain` itself.
    let mut named = deployment.clone();
    named.verifier = Some(agent("plain"));
    assert!(rules.validate(&named, &BTreeMap::new(), &agents).is_ok());
    assert!(rules.validate(&named, &targets, &agents).is_ok());

    // When the deployment requires the verifier, `plain` would verify itself...
    let mut required = named.clone();
    required.require.insert(CheckSource::Verifier);
    let err = rules
        .validate(&required, &BTreeMap::new(), &agents)
        .unwrap_err();
    assert!(matches!(err, GateError::SelfVerifier { .. }), "{err}");
    assert!(err.to_string().contains("leaves out `verifier`"), "{err}");
    // ... unless its own entry leaves the verifier out for itself, the one removal allowed.
    let own = BTreeMap::from([(agent("plain"), layer(json!({"require": ["agent-checks"]})))]);
    assert!(rules.validate(&required, &own, &agents).is_ok());
    let policy = rules
        .for_target(&required, &agent("plain"), own.get(&agent("plain")))
        .unwrap();
    assert!(!policy.requires(CheckSource::Verifier) && policy.requires(CheckSource::AgentChecks));
    // Nothing else may go, for the verifier or for anyone else.
    for (who, gate) in [
        ("plain", json!({"require": []})),
        ("coder", json!({"require": ["agent-checks"]})),
    ] {
        let targets = BTreeMap::from([(agent(who), layer(gate))]);
        let err = rules.validate(&required, &targets, &agents).unwrap_err();
        assert!(matches!(err, GateError::Removes { .. }), "{who}: {err}");
    }

    targets.insert(agent("coder"), layer(json!({"verifier": "ghost"})));
    let err = rules.validate(&deployment, &targets, &agents).unwrap_err();
    assert!(err.to_string().contains("ghost"), "{err}");

    // The target that drops the deployment's source is refused, naming the agent.
    targets.insert(agent("coder"), layer(json!({"require": []})));
    let err = rules.validate(&deployment, &targets, &agents).unwrap_err();
    assert!(err.to_string().contains("coder"), "{err}");

    // And what this build cannot honour never gets as far as the references.
    let err = GateRules::default()
        .honouring([CheckSource::AgentChecks, CheckSource::Verifier])
        .validate(
            &deployment,
            &BTreeMap::from([(
                agent("plain"),
                layer(json!({"ci": {"required": ["build"]}})),
            )]),
            &agents,
        )
        .unwrap_err();
    assert!(err.to_string().contains("ci settings"), "{err}");
}

#[test]
fn the_ci_checks_of_a_layer_add_to_those_above_and_never_replace_them() {
    let rules = everything();
    let deployment = rules
        .apply(
            &GatePolicy::default(),
            &layer(json!({"require": ["ci"], "ci": {"required": ["build", "test"]}})),
            &Layer::Deployment,
        )
        .unwrap();
    let target = rules
        .apply(
            &deployment,
            &layer(json!({"ci": {"required": []}})),
            &Layer::Target(agent("coder")),
        )
        .unwrap();
    assert_eq!(
        target.ci.required,
        ["build".to_owned(), "test".to_owned()].into(),
        "an empty list does not weaken the deployment's"
    );
    let more = rules
        .apply(
            &target,
            &layer(json!({"ci": {"required": ["lint"], "timeoutSecs": 30}})),
            &Layer::Target(agent("coder")),
        )
        .unwrap();
    assert_eq!(more.ci.required.len(), 3);
    assert_eq!(more.ci.timeout.as_secs(), 30);
}

#[test]
fn both_spellings_of_a_source_are_read_and_the_round_trip_of_a_job_gate_works() {
    let rules = GateRules::default();
    for spelling in ["agent-checks", "agent_checks"] {
        let policy = rules
            .apply(
                &GatePolicy::default(),
                &layer(json!({"require": [spelling]})),
                &Layer::Thread,
            )
            .unwrap();
        assert_eq!(
            policy.require,
            [CheckSource::AgentChecks].into(),
            "{spelling}"
        );
    }
    // What the API emits (`Thread.job.gate`, as JobView writes it) is a valid request.
    let job = GatePolicy::requiring([CheckSource::AgentChecks])
        .require
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let emitted = serde_json::to_value(&job).unwrap();
    assert_eq!(emitted, json!(["agent_checks"]));
    let again = rules
        .apply(
            &GatePolicy::requiring([CheckSource::AgentChecks]),
            &layer(json!({ "require": emitted })),
            &Layer::Thread,
        )
        .unwrap();
    assert_eq!(again, GatePolicy::requiring([CheckSource::AgentChecks]));
}

#[test]
fn a_request_is_compared_with_the_gate_the_thread_already_has() {
    let w = World::new();
    let app = w.app();
    let has = GatePolicy {
        max_attempts: 2,
        ..GatePolicy::requiring([CheckSource::AgentChecks])
    };
    let changes = |request: serde_json::Value| app.gate_request_changes(&has, &layer(request));
    // Saying what the thread has changes nothing, in either spelling.
    assert!(!changes(json!({})).unwrap());
    assert!(!changes(json!({"maxAttempts": 2})).unwrap());
    assert!(!changes(json!({"require": ["agent_checks"], "maxAttempts": 2})).unwrap());
    // A different number would, and so would more sources on a gate without them.
    assert!(changes(json!({"maxAttempts": 3})).unwrap());
    assert!(
        app.gate_request_changes(
            &GatePolicy::default(),
            &layer(json!({"require": ["agent-checks"]}))
        )
        .unwrap()
    );
    // What the rules refuse in any case is a refusal, not a change.
    assert!(matches!(
        changes(json!({"require": []})),
        Err(AppError::Invalid(_))
    ));
    assert!(matches!(
        changes(json!({"maxAttempts": 99})),
        Err(AppError::Invalid(_))
    ));
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
        // A thread may require the verifier, but not choose it: the target configures that.
        (json!({"verifier": "plain"}), "cannot be set per thread"),
        (json!({"ci": {"required": ["build"]}}), "per thread"),
        // `ci` on top of a policy that names no check: the first report would decide.
        (
            json!({"require": ["agent-checks", "ci"]}),
            "no check is named",
        ),
    ];
    for (request, why) in refused {
        let err = create_with(&app, "coder", Some(layer(request.clone())))
            .await
            .unwrap_err();
        assert_eq!(err.class(), ErrorClass::Invalid, "{request}");
        assert!(err.to_string().contains(why), "{request}: {err}");
    }
    assert!(
        app.list_threads(&alice(), ThreadListing::recent(None, 10, false))
            .await
            .unwrap()
            .is_empty(),
        "a refused request creates no thread"
    );
}

/// A thread may require the verifier, but it cannot choose it: the source it asks for must have
/// an agent behind it (named by the deployment or the agent's entry), and never the agent itself.
#[tokio::test]
async fn a_thread_may_require_the_verifier_only_where_there_is_one_to_ask() {
    let w = World::new();
    let app = w.app_with(AppConfig {
        // `coder` names `plain` as its verifier but does not require it; `plain` names none.
        target_gates: BTreeMap::from([(agent("coder"), layer(json!({"verifier": "plain"})))]),
        ..AppConfig::default()
    });
    let require = || Some(layer(json!({"require": ["verifier"]})));

    let t = create_with(&app, "coder", require()).await.unwrap();
    assert_eq!(t.job.gate.verifier, Some(agent("plain")));
    assert!(t.job.gate.requires(CheckSource::Verifier));

    let err = create_with(&app, "plain", require()).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Invalid);
    assert!(
        err.to_string().contains("no verifier agent is configured"),
        "{err}"
    );

    // The deployment names `plain` as the verifier of everyone; `plain` cannot verify itself.
    let app = w.app_with(AppConfig {
        gate: GatePolicy {
            verifier: Some(agent("plain")),
            ..GatePolicy::default()
        },
        ..AppConfig::default()
    });
    assert!(create_with(&app, "coder", require()).await.is_ok());
    let err = create_with(&app, "plain", require()).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Invalid);
    assert!(
        err.to_string().contains("would verify its own work"),
        "{err}"
    );
    assert!(
        app.list_threads(&alice(), ThreadListing::recent(None, 10, false))
            .await
            .unwrap()
            .iter()
            .all(|t| t.target.agent_id == agent("coder")),
        "a refused request creates no thread"
    );
}

fn try_app(w: &World, cfg: AppConfig) -> Result<TestApp, GateError> {
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
        },
        directory(),
        cfg,
    )
}

/// Another composition root cannot slip in a gate the build cannot honour: the constructor is
/// where it is refused, not a 500 on every request later.
#[test]
fn the_constructor_refuses_a_gate_that_could_never_pass() {
    let w = World::new();
    let refused = |cfg: AppConfig| try_app(&w, cfg).err().map(|e| e.to_string());
    let mut ci = GatePolicy::requiring([CheckSource::Ci]);
    ci.ci.required = ["build".to_owned()].into();
    // A build, or a deployment, that cannot honour CI: here, rules that leave it out.
    let without_ci =
        GateRules::default().honouring([CheckSource::AgentChecks, CheckSource::Verifier]);

    let says = refused(AppConfig {
        gate: ci.clone(),
        gate_rules: without_ci.clone(),
        ..AppConfig::default()
    })
    .unwrap();
    assert!(
        says.contains("deployment") && says.contains("only agent-checks, verifier"),
        "{says}"
    );
    let says = refused(AppConfig {
        target_gates: BTreeMap::from([(
            agent("coder"),
            layer(json!({"require": ["ci"], "ci": {"required": ["build"]}})),
        )]),
        gate_rules: without_ci.clone(),
        ..AppConfig::default()
    })
    .unwrap();
    assert!(
        says.contains("coder") && says.contains("only agent-checks, verifier"),
        "{says}"
    );
    let says = refused(AppConfig {
        gate: GatePolicy {
            ci: orch_core::CiPolicy {
                required: ["build".to_owned()].into(),
                ..orch_core::CiPolicy::default()
            },
            ..GatePolicy::default()
        },
        gate_rules: without_ci,
        ..AppConfig::default()
    })
    .unwrap();
    assert!(says.contains("ci settings"), "{says}");
    // The verifier is honoured: naming one is fine, and a typo in its id is still caught.
    assert!(
        refused(AppConfig {
            gate: GatePolicy {
                verifier: Some(agent("plain")),
                ..GatePolicy::default()
            },
            ..AppConfig::default()
        })
        .is_none()
    );
    let says = refused(AppConfig {
        gate: GatePolicy {
            verifier: Some(agent("ghost")),
            ..GatePolicy::default()
        },
        ..AppConfig::default()
    })
    .unwrap();
    assert!(says.contains("ghost"), "{says}");
    let says = refused(AppConfig {
        gate: GatePolicy {
            max_attempts: 11,
            ..GatePolicy::default()
        },
        ..AppConfig::default()
    })
    .unwrap();
    assert!(says.contains("1..=10"), "{says}");
    // A CI gate builds with this build's own rules (slice 6), and its references are still checked.
    assert!(
        refused(AppConfig {
            gate: ci.clone(),
            ..AppConfig::default()
        })
        .is_none()
    );
    let says = refused(AppConfig {
        gate: GatePolicy {
            verifier: Some(agent("ghost")),
            ..ci
        },
        ..AppConfig::default()
    })
    .unwrap();
    assert!(says.contains("ghost"), "{says}");
    assert!(try_app(&w, AppConfig::default()).is_ok());
}

/// A gate that requires `ci` names the checks that count: "the first report decides" would let a
/// red commit pass on whichever report arrives first (a `skipped`, another workflow's).
#[test]
fn a_gate_that_requires_ci_must_name_its_checks_in_every_layer() {
    let rules = GateRules::default();
    let ci_only = GatePolicy::requiring([CheckSource::Ci]);
    // The deployment's own policy.
    let err = rules
        .check_policy(&ci_only, &Layer::Deployment)
        .unwrap_err();
    assert!(
        matches!(
            err,
            GateError::CiWithoutChecks {
                layer: Layer::Deployment
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("ci.required"), "{err}");
    // A target that adds the source without names, or with an empty list.
    for value in [
        json!({"require": ["ci"]}),
        json!({"require": ["ci"], "ci": {"required": []}}),
        json!({"require": ["ci"], "ci": {"timeoutSecs": 30}}),
    ] {
        let err = rules
            .apply(
                &GatePolicy::default(),
                &layer(value.clone()),
                &Layer::Target(agent("coder")),
            )
            .unwrap_err();
        assert!(
            matches!(err, GateError::CiWithoutChecks { .. }),
            "{value}: {err}"
        );
    }
    // A per-thread request that adds `ci` on top of a policy with no names.
    let err = rules
        .apply(
            &GatePolicy::default(),
            &layer(json!({"require": ["ci"]})),
            &Layer::Thread,
        )
        .unwrap_err();
    assert!(
        matches!(
            err,
            GateError::CiWithoutChecks {
                layer: Layer::Thread
            }
        ),
        "{err}"
    );
    // With names, it passes at every layer that may say it.
    let named = rules
        .apply(
            &GatePolicy::default(),
            &layer(json!({"require": ["ci"], "ci": {"required": ["build"]}})),
            &Layer::Target(agent("coder")),
        )
        .unwrap();
    rules.check_policy(&named, &Layer::Deployment).unwrap();
    // A source that is not required does not need names.
    rules
        .check_policy(&GatePolicy::default(), &Layer::Deployment)
        .unwrap();
}

/// A deployment that mounts no CI webhook refuses `ci` everywhere, per-thread requests included,
/// with the reason it gives.
#[test]
fn a_deployment_can_refuse_ci_and_the_refusal_covers_every_layer() {
    const WHY: &str = "no CI webhook surface is mounted (ORCH_SURFACES)";
    let rules = GateRules::default().refusing(CheckSource::Ci, WHY);
    assert!(!rules.honours(CheckSource::Ci));
    assert!(rules.honours(CheckSource::AgentChecks));
    let mut above = GatePolicy::default();
    above.ci.required = ["build".to_owned()].into();
    for at in [
        Layer::Deployment,
        Layer::Target(agent("coder")),
        Layer::Thread,
    ] {
        let err = rules
            .apply(&above, &layer(json!({"require": ["ci"]})), &at)
            .unwrap_err();
        assert!(matches!(err, GateError::Unavailable { .. }), "{at}: {err}");
        let message = err.to_string();
        assert!(message.contains(WHY), "{message}");
        assert!(
            message.contains("only agent-checks, verifier can be required"),
            "{message}"
        );
    }
    let mut ci = GatePolicy::requiring([CheckSource::Ci]);
    ci.ci.required = ["build".to_owned()].into();
    assert!(rules.check_policy(&ci, &Layer::Deployment).is_err());
}

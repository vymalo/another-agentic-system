//! `App::ask`, the application's half of the thread tool `ask_agent` (ADR 0026): a call the
//! endpoint verified becomes an ask on the ledger, or a refusal that says why and writes nothing.
//! The rules of the ledger (who may be asked, the limits, one end) are the core's and are proven in
//! `orch-core`'s `asks.rs`; here are the checks that need the application: the token against the
//! ledger, the registry, the roles, the call key and the step the ask is shown under.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AgentDirectory, AgentEntry, App, AppConfig, AppError, ApplyOutcome, AskCall, Creation, Inbound,
    NewThread, Permission, Policy, RoleGrant,
};
use orch_core::{
    Actor, AgentId, AgentTaskState, AgentUpdate, AskLimits, AskOutcome, AskRefusal, AskResult,
    Caller, Classify, ErrorClass, EventBody, Input, Mention, ThreadId, ThreadRecord, ThreadState,
    TransitionError,
};
use orch_ports::memory::ScriptedModel;
use orch_ports::{
    AgentEndpoint, BindingUpdate, NewTimer, PortSet, Role, TIMER_SOURCE, ThreadStore,
};
use support::*;

/// `gather @coder and @reviewer`: `@coder` is at 7..13, `@reviewer` at 18..27.
const TEXT: &str = "gather @coder and @reviewer";

fn mentions() -> Vec<Mention> {
    let mention = |id: &str, start, end| Mention {
        agent_id: AgentId::new(id),
        label: format!("@{id}"),
        start,
        end,
        card_url: None,
    };
    vec![mention("coder", 7, 13), mention("reviewer", 18, 27)]
}

fn entry(id: &str) -> AgentEntry {
    AgentEntry {
        endpoint: AgentEndpoint::a2a(
            AgentId::new(id),
            format!("https://{id}.example.com/.well-known/agent-card.json"),
            None,
        ),
        name: id.to_owned(),
    }
}

/// The agents this file's threads use: the directory of the support module has two.
fn directory(ids: &[&str]) -> AgentDirectory {
    AgentDirectory::new(ids.iter().map(|id| entry(id)).collect())
}

/// An application on the world's store that lists exactly `ids`.
fn app_listing(w: &World, ids: &[&str], cfg: AppConfig) -> Arc<TestApp> {
    let directory = directory(ids);
    Arc::new(
        App::new(
            PortSet {
                artifacts: orch_ports::NoArtifacts,
                store: w.store.clone(),
                wakeup: w.wakeup.clone(),
                agents: w.agent.clone(),
                clock: orch_ports::SystemClock,
                ids: w.ids.clone(),
                model: ScriptedModel::default().with_endpoints(["default"]),
                auth: orch_ports::RefuseAll,
                registry: directory.fixed_registry(),
            },
            directory,
            cfg,
        )
        .expect("a valid gate"),
    )
}

fn app_with(w: &World, cfg: AppConfig) -> Arc<TestApp> {
    app_listing(w, &["plain", "coder", "reviewer"], cfg)
}

fn working_status() -> Input {
    Input::Agent {
        agent: AgentId::new("plain"),
        revision: Some("r1".to_owned()),
        update: AgentUpdate::Status {
            state: AgentTaskState::Working,
            detail: None,
        },
    }
}

/// A thread of `plain` that mentions `coder` and `reviewer`, working, with a task on its binding.
async fn working_thread(app: &TestApp) -> ThreadRecord {
    let id = ThreadId(uuid::Uuid::now_v7());
    let Creation::Created { .. } = app
        .create_thread_as(
            &alice(),
            id,
            NewThread {
                title: None,
                target: target("plain"),
                text: TEXT.to_owned(),
            },
            Inbound {
                mentions: mentions(),
                ..Inbound::default()
            },
        )
        .await
        .unwrap()
    else {
        panic!("a new thread");
    };
    app.apply(id, working_status(), None, None, None)
        .await
        .unwrap();
    app.record_binding(
        id,
        BindingUpdate {
            task_id: Some("task-7".to_owned()),
            task_state: Some(AgentTaskState::Working),
            revision: Some("r1".to_owned()),
        },
        None,
    )
    .await
    .unwrap();
    app.get_thread(&alice(), id).await.unwrap()
}

fn call(thread: &ThreadRecord, to: &str, id: Option<&str>) -> AskCall {
    AskCall {
        thread: thread.id,
        job: 1,
        caller: Caller::Main,
        asker: AgentId::new("plain"),
        agent: AgentId::new(to),
        text: "find the data".to_owned(),
        call_id: id.map(str::to_owned),
        parent_step: None,
        timeout: None,
    }
}

fn refusal(error: AppError) -> AskRefusal {
    match error {
        AppError::Transition(TransitionError::AskRefused(why)) => why,
        other => panic!("expected a refusal of the ask, got {other:?}"),
    }
}

async fn thread_of(app: &TestApp, id: ThreadId) -> ThreadRecord {
    app.get_thread(&alice(), id).await.unwrap()
}

async fn end(app: &TestApp, thread: ThreadId, ask: u32) {
    app.apply(
        thread,
        Input::AskFinished {
            job: 1,
            ask,
            revision: None,
            result: AskResult::of(AskOutcome::Completed),
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn a_call_becomes_an_ask_attributed_to_the_asker_with_its_key_and_step() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    let before = thread_of(&app, t.id).await.last_seq;

    let handle = app
        .ask(AskCall {
            parent_step: Some("call-3".to_owned()),
            ..call(&t, "coder", Some("c1"))
        })
        .await
        .unwrap();
    assert_eq!(handle.ask, 1);
    assert_eq!(handle.agent, AgentId::new("coder"));
    assert!(!handle.reattached);
    assert_eq!(
        handle.after, before,
        "the log is read from just before the ask"
    );
    assert_eq!(handle.timeout, Duration::from_secs(1800));

    let events = app.list_events(&alice(), t.id, before, 10).await.unwrap();
    let [started] = &events[..] else {
        panic!("one event: {events:?}");
    };
    // the asker's own words, with the revision that serves its task
    assert_eq!(
        started.actor,
        Actor::agent(&AgentId::new("plain"), Some("r1".to_owned()))
    );
    let EventBody::AskStarted(d) = &started.body else {
        panic!("ask_started");
    };
    assert_eq!((d.ask, d.depth, d.by), (1, 1, Caller::Main));
    assert_eq!(d.text, "find the data");
    assert_eq!(d.step_id, "ask-1");
    // the agent's step ids are its task's, prefixed
    assert_eq!(d.parent_step_id.as_deref(), Some("task-7/call-3"));
    let ledger = thread_of(&app, t.id).await.job.asks;
    assert_eq!(
        ledger[0].call_key.as_deref(),
        Some(format!("ask:{}:main:c1", t.id).as_str())
    );
}

#[tokio::test]
async fn an_asked_agents_ask_is_shown_under_its_own_step_and_is_one_level_deeper() {
    let w = World::new();
    let app = app_with(
        &w,
        AppConfig {
            asks: AskLimits {
                depth: 3,
                ..AskLimits::default()
            },
            ..AppConfig::default()
        },
    );
    let t = working_thread(&app).await;
    app.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    let handle = app
        .ask(AskCall {
            caller: Caller::Ask(1),
            asker: AgentId::new("coder"),
            // the asked agent's own step ids are not the thread's: the ask is under its step
            parent_step: Some("whatever".to_owned()),
            ..call(&t, "reviewer", Some("c1"))
        })
        .await
        .unwrap();
    assert_eq!(
        handle.ask, 2,
        "the same callId of another caller is another call"
    );
    let started = app
        .list_events(&alice(), t.id, handle.after, 10)
        .await
        .unwrap();
    let EventBody::AskStarted(d) = &started[0].body else {
        panic!("ask_started");
    };
    assert_eq!((d.by, d.depth), (Caller::Ask(1), 2));
    assert_eq!(d.parent_step_id.as_deref(), Some("ask-1"));
    assert_eq!(started[0].actor.name, "coder");
}

#[tokio::test]
async fn the_same_call_again_re_attaches_and_writes_nothing() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    let first = app.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    let after = thread_of(&app, t.id).await;

    let again = app.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    assert_eq!(
        (again.ask, again.reattached, again.after),
        (first.ask, true, 0)
    );
    let same = thread_of(&app, t.id).await;
    assert_eq!(same.last_seq, after.last_seq, "nothing was written");
    assert_eq!(same.job.asks.len(), 1);

    // whatever has become of the ask or of the world since: still the same ask
    end(&app, t.id, 1).await;
    let spent = AppConfig {
        asks: AskLimits {
            per_job: 1,
            ..AskLimits::default()
        },
        ..AppConfig::default()
    };
    let strict = app_listing(&w, &["plain"], spent);
    let again = strict.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    assert_eq!((again.ask, again.reattached), (1, true));

    // a call with no id is a new ask every time
    let a = app.ask(call(&t, "coder", None)).await.unwrap();
    let b = app.ask(call(&t, "coder", None)).await.unwrap();
    assert_eq!((a.ask, b.ask), (2, 3));
}

#[tokio::test]
async fn a_call_key_names_one_question() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    app.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    let before = thread_of(&app, t.id).await;
    for other in [
        call(&t, "reviewer", Some("c1")),
        AskCall {
            text: "find other data".to_owned(),
            ..call(&t, "coder", Some("c1"))
        },
    ] {
        let err = app.ask(other).await.unwrap_err();
        assert_eq!(refusal(err), AskRefusal::CallKeyReused);
    }
    assert_eq!(thread_of(&app, t.id).await.last_seq, before.last_seq);
}

#[tokio::test]
async fn a_call_may_lower_the_deadline_and_never_raise_it() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    let lowered = app
        .ask(AskCall {
            timeout: Some(Duration::from_secs(60)),
            ..call(&t, "coder", Some("c1"))
        })
        .await
        .unwrap();
    assert_eq!(lowered.timeout, Duration::from_secs(60));
    let raised = app
        .ask(AskCall {
            timeout: Some(Duration::from_secs(7200)),
            ..call(&t, "coder", Some("c2"))
        })
        .await
        .unwrap();
    assert_eq!(raised.timeout, Duration::from_secs(1800));
    // a repeat of the first call re-attaches and is told the deployment's deadline, not its own
    let repeat = app
        .ask(AskCall {
            timeout: Some(Duration::from_secs(5)),
            ..call(&t, "coder", Some("c1"))
        })
        .await
        .unwrap();
    assert_eq!((repeat.ask, repeat.reattached), (lowered.ask, true));
    assert_eq!(repeat.timeout, Duration::from_secs(1800));
    // the deadline timers say so: due 60 s and 1800 s after the same moment, give or take the
    // time between the two commits
    let due = |ask: u32| {
        let w = w.clone();
        let key = deadline_key(t.id, 1, ask);
        async move {
            w.store
                .find_inbox(TIMER_SOURCE, &key)
                .await
                .unwrap()
                .expect("the deadline is an inbox row")
                .available_at
        }
    };
    let gap = due(2).await.duration_since(due(1).await);
    let gap = gap.as_secs();
    assert!(
        (1735..=1745).contains(&gap),
        "1800 s against 60 s, not {gap} s"
    );
}

fn deadline_key(id: ThreadId, job: u32, ask: u32) -> String {
    NewTimer {
        id: orch_ports::InboxId(uuid::Uuid::nil()),
        after: jiff::SignedDuration::ZERO,
        timer: orch_core::Timer::AskDeadline { job, ask },
    }
    .idempotency_key(id)
}

#[tokio::test]
async fn every_refusal_writes_nothing_and_says_which_class_it_is() {
    let w = World::new();
    let cfg = AppConfig {
        asks: AskLimits {
            depth: 1,
            per_job: 2,
            running: 1,
            ..AskLimits::default()
        },
        ..AppConfig::default()
    };
    let app = app_with(&w, cfg);
    let t = working_thread(&app).await;
    let head = thread_of(&app, t.id).await.last_seq;
    let rejected = |e: &AppError| e.class();

    // an agent nobody mentioned: the agents that may be asked are listed
    let err = app.ask(call(&t, "plain", Some("x"))).await.unwrap_err();
    let class = rejected(&err);
    let AskRefusal::NotMentioned { mentioned, .. } = refusal(err) else {
        panic!("not mentioned");
    };
    assert_eq!(mentioned, [AgentId::new("coder"), AgentId::new("reviewer")]);
    assert_eq!(class, ErrorClass::Invalid);

    // a token of another job
    let err = app
        .ask(AskCall {
            job: 2,
            ..call(&t, "coder", Some("x"))
        })
        .await
        .unwrap_err();
    assert_eq!(refusal(err), AskRefusal::TaskOver);

    // a caller that is not on the ledger: an ask that does not exist, and one that is another
    // agent's
    let err = app
        .ask(AskCall {
            caller: Caller::Ask(9),
            asker: AgentId::new("coder"),
            ..call(&t, "reviewer", Some("x"))
        })
        .await
        .unwrap_err();
    assert_eq!(refusal(err), AskRefusal::UnknownCaller { n: 9 });
    let err = app
        .ask(AskCall {
            asker: AgentId::new("coder"),
            ..call(&t, "reviewer", Some("x"))
        })
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
    assert_eq!(err.class(), ErrorClass::NotFound);

    // the first ask is fine; the depth limit is 1, so the asked agent may not ask
    let first = app.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    let err = app
        .ask(AskCall {
            caller: Caller::Ask(first.ask),
            asker: AgentId::new("coder"),
            ..call(&t, "reviewer", Some("x"))
        })
        .await
        .unwrap_err();
    assert_eq!(refusal(err), AskRefusal::DepthReached { max: 1 });
    // another agent of the asked agent's token
    let err = app
        .ask(AskCall {
            caller: Caller::Ask(first.ask),
            asker: AgentId::new("reviewer"),
            ..call(&t, "reviewer", Some("x"))
        })
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");

    // one runs and the limit is one running
    let err = app.ask(call(&t, "reviewer", Some("c2"))).await.unwrap_err();
    assert_eq!(refusal(err), AskRefusal::TooManyRunning { max: 1 });
    // the first ends: its token is a task that is over
    end(&app, t.id, 1).await;
    let err = app
        .ask(AskCall {
            caller: Caller::Ask(first.ask),
            asker: AgentId::new("coder"),
            ..call(&t, "reviewer", Some("x"))
        })
        .await
        .unwrap_err();
    assert_eq!(refusal(err), AskRefusal::TaskOver);
    // two asks a job: the second is fine, the third is not
    app.ask(call(&t, "reviewer", Some("c2"))).await.unwrap();
    end(&app, t.id, 2).await;
    let err = app.ask(call(&t, "coder", Some("c3"))).await.unwrap_err();
    assert_eq!(refusal(err), AskRefusal::TooManyInJob { max: 2 });

    // an empty question, a call key too long
    let err = app
        .ask(AskCall {
            text: "  ".to_owned(),
            ..call(&t, "coder", Some("e1"))
        })
        .await
        .unwrap_err();
    assert_eq!(refusal(err), AskRefusal::EmptyText);
    let err = app
        .ask(AskCall {
            call_id: Some("k".repeat(300)),
            ..call(&t, "coder", None)
        })
        .await
        .unwrap_err();
    assert!(matches!(refusal(err), AskRefusal::CallKeyTooLong { .. }));

    // nothing was written by any of them: the log holds the two asks and their ends
    let events = app.list_events(&alice(), t.id, head, 50).await.unwrap();
    let kinds: Vec<_> = events.iter().map(orch_core::Event::kind).collect();
    assert_eq!(
        kinds
            .iter()
            .filter(|k| **k == orch_core::EventKind::AskStarted)
            .count(),
        2,
        "{kinds:?}"
    );
    assert_eq!(events.len(), 4, "{kinds:?}");

    // a thread that is over takes none, and one that does not exist is not found
    app.apply(
        t.id,
        Input::Agent {
            agent: AgentId::new("plain"),
            revision: None,
            update: AgentUpdate::Status {
                state: AgentTaskState::Completed,
                detail: None,
            },
        },
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(thread_of(&app, t.id).await.state, ThreadState::Done);
    let err = app.ask(call(&t, "coder", Some("late"))).await.unwrap_err();
    assert_eq!(refusal(err), AskRefusal::TaskOver);
    let err = app
        .ask(AskCall {
            thread: ThreadId(uuid::Uuid::now_v7()),
            ..call(&t, "coder", Some("x"))
        })
        .await
        .unwrap_err();
    assert!(matches!(err, AppError::NotFound), "{err:?}");
}

#[tokio::test]
async fn an_agent_the_registry_no_longer_lists_is_422() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    // the platform removed `coder` since the person mentioned it
    let without = app_listing(&w, &["plain", "reviewer"], AppConfig::default());
    let err = without
        .ask(call(&t, "coder", Some("c1")))
        .await
        .unwrap_err();
    assert!(matches!(&err, AppError::Unprocessable(m) if m == "agent 'coder' is no longer listed"));
    assert_eq!(err.class(), ErrorClass::Rejected);
    // the others are not affected
    without.ask(call(&t, "reviewer", Some("c2"))).await.unwrap();
    assert_eq!(thread_of(&app, t.id).await.job.asks.len(), 1);
}

#[tokio::test]
async fn an_agent_no_role_may_invoke_is_403_and_nothing_is_written() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    let head = thread_of(&app, t.id).await.last_seq;
    // the deployment's only role may use `plain` and `reviewer`, not `coder`
    let grant = RoleGrant {
        agents: orch_app::AgentScope::from_patterns(["plain", "reviewer"]),
        ..RoleGrant::user()
    };
    let policy = Policy::new(
        BTreeMap::from([(Role::new("user"), grant)]),
        Some(Role::new("user")),
    )
    .unwrap();
    assert!(!policy.any_role_may_invoke(&AgentId::new("coder")));
    assert!(policy.any_role_may_invoke(&AgentId::new("reviewer")));
    let strict = app_with(
        &w,
        AppConfig {
            policy,
            ..AppConfig::default()
        },
    );
    let err = strict.ask(call(&t, "coder", Some("c1"))).await.unwrap_err();
    assert!(
        matches!(
            &err,
            AppError::Forbidden {
                permission: Permission::AgentInvoke,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(err.class(), ErrorClass::Forbidden);
    assert_eq!(thread_of(&app, t.id).await.last_seq, head);
    // the agent a role names is asked
    strict.ask(call(&t, "reviewer", Some("c2"))).await.unwrap();
    // a policy that grants nothing refuses every agent
    let nobody = app_with(
        &w,
        AppConfig {
            policy: Policy::deny_all(),
            ..AppConfig::default()
        },
    );
    let err = nobody
        .ask(call(&t, "reviewer", Some("c3")))
        .await
        .unwrap_err();
    assert_eq!(err.class(), ErrorClass::Forbidden);
}

#[tokio::test]
async fn a_user_still_cannot_submit_an_ask() {
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    let input = Input::Ask {
        actor: Actor::agent(&AgentId::new("plain"), None),
        caller: Caller::Main,
        agent: AgentId::new("coder"),
        text: "x".to_owned(),
        call_key: None,
        parent_step: None,
        limits: AskLimits::default(),
    };
    let err = app.submit(&alice(), t.id, input, None).await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Invalid);
    assert!(matches!(
        app.apply(t.id, working_status(), None, None, None)
            .await
            .unwrap(),
        ApplyOutcome::Applied { .. }
    ));
}

#[tokio::test]
async fn the_log_is_followed_for_the_tool_without_an_owner() {
    use futures::StreamExt;
    let w = World::new();
    let app = app_with(&w, AppConfig::default());
    let t = working_thread(&app).await;
    let handle = app.ask(call(&t, "coder", Some("c1"))).await.unwrap();
    let mut stream = app
        .thread_events_for_tools(t.id, handle.after)
        .await
        .unwrap();
    let first = stream.next().await.unwrap();
    assert!(matches!(first.body, EventBody::AskStarted(_)));
    end(&app, t.id, handle.ask).await;
    let next = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(&next.body, EventBody::AskFinished(d) if d.ask == 1));
    // a thread that is not there is not found
    let missing = app
        .thread_events_for_tools(ThreadId(uuid::Uuid::now_v7()), 0)
        .await;
    assert!(matches!(missing, Err(AppError::NotFound)));
}

#[tokio::test]
async fn a_registry_that_cannot_say_is_a_retry_not_a_refusal_and_a_repeat_does_not_ask_it() {
    use orch_core::AgentSource;
    use orch_ports::memory::{MemoryRegistry, MemoryStore, MemoryWakeup, ScriptedAgent, SeqIds};
    use orch_ports::{CompositeRegistry, FixedRegistry, RegistryEntry, SystemClock};

    type RegistryPorts = PortSet<
        MemoryStore,
        MemoryWakeup,
        ScriptedAgent,
        SystemClock,
        SeqIds,
        ScriptedModel,
        CompositeRegistry<FixedRegistry, MemoryRegistry>,
    >;
    let w = World::new();
    let registry = MemoryRegistry::new();
    // `reviewer` is the platform's: only the registry lists it
    registry.add(RegistryEntry {
        endpoint: entry("reviewer").endpoint,
        name: "Reviewer".to_owned(),
        tags: Vec::new(),
        origin: AgentSource::Registry,
    });
    let directory = directory(&["plain", "coder"]);
    let app: Arc<App<RegistryPorts>> = Arc::new(
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
                registry: CompositeRegistry::new(directory.fixed_registry(), registry.clone()),
            },
            directory,
            AppConfig::default(),
        )
        .expect("a valid gate"),
    );
    let id = ThreadId(uuid::Uuid::now_v7());
    app.create_thread_as(
        &alice(),
        id,
        NewThread {
            title: None,
            target: target("plain"),
            text: TEXT.to_owned(),
        },
        Inbound {
            mentions: mentions(),
            ..Inbound::default()
        },
    )
    .await
    .unwrap();
    app.apply(id, working_status(), None, None, None)
        .await
        .unwrap();
    let t = app.get_thread(&alice(), id).await.unwrap();

    let first = app.ask(call(&t, "reviewer", Some("c1"))).await.unwrap();
    registry.set_down(true);
    // the registry cannot say: the caller may try again, and nothing is written
    let head = app.get_thread(&alice(), id).await.unwrap().last_seq;
    let err = app.ask(call(&t, "reviewer", Some("c2"))).await.unwrap_err();
    assert!(
        matches!(err, AppError::RegistryUnavailable { .. }),
        "{err:?}"
    );
    assert_eq!(app.get_thread(&alice(), id).await.unwrap().last_seq, head);
    // a repeat of a call is answered from the ledger, without asking the registry
    let again = app.ask(call(&t, "reviewer", Some("c1"))).await.unwrap();
    assert_eq!((again.ask, again.reattached), (first.ask, true));
    // and when it answers again, the call goes through
    registry.set_down(false);
    app.ask(call(&t, "reviewer", Some("c2"))).await.unwrap();
}

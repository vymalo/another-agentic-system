//! MCP servers attached to a thread (ADR 0024): who may attach what, what the log and the thread
//! keep, what a fork keeps, and what an agent is told when a message is sent.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use orch_app::{
    AppConfig, AppError, Creation, ForkAt, ForkRequest, Inbound, NewThread, ToolServerInfo,
};
use orch_core::{
    AgentId, AgentTarget, AttachedServer, Classify, ErrorClass, EventBody, EventKind, ThreadId,
    ThreadRecord, ToolsData, UserId,
};
use orch_ports::Principal;
use orch_ports::memory::Call;
use support::*;

fn websearch() -> ToolServerInfo {
    ToolServerInfo {
        description: Some("Search the web.".to_owned()),
        icon: Some("data:image/svg+xml;base64,PHN2Zy8+".to_owned()),
        ..ToolServerInfo::new("websearch", "Web search")
    }
}

fn docs() -> ToolServerInfo {
    ToolServerInfo::new("docs", "Documentation")
}

/// Only the coder may use it.
fn repos() -> ToolServerInfo {
    ToolServerInfo {
        agents: Some(vec![AgentId::new("coder")]),
        ..ToolServerInfo::new("repos", "Repositories")
    }
}

fn with_servers(w: &World) -> Arc<TestApp> {
    w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tool_servers: vec![websearch(), docs(), repos()],
        ..AppConfig::default()
    })
}

fn ids(items: &[&str]) -> Vec<String> {
    items.iter().map(|i| (*i).to_owned()).collect()
}

fn new_thread(agent: &str) -> NewThread {
    NewThread {
        title: None,
        target: AgentTarget {
            agent_id: AgentId::new(agent),
            release: None,
        },
        text: "hello".to_owned(),
    }
}

async fn create_with(
    app: &TestApp,
    who: &UserId,
    agent: &str,
    tools: &[&str],
) -> Result<ThreadRecord, AppError> {
    let id = ThreadId(uuid::Uuid::now_v7());
    let inbound = Inbound {
        tools: ids(tools),
        ..Inbound::default()
    };
    match app
        .create_thread_as(who, id, new_thread(agent), inbound)
        .await?
    {
        Creation::Created { thread, .. } => Ok(thread),
        Creation::Exists => panic!("a fresh id exists"),
    }
}

fn unprocessable<T: std::fmt::Debug>(result: Result<T, AppError>) -> String {
    match result {
        Err(AppError::Unprocessable(detail)) => detail,
        other => panic!("expected Unprocessable, got {other:?}"),
    }
}

#[tokio::test]
async fn a_thread_is_created_with_its_servers_after_the_first_message_in_one_commit() {
    let w = World::new();
    let app = with_servers(&w);
    let thread = create_with(&app, &alice(), "plain", &["websearch", "docs"])
        .await
        .unwrap();
    assert_eq!(thread.job.tools, ids(&["docs", "websearch"]));
    let log = events(&app, &alice(), thread.id).await;
    assert_eq!(
        kinds(&log),
        [EventKind::UserMessage, EventKind::ToolsAttached],
        "the message first, then the servers"
    );
    assert_eq!(
        log[1].body,
        EventBody::ToolsAttached(ToolsData {
            servers: ids(&["docs", "websearch"])
        })
    );
    assert_eq!(log[1].actor.name, "alice@example.com", "the person's event");
    // both are in the first commit: one version, no gap
    assert_eq!(thread.last_seq, 2);
    assert_eq!(app.get_thread(&alice(), thread.id).await.unwrap(), thread);
}

#[tokio::test]
async fn a_thread_created_without_servers_has_no_event_for_them() {
    let w = World::new();
    let app = with_servers(&w);
    let thread = create_with(&app, &alice(), "plain", &[]).await.unwrap();
    assert!(thread.job.tools.is_empty());
    assert_eq!(
        kinds(&events(&app, &alice(), thread.id).await),
        [EventKind::UserMessage]
    );
}

#[tokio::test]
async fn creating_a_thread_refuses_what_it_could_not_attach_and_writes_nothing() {
    let w = World::new();
    let app = with_servers(&w);
    // unknown
    let detail = unprocessable(create_with(&app, &alice(), "plain", &["nosuch"]).await);
    assert!(detail.contains("nosuch"), "{detail}");
    // not offered for the thread's agent
    let detail = unprocessable(create_with(&app, &alice(), "plain", &["repos"]).await);
    assert!(
        detail.contains("repos") && detail.contains("plain"),
        "{detail}"
    );
    assert!(
        create_with(&app, &alice(), "coder", &["repos"])
            .await
            .is_ok()
    );
    // not an id at all
    let bad = create_with(&app, &alice(), "plain", &["Web Search"]).await;
    assert!(matches!(bad, Err(AppError::Invalid(_))), "{bad:?}");
    // nothing was created by the refusals
    let threads = app.list_threads(&alice(), None, 50, false).await.unwrap();
    assert_eq!(threads.len(), 1, "only the coder's thread");
}

#[tokio::test]
async fn the_set_is_changed_in_any_state_and_the_same_set_changes_nothing() {
    let w = World::new();
    let app = with_servers(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create(&app, &alice(), "plain", "echo hi").await;
    wait_state(&app, &alice(), t.id, orch_core::ThreadState::Done).await;
    let before = events(&app, &alice(), t.id).await.len();

    let thread = app
        .set_tools(&alice(), t.id, ids(&["websearch"]))
        .await
        .unwrap();
    assert_eq!(thread.job.tools, ids(&["websearch"]));
    // the same set again: the thread as it is, no event, no version
    let again = app
        .set_tools(&alice(), t.id, ids(&["websearch", "websearch"]))
        .await
        .unwrap();
    assert_eq!(again, thread);
    let log = events(&app, &alice(), t.id).await;
    assert_eq!(log.len(), before + 1);
    assert_eq!(log.last().unwrap().kind(), EventKind::ToolsAttached);

    // one more and one fewer, in one commit: the attach event, then the detach event
    let thread = app.set_tools(&alice(), t.id, ids(&["docs"])).await.unwrap();
    assert_eq!(thread.job.tools, ids(&["docs"]));
    let log = events(&app, &alice(), t.id).await;
    assert_eq!(
        kinds(&log[before..]),
        [
            EventKind::ToolsAttached,
            EventKind::ToolsAttached,
            EventKind::ToolsDetached
        ]
    );
    assert_eq!(
        log.last().unwrap().body,
        EventBody::ToolsDetached(ToolsData {
            servers: ids(&["websearch"])
        })
    );
    // clearing
    let thread = app.set_tools(&alice(), t.id, vec![]).await.unwrap();
    assert!(thread.job.tools.is_empty());
    // the thread did not leave its state for any of this
    assert_eq!(thread.state, orch_core::ThreadState::Done);
    run.shutdown().await;
}

#[tokio::test]
async fn the_set_is_refused_with_a_reason_that_names_the_server_and_never_a_url() {
    let w = World::new();
    let app = with_servers(&w);
    let t = create(&app, &alice(), "plain", "hello").await;
    let detail = unprocessable(app.set_tools(&alice(), t.id, ids(&["nosuch"])).await);
    assert!(detail.contains("nosuch"));
    let detail = unprocessable(app.set_tools(&alice(), t.id, ids(&["repos"])).await);
    assert!(detail.contains("repos"));
    // more than sixteen
    let many: Vec<String> = (0..17).map(|n| format!("s{n}")).collect();
    unprocessable(app.set_tools(&alice(), t.id, many).await);
    let bad = app.set_tools(&alice(), t.id, ids(&["Bad Id"])).await;
    assert!(matches!(bad, Err(AppError::Invalid(_))), "{bad:?}");
    // none of it changed the thread
    assert!(
        app.get_thread(&alice(), t.id)
            .await
            .unwrap()
            .job
            .tools
            .is_empty()
    );
    assert_eq!(
        kinds(&events(&app, &alice(), t.id).await),
        [EventKind::UserMessage]
    );
}

#[tokio::test]
async fn a_thread_keeps_a_server_the_deployment_no_longer_lists_and_a_person_can_detach_it() {
    let w = World::new();
    let before = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tool_servers: vec![websearch(), docs()],
        ..AppConfig::default()
    });
    let t = create_with(&before, &alice(), "plain", &["websearch"])
        .await
        .unwrap();
    // the deployment dropped `websearch` (a restart with another file)
    let after = w.app_with(AppConfig {
        stream_poll: Duration::from_millis(100),
        tool_servers: vec![docs()],
        ..AppConfig::default()
    });
    // keeping it while attaching another is not refused: only what is new is checked
    let thread = after
        .set_tools(&alice(), t.id, ids(&["websearch", "docs"]))
        .await
        .unwrap();
    assert_eq!(thread.job.tools, ids(&["docs", "websearch"]));
    // and detaching it is always possible
    let thread = after
        .set_tools(&alice(), t.id, ids(&["docs"]))
        .await
        .unwrap();
    assert_eq!(thread.job.tools, ids(&["docs"]));
    // attaching it afresh is not
    unprocessable(
        after
            .set_tools(&alice(), t.id, ids(&["docs", "websearch"]))
            .await,
    );
}

#[tokio::test]
async fn only_the_person_who_may_act_on_the_thread_may_set_its_servers() {
    let w = World::new();
    let app = with_servers(&w);
    let t = create(&app, &alice(), "plain", "hello").await;
    // another person: the thread does not exist for them
    let denied = app.set_tools(&bob(), t.id, ids(&["docs"])).await;
    assert!(matches!(denied, Err(AppError::NotFound)), "{denied:?}");
    let nobody = ThreadId(uuid::Uuid::from_u128(7));
    let denied = app.set_tools(&alice(), nobody, ids(&["docs"])).await;
    assert!(matches!(denied, Err(AppError::NotFound)), "{denied:?}");
    // an administrator is no exception: no role reaches another person's thread (ADR 0039)
    let admin = Principal {
        roles: [orch_ports::Role::new("admin")].into(),
        ..Principal::of(UserId::new("root@example.com"))
    };
    let denied = app.set_tools(&admin, t.id, ids(&["docs"])).await;
    assert!(matches!(denied, Err(AppError::NotFound)), "{denied:?}");
    // a person whose roles hold no `thread.write` is refused whatever the thread
    let nobody_role = Principal::of(UserId::new("lurker@example.com"));
    let policy_app = w.app_with(AppConfig {
        policy: orch_app::Policy::new(Default::default(), None).unwrap(),
        tool_servers: vec![docs()],
        ..AppConfig::default()
    });
    assert!(matches!(
        policy_app
            .set_tools(&nobody_role, t.id, ids(&["docs"]))
            .await,
        Err(AppError::Forbidden { .. })
    ));
    assert!(policy_app.list_tool_servers(&nobody_role).is_err());
    // nothing changed for any of them
    assert!(
        app.get_thread(&alice(), t.id)
            .await
            .unwrap()
            .job
            .tools
            .is_empty()
    );
}

#[tokio::test]
async fn the_servers_are_listed_in_the_order_of_the_deployment_to_anyone_who_may_write() {
    let w = World::new();
    let app = with_servers(&w);
    let listed = app.list_tool_servers(&alice()).unwrap();
    assert_eq!(
        listed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["websearch", "docs", "repos"]
    );
    assert_eq!(
        ErrorClass::Rejected,
        AppError::Unprocessable(String::new()).class()
    );
    // the debug of a server leaves its icon out
    let debug = format!("{:?}", websearch());
    assert!(!debug.contains("PHN2Zy8+"), "{debug}");
}

#[tokio::test]
async fn an_agent_is_told_the_servers_attached_that_it_may_use_each_time_a_message_is_sent() {
    let w = World::new();
    let app = with_servers(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create_with(&app, &alice(), "plain", &["websearch", "docs"])
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, orch_core::ThreadState::Done).await;
    // attach one more between the jobs, detach one: the next message says what is attached now
    app.set_tools(&alice(), t.id, ids(&["docs", "websearch"]))
        .await
        .unwrap();
    app.set_tools(&alice(), t.id, ids(&["docs"])).await.unwrap();
    app.post_message(&alice(), t.id, "echo again".into())
        .await
        .unwrap();
    eventually("the second send", || async {
        (w.agent.sends().len() == 2).then_some(())
    })
    .await;
    let told: Vec<Vec<AttachedServer>> = w
        .agent
        .sends()
        .into_iter()
        .map(|call| match call {
            Call::Send { thread_tools, .. } => thread_tools.expect("a grant").attached,
            other => panic!("not a send: {other:?}"),
        })
        .collect();
    let server = |id: &str, name: &str, description: Option<&str>| AttachedServer {
        id: id.to_owned(),
        name: name.to_owned(),
        description: description.map(str::to_owned),
    };
    assert_eq!(
        told,
        [
            vec![
                server("docs", "Documentation", None),
                server("websearch", "Web search", Some("Search the web."))
            ],
            vec![server("docs", "Documentation", None)],
        ],
        "ids, names and descriptions, in the order of the ids, as attached at each send"
    );
    run.shutdown().await;
}

#[tokio::test]
async fn a_server_an_agent_may_not_use_is_not_told_to_it() {
    let w = World::new();
    let app = with_servers(&w);
    // `repos` is the coder's: attached to a coder thread, then the thread is read by the plain
    // agent's rules
    assert_eq!(
        app.attached_for(&AgentId::new("coder"), &ids(&["repos", "docs"]))
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["docs", "repos"]
    );
    assert_eq!(
        app.attached_for(&AgentId::new("plain"), &ids(&["repos", "docs", "gone"]))
            .iter()
            .map(|s| s.id.as_str())
            .collect::<Vec<_>>(),
        ["docs"],
        "not offered for it, and one the deployment no longer lists"
    );
}

#[tokio::test]
async fn a_fork_has_the_servers_its_copy_left_attached_but_only_those_its_agent_may_use() {
    let w = World::new();
    let app = with_servers(&w);
    let run = spawn_dispatcher(&app, fast(), "d1");
    let t = create_with(&app, &alice(), "coder", &["repos", "docs"])
        .await
        .unwrap();
    wait_state(&app, &alice(), t.id, orch_core::ThreadState::Done).await;
    // a fork on the same agent keeps both
    let same = app
        .fork_thread(
            &alice(),
            t.id,
            ForkRequest {
                at: ForkAt::AfterTurn { seq: 1 },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(same.thread.job.tools, ids(&["docs", "repos"]));
    // a fork that continues with the plain agent cannot have the coder's server: the copy says it
    // was attached, and the log of the fork then says it was detached, so the log and the
    // thread agree
    let other = app
        .fork_thread(
            &alice(),
            t.id,
            ForkRequest {
                at: ForkAt::AfterTurn { seq: 1 },
                target: Some(AgentTarget {
                    agent_id: AgentId::new("plain"),
                    release: None,
                }),
                id: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(other.thread.job.tools, ids(&["docs"]));
    let log = events(&app, &alice(), other.thread.id).await;
    let last = log.last().unwrap();
    assert_eq!(
        last.body,
        EventBody::ToolsDetached(ToolsData {
            servers: ids(&["repos"])
        })
    );
    run.shutdown().await;
}

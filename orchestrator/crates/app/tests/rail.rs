//! The person's list of threads through the application (ADR 0042): pinning, archiving, moving and
//! ejecting are rows of the thread, never events of its log; they need the thread to be the
//! person's and `thread.read`, not `thread.write`; a fork is nested under the row the person sees.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use orch_app::{
    AgentScope, AppConfig, AppError, FirstMessage, ForkAt, ForkRequest, Permission, Policy,
    RoleGrant,
};
use orch_core::{Classify, ErrorClass, ForkKind, Origin, ThreadId, ThreadRecord, ThreadState};
use orch_ports::{ArchivedFilter, Arrangement, Place, Principal, Role, ThreadListing, ThreadStore};
use support::*;

fn principal(email: &str, roles: &[&str]) -> Principal {
    Principal {
        roles: roles.iter().map(|r| Role::new(*r)).collect(),
        ..Principal::of(orch_core::UserId::new(email))
    }
}

fn role(permissions: &[Permission]) -> RoleGrant {
    RoleGrant {
        permissions: permissions.iter().copied().collect::<BTreeSet<_>>(),
        agents: AgentScope::from_patterns(["*"]),
    }
}

/// `user` does everything, `reader` only reads, `writer` writes and cannot read the list.
fn app_under_roles(w: &World) -> Arc<TestApp> {
    let roles: BTreeMap<Role, RoleGrant> = [
        ("user", role(&Permission::ALL)),
        ("reader", role(&[Permission::ThreadRead])),
        (
            "writer",
            role(&[Permission::ThreadWrite, Permission::AgentInvoke]),
        ),
    ]
    .into_iter()
    .map(|(name, grant)| (Role::new(name), grant))
    .collect();
    w.app_with(AppConfig {
        policy: Policy::new(roles, None).unwrap(),
        stream_poll: std::time::Duration::from_millis(100),
        ..AppConfig::default()
    })
}

fn rail() -> ThreadListing {
    ThreadListing::recent(None, 50, false).in_rail_order()
}

async fn rail_ids(app: &TestApp, who: &impl orch_app::Requester) -> Vec<ThreadId> {
    app.list_threads(who, rail())
        .await
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect()
}

fn pin() -> Arrangement {
    Arrangement {
        pinned: Some(true),
        ..Arrangement::default()
    }
}

/// A thread whose first turn is over: one a person can fork.
async fn finished(w: &World, app: &Arc<TestApp>, text: &str) -> ThreadRecord {
    let run = spawn_dispatcher(app, fast(), "d1");
    let t = create(app, &alice(), "plain", text).await;
    wait_state(app, &alice(), t.id, ThreadState::Done).await;
    run.shutdown().await;
    assert_eq!(state_of(w, t.id).await, ThreadState::Done);
    app.get_thread(&alice(), t.id).await.unwrap()
}

fn fork_id(n: u128) -> ThreadId {
    ThreadId(uuid::Uuid::from_u128(
        0x0190_0000_0000_7000_a000_0000_0000_0000 + n,
    ))
}

fn after(seq: i64) -> ForkRequest {
    ForkRequest {
        at: ForkAt::AfterTurn { seq, first: None },
        target: None,
        id: None,
    }
}

#[tokio::test]
async fn arranging_is_a_row_and_never_an_event() {
    let w = World::new();
    let app = w.app();
    let a = create(&app, &alice(), "plain", "one").await;
    let b = create(&app, &alice(), "plain", "two").await;
    let log = events(&app, &alice(), a.id).await;
    assert_eq!(rail_ids(&app, &alice()).await, vec![b.id, a.id]);

    let pinned = app.arrange_thread(&alice(), a.id, pin()).await.unwrap();
    assert!(pinned.pinned_at.is_some());
    assert_eq!(rail_ids(&app, &alice()).await, vec![a.id, b.id]);
    let wire = serde_json::to_value(&pinned).unwrap();
    assert_eq!(wire["pinned"], serde_json::json!(true));

    let archived = app
        .arrange_thread(
            &alice(),
            b.id,
            Arrangement {
                archived: Some(true),
                ..Arrangement::default()
            },
        )
        .await
        .unwrap();
    assert!(archived.archived_at.is_some());
    assert_eq!(rail_ids(&app, &alice()).await, vec![a.id]);

    // nothing of it is in the log, in the version or in the time of the last change
    assert_eq!(events(&app, &alice(), a.id).await, log);
    let (before, now) = (a.clone(), app.get_thread(&alice(), a.id).await.unwrap());
    assert_eq!(
        (before.version, before.last_seq, before.updated_at),
        (now.version, now.last_seq, now.updated_at)
    );
    // and the conversation goes on as if nothing had happened
    app.post_message(&alice(), a.id, "again".to_owned())
        .await
        .unwrap();
}

#[tokio::test]
async fn arranging_needs_thread_read_and_ownership_and_not_thread_write() {
    let w = World::new();
    let app = app_under_roles(&w);
    let user = principal("alice@example.com", &["user"]);
    let t = app
        .create_thread(
            &user,
            orch_app::NewThread {
                title: None,
                target: target("plain"),
                text: "hello".to_owned(),
            },
        )
        .await
        .unwrap();

    // a reader cannot write the thread and may arrange it
    let reader = principal("alice@example.com", &["reader"]);
    assert!(matches!(
        app.rename_thread(&reader, t.id, "x").await,
        Err(AppError::Forbidden { .. })
    ));
    assert!(
        app.arrange_thread(&reader, t.id, pin())
            .await
            .unwrap()
            .pinned_at
            .is_some()
    );
    // a role that cannot read the list cannot arrange it
    let writer = principal("alice@example.com", &["writer"]);
    match app.arrange_thread(&writer, t.id, pin()).await {
        Err(AppError::Forbidden { permission, .. }) => {
            assert_eq!(permission, Permission::ThreadRead);
        }
        other => panic!("{other:?}"),
    }
    // nobody arranges another person's thread, the administrator least of all: it is not there
    for other in [
        principal("bob@example.com", &["user"]),
        principal("bob@example.com", &["reader"]),
        principal("root@example.com", &["user", "reader"]),
    ] {
        assert!(
            matches!(
                app.arrange_thread(&other, t.id, pin()).await,
                Err(AppError::NotFound)
            ),
            "{}",
            other.user
        );
    }
    let missing = app.arrange_thread(&user, fork_id(99), pin()).await;
    assert!(matches!(missing, Err(AppError::NotFound)), "{missing:?}");
}

#[tokio::test]
async fn a_refusal_has_its_name_and_the_class_of_a_request_that_cannot_be_done() {
    let w = World::new();
    let app = w.app();
    let a = create(&app, &alice(), "plain", "one").await;
    let b = create(&app, &alice(), "plain", "two").await;
    let place = |place| Arrangement {
        place: Some(place),
        ..Arrangement::default()
    };
    for anchor in [fork_id(1), a.id] {
        let e = app
            .arrange_thread(&alice(), a.id, place(Place::Before(anchor)))
            .await
            .unwrap_err();
        assert!(
            matches!(&e, AppError::Arrangement { code, .. } if *code == "bad_anchor"),
            "{e:?}"
        );
        assert_eq!(e.class(), ErrorClass::Rejected);
    }
    // another person's thread is no anchor
    let theirs = create(&app, &bob(), "plain", "mine").await;
    let e = app
        .arrange_thread(&alice(), b.id, place(Place::After(theirs.id)))
        .await
        .unwrap_err();
    assert!(matches!(&e, AppError::Arrangement { code, .. } if *code == "bad_anchor"));
    assert_eq!(rail_ids(&app, &alice()).await, vec![b.id, a.id]);
}

#[tokio::test]
async fn a_fork_is_nested_under_the_row_the_person_sees() {
    let w = World::new();
    let app = w.app();
    let parent = finished(&w, &app, "echo one").await;

    // a fork from here is nested under its parent
    let f1 = app
        .fork_thread(&alice(), parent.id, after(1))
        .await
        .unwrap()
        .thread;
    assert_eq!(f1.rail_parent, Some(parent.id));
    // the lineage is the fork's own business and stays where it was
    assert_eq!(f1.forked_from.unwrap().thread_id, Some(parent.id));
    let wire = serde_json::to_value(&f1).unwrap();
    assert_eq!(
        wire["nestedUnder"],
        serde_json::json!(parent.id.to_string())
    );
    assert_eq!(rail_ids(&app, &alice()).await, vec![parent.id, f1.id]);

    // a fork of a fork is a sibling under the same root, and so is one made with its first message
    let f2 = app
        .fork_and_send(
            &alice(),
            f1.id,
            fork_id(2),
            1,
            None,
            FirstMessage {
                text: "echo two".to_owned(),
                message_id: None,
                ui_catalog: None,
                mentions: Vec::new(),
                run_id: None,
                origin: Origin::Agui,
            },
        )
        .await
        .unwrap()
        .thread;
    assert_eq!(f2.rail_parent, Some(parent.id));
    assert_eq!(
        rail_ids(&app, &alice()).await,
        vec![parent.id, f2.id, f1.id]
    );

    // an ejected fork is a row of its own, and its forks nest under it
    let ejected = app
        .arrange_thread(
            &alice(),
            f1.id,
            Arrangement {
                unnest: true,
                ..Arrangement::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ejected.rail_parent, None);
    assert_eq!(ejected.forked_from, f1.forked_from);
    let f3 = app
        .fork_thread(&alice(), f1.id, after(1))
        .await
        .unwrap()
        .thread;
    assert_eq!(f3.rail_parent, Some(f1.id));
}

#[tokio::test]
async fn an_edit_is_not_listed_and_a_fork_of_one_is_nested_under_its_root() {
    let w = World::new();
    let app = w.app();
    let parent = finished(&w, &app, "echo one").await;
    let edit = app
        .fork_thread(
            &alice(),
            parent.id,
            ForkRequest {
                at: ForkAt::Replace {
                    seq: 1,
                    text: "echo instead".to_owned(),
                    message_id: None,
                },
                target: None,
                id: None,
            },
        )
        .await
        .unwrap()
        .thread;
    assert_eq!(edit.forked_from.unwrap().kind, ForkKind::Edit);
    assert_eq!(edit.rail_parent, None);
    assert_eq!(rail_ids(&app, &alice()).await, vec![parent.id]);

    // finish the edit's turn, then fork from it: the person sees the conversation's root
    let run = spawn_dispatcher(&app, fast(), "d2");
    wait_state(&app, &alice(), edit.id, ThreadState::Done).await;
    run.shutdown().await;
    let fork = app
        .fork_thread(&alice(), edit.id, after(1))
        .await
        .unwrap()
        .thread;
    assert_eq!(fork.forked_from.unwrap().kind, ForkKind::Fork);
    assert_eq!(fork.rail_parent, Some(parent.id));
    assert_eq!(rail_ids(&app, &alice()).await, vec![parent.id, fork.id]);
}

#[tokio::test]
async fn a_fork_of_an_archived_thread_is_not_hidden_with_it() {
    let w = World::new();
    let app = w.app();
    let parent = finished(&w, &app, "echo one").await;
    app.arrange_thread(
        &alice(),
        parent.id,
        Arrangement {
            archived: Some(true),
            ..Arrangement::default()
        },
    )
    .await
    .unwrap();
    let fork = app
        .fork_thread(&alice(), parent.id, after(1))
        .await
        .unwrap()
        .thread;
    assert_eq!(
        fork.rail_parent, None,
        "it would be out of sight in a block that is"
    );
    assert_eq!(rail_ids(&app, &alice()).await, vec![fork.id]);
    let archived = app
        .list_threads(&alice(), rail().archived(ArchivedFilter::Only))
        .await
        .unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].id, parent.id);
    // and the store holds what the application said
    let stored = w.store.get_thread(None, fork.id).await.unwrap().unwrap();
    assert_eq!(stored.rail_parent, None);
}

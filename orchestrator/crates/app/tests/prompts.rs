//! A person's prompt library through the application (ADR 0070): who may save, share, read and render
//! a prompt, what a share lets another person do, the order of the checks, and that a write that
//! changes nothing keeps its revision.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use orch_app::{
    AgentScope, AppConfig, AppError, EditPrompt, Permission, Policy, PromptRefusal, RenderPrompt,
    RoleGrant,
};
use orch_core::prompt::{PromptId, PromptInput};
use orch_core::{Classify, ErrorClass};
use orch_ports::{Principal, PromptScope, Role};
use serde_json::{Value, json};
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
        max_tier: None,
    }
}

fn all_but_sharing() -> Vec<Permission> {
    Permission::ALL
        .into_iter()
        .filter(|p| *p != Permission::PromptsShare)
        .collect()
}

/// `plain` holds everything but `prompts.share`; `sharer` holds it too; `admin` holds `admin` and no sharing.
fn app_with_roles(w: &World, sharers: &[&str]) -> Arc<TestApp> {
    let mut roles: BTreeMap<Role, RoleGrant> = BTreeMap::new();
    roles.insert(Role::new("plain"), role(&all_but_sharing()));
    roles.insert(Role::new("team"), role(&all_but_sharing()));
    roles.insert(Role::new("sharer"), role(&all_but_sharing()));
    let mut with = all_but_sharing();
    with.push(Permission::PromptsShare);
    for name in sharers {
        roles.insert(Role::new(*name), role(&with));
    }
    roles.insert(
        Role::new("admin"),
        role(&[Permission::Admin, Permission::ThreadRead]),
    );
    w.app_with(AppConfig {
        policy: Policy::new(roles, None).unwrap(),
        ..AppConfig::default()
    })
}

fn question() -> Value {
    json!({"id": "who", "question": "Who?", "type": "text"})
}

fn input(name: &str, audience: &[&str]) -> PromptInput {
    PromptInput {
        name: name.to_owned(),
        title: format!("Title {name}"),
        description: String::new(),
        body: "Hello {{who}}".to_owned(),
        variables: vec![question()],
        audience: audience.iter().map(|s| (*s).to_owned()).collect(),
    }
}

fn answer(text: &str) -> Value {
    json!([{"id": "who", "values": [], "text": text}])
}

fn refusal(error: AppError) -> PromptRefusal {
    match error {
        AppError::Prompt(r) => r,
        other => panic!("not a prompt refusal: {other:?}"),
    }
}

#[tokio::test]
async fn a_person_saves_reads_lists_edits_and_deletes_their_own_prompt() {
    let w = World::new();
    let app = app_with_roles(&w, &["sharer"]);
    let alice = principal("alice@example.com", &["plain"]);
    let saved = app
        .create_prompt(&alice, input("greet", &[]))
        .await
        .unwrap();
    assert!(saved.own());
    let id = saved.prompt().id().clone();
    let page = app
        .list_prompts(&alice, PromptScope::All, None)
        .await
        .unwrap();
    assert_eq!(page.held, 1);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].variable_count, 1);
    let got = app.get_prompt(&alice, &id).await.unwrap();
    assert_eq!(got, saved);
    // an edit with the revision it read replaces the member and has a new revision
    let edited = app
        .edit_prompt(
            &alice,
            &id,
            EditPrompt {
                revision: Some(saved.revision()),
                title: Some("A new title".into()),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(edited.prompt().title(), "A new title");
    assert_ne!(edited.revision(), saved.revision());
    // the old revision is stale, and a change that changes nothing keeps the revision
    let stale = app
        .edit_prompt(
            &alice,
            &id,
            EditPrompt {
                revision: Some(saved.revision()),
                title: Some("Again".into()),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(stale), PromptRefusal::StaleRevision);
    let same = app
        .edit_prompt(
            &alice,
            &id,
            EditPrompt {
                revision: Some(edited.revision()),
                title: Some("A new title".into()),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(same.revision(), edited.revision());
    // delete: stale, then current, then gone
    let stale = app
        .delete_prompt(&alice, &id, Some(saved.revision()))
        .await
        .unwrap_err();
    assert_eq!(refusal(stale), PromptRefusal::StaleRevision);
    app.delete_prompt(&alice, &id, Some(edited.revision()))
        .await
        .unwrap();
    let gone = app.delete_prompt(&alice, &id, None).await.unwrap_err();
    assert_eq!(refusal(gone), PromptRefusal::NotFound);
}

#[tokio::test]
async fn sharing_needs_the_permission_before_the_values_are_read_and_unsharing_needs_nothing() {
    let w = World::new();
    let app = app_with_roles(&w, &["sharer"]);
    let plain = principal("plain@example.com", &["plain"]);
    let sharer = principal("sharer@example.com", &["sharer"]);
    // 403 before the values: a bad name that is also shared is `Forbidden`
    let mut bad = input("A bad name", &["team"]);
    bad.name = "NOT VALID".into();
    let denied = app.create_prompt(&plain, bad).await.unwrap_err();
    assert!(
        matches!(
            denied,
            AppError::Forbidden {
                permission: Permission::PromptsShare,
                ..
            }
        ),
        "{denied:?}"
    );
    assert_eq!(denied.class(), ErrorClass::Forbidden);
    // a private one is fine, and a change that adds an audience is not
    let mine = app.create_prompt(&plain, input("mine", &[])).await.unwrap();
    let add = app
        .edit_prompt(
            &plain,
            mine.prompt().id(),
            EditPrompt {
                revision: Some(mine.revision()),
                audience: Some(vec!["team".into()]),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(add, AppError::Forbidden { .. }));
    // the sharer shares; a person who then loses the permission is frozen: an edit that leaves it shared
    // is refused, unsharing and deleting are not
    let shared = app
        .create_prompt(&sharer, input("shared", &["team"]))
        .await
        .unwrap();
    let now_plain = app_with_roles(&w, &[]);
    let frozen = now_plain
        .edit_prompt(
            &sharer,
            shared.prompt().id(),
            EditPrompt {
                revision: Some(shared.revision()),
                title: Some("New".into()),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(frozen, AppError::Forbidden { .. }), "{frozen:?}");
    let withdrawn = now_plain
        .edit_prompt(
            &sharer,
            shared.prompt().id(),
            EditPrompt {
                revision: Some(shared.revision()),
                audience: Some(Vec::new()),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap();
    assert!(withdrawn.prompt().audience().is_private());
    now_plain
        .delete_prompt(&sharer, shared.prompt().id(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn the_audience_reads_and_renders_and_nobody_else_does_and_the_administrator_has_no_bypass() {
    let w = World::new();
    let app = app_with_roles(&w, &["sharer"]);
    let author = principal("sharer@example.com", &["sharer"]);
    let member = principal("member@example.com", &["plain", "team"]);
    let outsider = principal("outsider@example.com", &["plain"]);
    let admin = principal("root@example.com", &["admin"]);
    let shared = app
        .create_prompt(&author, input("shared", &["team"]))
        .await
        .unwrap();
    let id = shared.prompt().id().clone();

    let read = app.get_prompt(&member, &id).await.unwrap();
    assert!(!read.own());
    assert!(
        read.prompt().audience().is_private(),
        "the roles of the share are not the member's to know"
    );
    let rendered = app
        .render_prompt(
            &member,
            &id,
            RenderPrompt {
                revision: Some(shared.revision()),
                answers: Some(answer("there")),
                note: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(rendered.text, "Hello there");
    assert_eq!(rendered.revision, shared.revision());

    // a write by a member is the same 404 as for an id that never existed
    let edit = app
        .edit_prompt(
            &member,
            &id,
            EditPrompt {
                revision: Some(shared.revision()),
                title: Some("Mine".into()),
                ..EditPrompt::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(edit), PromptRefusal::NotFound);
    let delete = app.delete_prompt(&member, &id, None).await.unwrap_err();
    assert_eq!(refusal(delete), PromptRefusal::NotFound);
    for stranger in [&outsider, &admin] {
        let read = app.get_prompt(stranger, &id).await.unwrap_err();
        assert_eq!(refusal(read), PromptRefusal::NotFound);
        let render = app
            .render_prompt(
                stranger,
                &id,
                RenderPrompt {
                    answers: Some(answer("x")),
                    ..RenderPrompt::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(refusal(render), PromptRefusal::NotFound);
        assert!(
            app.list_prompts(stranger, PromptScope::All, None)
                .await
                .unwrap()
                .items
                .is_empty()
        );
    }
    // the member's list has it, as not theirs and with no audience
    let page = app
        .list_prompts(&member, PromptScope::Shared, None)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert!(!page.items[0].own && page.items[0].audience.is_none());
    // the author unshares it: gone at once
    app.edit_prompt(
        &author,
        &id,
        EditPrompt {
            revision: Some(shared.revision()),
            audience: Some(Vec::new()),
            ..EditPrompt::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        refusal(app.get_prompt(&member, &id).await.unwrap_err()),
        PromptRefusal::NotFound
    );
}

#[tokio::test]
async fn a_render_checks_the_revision_then_the_answer_then_the_size_and_writes_nothing() {
    let w = World::new();
    let app = app_with_roles(&w, &["sharer"]);
    let alice = principal("alice@example.com", &["plain"]);
    let saved = app
        .create_prompt(&alice, input("greet", &[]))
        .await
        .unwrap();
    let id = saved.prompt().id().clone();
    let stale = app
        .render_prompt(
            &alice,
            &id,
            RenderPrompt {
                revision: Some(orch_ports::PromptRevision::new(saved.revision().get() + 1)),
                answers: Some(answer("x")),
                note: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(stale), PromptRefusal::StaleRevision);
    for (request, rule) in [
        (
            RenderPrompt {
                answers: None,
                ..RenderPrompt::default()
            },
            "unknown",
        ),
        (
            RenderPrompt {
                answers: Some(answer("x")),
                note: true,
                ..RenderPrompt::default()
            },
            "unexpected",
        ),
        (
            RenderPrompt {
                answers: Some(json!([{"id": "who", "values": ["x"], "text": "y"}])),
                ..RenderPrompt::default()
            },
            "not_offered",
        ),
    ] {
        let PromptRefusal::InvalidAnswer(r) =
            refusal(app.render_prompt(&alice, &id, request).await.unwrap_err())
        else {
            panic!("not an answer refusal");
        };
        assert_eq!(r.rule, rule);
    }
    // a text that would be over the limit is refused before anything is built
    let mut big = input("big", &[]);
    big.body = "{{who}}".repeat(40);
    big.variables = vec![json!({"id": "who", "question": "Who?", "type": "longText"})];
    let big = app.create_prompt(&alice, big).await.unwrap();
    let long = app
        .render_prompt(
            &alice,
            big.prompt().id(),
            RenderPrompt {
                answers: Some(json!([{"id": "who", "values": [], "text": "x".repeat(2000)}])),
                ..RenderPrompt::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(refusal(long), PromptRefusal::RenderedTooLong);
    // it wrote nothing
    assert_eq!(
        app.get_prompt(&alice, &id).await.unwrap().revision(),
        saved.revision()
    );
}

#[tokio::test]
async fn the_limit_the_name_and_the_values_have_their_codes() {
    let w = World::new();
    let app = app_with_roles(&w, &["sharer"]);
    let alice = principal("alice@example.com", &["plain"]);
    let bad = {
        let mut i = input("x", &[]);
        i.body = "Hello {{ who }}".into();
        i
    };
    let PromptRefusal::Invalid(e) = refusal(app.create_prompt(&alice, bad).await.unwrap_err())
    else {
        panic!("not a rule of the page");
    };
    assert_eq!(
        (e.code, e.at.as_str(), e.offset),
        ("bad_reference", "body", Some(6))
    );
    app.create_prompt(&alice, input("dup", &[])).await.unwrap();
    let again = app
        .create_prompt(&alice, input("dup", &[]))
        .await
        .unwrap_err();
    assert_eq!(refusal(again), PromptRefusal::NameTaken);
    for n in 1..100 {
        app.create_prompt(&alice, input(&format!("p{n}"), &[]))
            .await
            .unwrap();
    }
    let over = app
        .create_prompt(&alice, input("over", &[]))
        .await
        .unwrap_err();
    assert_eq!(refusal(over), PromptRefusal::TooMany { limit: 100 });
    assert_eq!(
        app.list_prompts(&alice, PromptScope::Mine, None)
            .await
            .unwrap()
            .held,
        100
    );
    // an id that is no prompt's
    let nobody = PromptId::from_entropy([9; 16]);
    assert_eq!(
        refusal(app.get_prompt(&alice, &nobody).await.unwrap_err()),
        PromptRefusal::NotFound
    );
}

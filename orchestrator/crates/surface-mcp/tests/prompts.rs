//! The prompt library as MCP prompts (`prompts/list`, `prompts/get`), through an in-process rmcp
//! client over streamable HTTP against the in-memory stack (ADR 0070).
//!
//! What a client sees is what the person sees on `/api/prompts`: their own prompts and the ones
//! shared with their roles, never another person's private one; and `prompts/get` with arguments
//! answers the text the server's own render rules make, nothing sent and nothing written.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_app::{AgentScope, Permission, Policy, RoleGrant};
use orch_core::UserId;
use orch_ports::Role;
use rmcp::model::{GetPromptRequestParams, Prompt};
use serde_json::{Value, json};
use support::*;

/// A policy where the default role `user` may share prompts (the built-in `user` cannot), and
/// `chat` may not: so bob (no roles claim) shares, and carol (the claim `chat`) only reads what was
/// shared with her.
fn policy() -> Policy {
    let base: Vec<Permission> = Permission::ALL
        .into_iter()
        .filter(|p| *p != Permission::Admin && *p != Permission::PromptsShare)
        .collect();
    let mut sharing = base.clone();
    sharing.push(Permission::PromptsShare);
    let grant = |permissions: &[Permission]| RoleGrant {
        permissions: permissions.iter().copied().collect::<BTreeSet<_>>(),
        agents: AgentScope::from_patterns(["*"]),
        max_tier: None,
    };
    Policy::new(
        [
            (Role::new("user"), grant(&sharing)),
            (Role::new("chat"), grant(&base)),
        ]
        .into_iter()
        .collect(),
        Some(Role::new("user")),
    )
    .unwrap()
}

async fn harness() -> Harness {
    Harness::start_with(Options {
        policy: Some(policy()),
        ..Options::default()
    })
    .await
}

/// Saves a prompt over `/api/prompts` as `who`, and answers its id: the real surface, so that the
/// store holds exactly what a person's save makes.
async fn create(h: &Harness, who: &UserId, body: Value) -> String {
    let resp = h
        .http
        .post(format!("{}/api/prompts", h.base))
        .header("X-Auth-Request-Email", who.as_str())
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    assert_eq!(status, 201, "the save was refused: {text}");
    let view: Value = serde_json::from_str(&text).unwrap();
    view["id"].as_str().unwrap().to_owned()
}

fn names(prompts: &[Prompt]) -> Vec<String> {
    prompts.iter().map(|p| p.name.clone()).collect()
}

/// The prompt of `name`, or a panic that lists what was there.
fn prompt<'a>(prompts: &'a [Prompt], name: &str) -> &'a Prompt {
    prompts
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no prompt {name:?} in {:?}", names(prompts)))
}

/// Bob's shared prompt: one choice and one scale, so that `arguments` reach both kinds.
fn shared_prompt() -> Value {
    json!({
        "name": "team-review",
        "title": "Review a change",
        "description": "Reviews a change for one concern",
        "body": "Review for {{focus}} at depth {{depth}}.",
        "variables": [
            {
                "id": "focus",
                "question": "Focus?",
                "type": "select",
                "options": [
                    { "value": "security", "label": "Security" },
                    { "value": "speed", "label": "Speed" }
                ]
            },
            {
                "id": "depth",
                "question": "Depth?",
                "type": "scale",
                "to": 5
            }
        ],
        "audience": ["chat"]
    })
}

#[tokio::test]
async fn prompts_list_is_the_callers_own_and_the_ones_shared_with_their_roles() {
    let h = harness().await;
    let alice_id = create(
        &h,
        &UserId::new(ALICE),
        json!({ "name": "alices-notes", "title": "My notes", "body": "Hello." }),
    )
    .await;
    let bob_private = create(
        &h,
        &UserId::new(BOB),
        json!({ "name": "bobs-draft", "title": "A draft", "body": "Draft." }),
    )
    .await;
    create(&h, &UserId::new(BOB), shared_prompt()).await;

    // Alice: her own prompt, and none of bob's, not even the one he shared with `chat`.
    let alice = h.client(ALICE_TOKEN).await;
    // The server says it has prompts at `initialize`, or a client would not ask.
    let info = alice.peer_info().expect("the handshake");
    assert!(
        info.capabilities.prompts.is_some(),
        "prompts are not announced"
    );
    let listed = alice.list_prompts(None).await.unwrap().prompts;
    assert_eq!(names(&listed), ["alices-notes"], "{listed:?}");
    let mine = prompt(&listed, "alices-notes");
    assert_eq!(mine.title.as_deref(), Some("My notes"));
    assert_eq!(mine.arguments.as_deref().map(|a| a.len()), Some(0));

    // Bob: both of his, the shared one with its title, description and arguments.
    let bob = h.client(BOB_TOKEN).await;
    let listed = bob.list_prompts(None).await.unwrap().prompts;
    let mut bob_names = names(&listed);
    bob_names.sort();
    assert_eq!(bob_names, ["bobs-draft", "team-review"], "{listed:?}");
    let shared = prompt(&listed, "team-review");
    assert_eq!(shared.title.as_deref(), Some("Review a change"));
    assert_eq!(
        shared.description.as_deref(),
        Some("Reviews a change for one concern")
    );
    let arguments = shared.arguments.as_deref().unwrap();
    assert_eq!(
        arguments
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["focus", "depth"]
    );
    assert_eq!(arguments[0].description.as_deref(), Some("Focus?"));
    assert_eq!(arguments[0].required, Some(true));

    // Carol holds the role bob shared with: the shared prompt, and neither of their private ones.
    let carol = h.client(CAROL_TOKEN).await;
    let listed = carol.list_prompts(None).await.unwrap().prompts;
    assert_eq!(names(&listed), ["team-review"], "{listed:?}");

    // Nothing of the library is addressed by anything but its name: no id leaked into a name.
    assert!(!names(&listed).contains(&alice_id) && !names(&listed).contains(&bob_private));
}

#[tokio::test]
async fn prompts_get_renders_the_prompt_from_the_callers_arguments() {
    let h = harness().await;
    create(&h, &UserId::new(BOB), shared_prompt()).await;
    let carol = h.client(CAROL_TOKEN).await;

    let result = carol
        .get_prompt(
            GetPromptRequestParams::new("team-review").with_arguments(
                [("focus", json!("security")), ("depth", json!("3"))]
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(result.messages.len(), 1);
    let rmcp::model::ContentBlock::Text(text) = &result.messages[0].content else {
        panic!("not text: {:?}", result.messages[0].content);
    };
    // The server's render rules: a choice is its label, a scale its number.
    assert_eq!(text.text, "Review for Security at depth 3.");

    // An answer the rules refuse is a protocol error naming the argument, never a rendered prompt.
    let refused = carol
        .get_prompt(
            GetPromptRequestParams::new("team-review").with_arguments(
                [("focus", json!("nonsense")), ("depth", json!("3"))]
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v))
                    .collect(),
            ),
        )
        .await;
    let error = refused.expect_err("a value that is not offered was rendered anyway");
    assert!(error.to_string().contains("focus"), "{error}");
}

#[tokio::test]
async fn prompts_get_reports_another_persons_private_prompt_as_unknown() {
    let h = harness().await;
    create(
        &h,
        &UserId::new(BOB),
        json!({ "name": "bobs-draft", "title": "A draft", "body": "Draft." }),
    )
    .await;

    let alice = h.client(ALICE_TOKEN).await;
    let answered = alice
        .get_prompt(GetPromptRequestParams::new("bobs-draft"))
        .await;
    let error = answered.expect_err("another person's private prompt was served");
    assert!(error.to_string().contains("unknown prompt"), "{error}");

    // An id is no capability either: the prompt's id, as a name, is the same answer.
    let by_id = alice
        .get_prompt(GetPromptRequestParams::new("p-000000000000"))
        .await;
    assert!(by_id.is_err());
}

#[tokio::test]
async fn two_visible_prompts_of_the_same_name_are_listed_under_their_ids() {
    // MCP addresses a prompt by name and the store names are unique only per owner, so a name that
    // two visible prompts hold is ambiguous: both are published under their id instead.
    let h = harness().await;
    let shared_id = create(&h, &UserId::new(BOB), shared_prompt()).await;
    let own_id = create(
        &h,
        &UserId::new(CAROL),
        json!({ "name": "team-review", "title": "Mine", "body": "Mine." }),
    )
    .await;

    let carol = h.client(CAROL_TOKEN).await;
    let listed = carol.list_prompts(None).await.unwrap().prompts;
    let mut expected = vec![shared_id, own_id];
    expected.sort();
    let mut got = names(&listed);
    got.sort();
    assert_eq!(got, expected, "{listed:?}");
    assert!(prompt(&listed, &expected[0]).title.is_some());
}

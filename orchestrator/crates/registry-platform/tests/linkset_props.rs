//! The document reader never panics and never lists an id that is not an agent id, whatever it is
//! given.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::is_valid_agent_id;
use orch_ports::AgentAudience;
use orch_registry_platform::linkset::{self, PROFILE};
use proptest::prelude::*;
use serde_json::{Value, json};

/// Any JSON, nested a few levels.
fn any_json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,20}".prop_map(Value::String),
    ];
    leaf.prop_recursive(4, 48, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::vec(("[a-z]{1,8}", inner), 0..6)
                .prop_map(|pairs| Value::Object(pairs.into_iter().collect())),
        ]
    })
}

/// An item that is nearly right in a different way each time.
fn item() -> impl Strategy<Value = Value> {
    let href = prop_oneof![
        Just(json!("https://a.example.com/.well-known/agent-card.json")),
        Just(json!("http://b.internal:9000/card")),
        Just(json!("/relative")),
        Just(json!("ftp://x/y")),
        Just(json!("https://u:p@x.example.com/")),
        any_json(),
    ];
    let service = prop_oneof![
        "[a-z0-9][a-z0-9-]{0,10}".prop_map(|s| json!([s])),
        ".{0,70}".prop_map(|s| json!([s])),
        Just(json!([])),
        any_json(),
    ];
    let title = prop_oneof![
        Just(Value::Null),
        ".{0,300}".prop_map(Value::String),
        any_json()
    ];
    let tags = prop_oneof![
        Just(Value::Null),
        prop::collection::vec(".{0,80}", 0..20).prop_map(|t| json!(t)),
        any_json(),
    ];
    (href, service, title, tags, audience()).prop_map(|(href, service, title, tags, audience)| {
        let mut item = json!({"href": href, "service": service});
        if !title.is_null() {
            item["title"] = title;
        }
        if !tags.is_null() {
            item["tags"] = tags;
        }
        if !audience.is_null() {
            item["audience"] = audience;
        }
        item
    })
}

/// An `audience` member that is right, nearly right, or nothing like it.
fn audience() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        Just(json!(["*"])),
        Just(json!([])),
        prop::collection::vec("[!-~]{0,70}", 0..40).prop_map(|t| json!(t)),
        prop::collection::vec(".{0,70}", 0..5).prop_map(|t| json!(t)),
        prop::collection::vec(
            prop_oneof![Just(json!("*")), Just(json!("a")), any_json()],
            0..6
        )
        .prop_map(Value::Array),
        any_json(),
    ]
}

fn document(items: Vec<Value>) -> Vec<u8> {
    serde_json::to_vec(&json!({"linkset": [{"profile": [{"href": PROFILE}], "item": items}]}))
        .unwrap()
}

proptest! {
    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = linkset::parse(&bytes);
    }

    #[test]
    fn arbitrary_json_never_panics(value in any_json()) {
        let _ = linkset::parse(&serde_json::to_vec(&value).unwrap());
    }

    #[test]
    fn a_listed_item_is_always_an_agent(items in prop::collection::vec(item(), 0..40)) {
        let parsed = linkset::parse(&document(items.clone())).unwrap();
        prop_assert_eq!(parsed.items.len() + parsed.skipped.len(), items.len());
        let mut seen = std::collections::BTreeSet::new();
        for listed in &parsed.items {
            prop_assert!(is_valid_agent_id(&listed.service), "{:?}", listed);
            prop_assert!(seen.insert(listed.service.clone()), "an id once: {:?}", listed);
            let url = url::Url::parse(&listed.href).unwrap();
            prop_assert!(matches!(url.scheme(), "http" | "https"));
            prop_assert!(url.has_host());
            prop_assert!(url.username().is_empty() && url.password().is_none());
            prop_assert!(!listed.title.is_empty());
            prop_assert!(listed.title.chars().count() <= linkset::MAX_TITLE_CHARS);
            prop_assert!(listed.tags.len() <= linkset::MAX_TAGS);
            prop_assert!(listed.tags.iter().all(|t| !t.is_empty() && t.chars().count() <= linkset::MAX_TAG_CHARS));
            prop_assert!(
                listed.audience != AgentAudience::Static,
                "a registry's agent is never one of the deployment's own: {:?}", listed
            );
        }
    }

    /// An audience is read as the contract says or it is none: never anything else, whatever the
    /// member holds, and whatever it holds the item is kept.
    #[test]
    fn an_audience_is_read_or_it_is_none(member in audience()) {
        let mut item = json!({"href": "https://a.example.com/card", "service": ["a"]});
        if !member.is_null() {
            item["audience"] = member.clone();
        }
        let parsed = linkset::parse(&document(vec![item])).unwrap();
        prop_assert_eq!(parsed.items.len(), 1);
        let want = match &member {
            Value::Array(values) if values.iter().all(Value::is_string) => {
                AgentAudience::from_values(values.iter().filter_map(Value::as_str))
            }
            _ => AgentAudience::AdminsOnly,
        };
        prop_assert_eq!(&parsed.items[0].audience, &want);
        // it is reported when it was there and could not be read, and only then
        let was_there = member.as_array().is_none_or(|a| !a.is_empty()) && !member.is_null();
        prop_assert_eq!(
            !parsed.malformed_audience.is_empty(),
            was_there && want == AgentAudience::AdminsOnly
        );
    }
}

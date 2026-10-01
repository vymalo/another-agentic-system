//! The document reader never panics and never lists an id that is not an agent id, whatever it is
//! given.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::is_valid_agent_id;
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
    (href, service, title, tags).prop_map(|(href, service, title, tags)| {
        let mut item = json!({"href": href, "service": service});
        if !title.is_null() {
            item["title"] = title;
        }
        if !tags.is_null() {
            item["tags"] = tags;
        }
        item
    })
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
        }
    }
}

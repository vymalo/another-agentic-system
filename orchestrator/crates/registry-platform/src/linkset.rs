//! The registry document of `agent-registry/v1` (another-agentic-platform,
//! `docs/extensions/agent-registry-v1.md`): a linkset in the shape of an RFC 9727 API catalog,
//! with one `item` link per agent service.
//!
//! This module is pure: bytes in, items out. The rules are the contract's:
//!
//! - The document is **unreadable** (the whole source is unavailable, not empty) when it is not
//!   JSON, is not an object with a `linkset` array, has no context object carrying the v1
//!   `profile` link or more than one, exceeds 1 MiB or 500 items. Context objects for other
//!   versions are ignored. Nothing is truncated: dropping agents from a long list would hide
//!   them.
//! - An **invalid item** is skipped and the rest kept: a missing, relative or non-`http(s)`
//!   `href`, a `service` that is not an array of exactly one valid agent id, a `title` that is not
//!   a string. One that repeats an id already listed is skipped (the first wins).
//! - **Tags** are advisory: a malformed `tags` value (not an array of strings, an empty or
//!   over-long tag, more than 16) is dropped as a whole and the item is kept.
//! - The **audience** says who may use the agent (the contract's section *Audience*, ADR 0045). An
//!   item with none, an empty one or a malformed one is kept, and is for administrators only: the
//!   item is read, never guessed at ([`AgentAudience::from_values`] holds the grammar). A malformed
//!   one is reported in [`Document::malformed_audience`] for the operator.
//! - Members and attributes this reader does not know are ignored.
//!
//! Two rules are this reader's own, for safety: an `href` that carries a user name or password is
//! skipped (the card URL is shown to every user), and a title is cut at 200 characters.

use orch_core::is_valid_agent_id;
use orch_ports::AgentAudience;
use serde_json::Value;
use url::Url;

/// The profile that names this contract: the document's own version marker.
pub const PROFILE: &str = "https://agents.vymalo.com/registry/v1";
/// The most bytes of a document.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// The most items of a document.
pub const MAX_ITEMS: usize = 500;
/// The most tags of an item.
pub const MAX_TAGS: usize = 16;
/// The most characters of a tag.
pub const MAX_TAG_CHARS: usize = 64;
/// The most characters of a title that are kept.
pub const MAX_TITLE_CHARS: usize = 200;

/// One agent service of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The service id, an agent id ([`is_valid_agent_id`]).
    pub service: String,
    /// The agent card (or service base) URL: absolute `http` or `https`, with a host and no
    /// credentials.
    pub href: String,
    /// The display name: the item's `title`, or the service id when it has none.
    pub title: String,
    /// The labels, as given; empty when the item has none or they were malformed.
    pub tags: Vec<String>,
    /// Whom the agent is for: never [`AgentAudience::Static`]. An item with no `audience`, an
    /// empty one or a malformed one has [`AgentAudience::AdminsOnly`].
    pub audience: AgentAudience,
}

/// An item that was left out, and why (for the operator's log, never for a user).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Its position in the document's `item` array.
    pub index: usize,
    /// Why.
    pub reason: &'static str,
}

/// A readable document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    /// The valid items, in the document's order.
    pub items: Vec<Item>,
    /// The items that were left out.
    pub skipped: Vec<Skipped>,
    /// The service ids of the items that are listed with an `audience` that was there and
    /// malformed (so they are for administrators only), in the document's order.
    pub malformed_audience: Vec<String>,
}

/// Why a document is unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    /// More than [`MAX_BODY_BYTES`].
    #[error("the document is larger than {MAX_BODY_BYTES} bytes")]
    TooLarge,
    /// Not JSON.
    #[error("the document is not JSON")]
    NotJson,
    /// JSON, but not an object with a `linkset` array of objects, or an `item` that is not an
    /// array.
    #[error("the document is not a linkset")]
    NotALinkset,
    /// No context object carries the v1 profile link.
    #[error("no context object carries the profile {PROFILE}")]
    NoProfile,
    /// More than one context object carries the v1 profile link.
    #[error("more than one context object carries the profile {PROFILE}")]
    AmbiguousProfile,
    /// More than [`MAX_ITEMS`] items.
    #[error("the document lists more than {MAX_ITEMS} items")]
    TooManyItems,
}

/// Reads a registry document.
///
/// # Errors
/// [`Unreadable`] when the document as a whole cannot be used; an item that cannot is skipped.
pub fn parse(body: &[u8]) -> Result<Document, Unreadable> {
    if body.len() > MAX_BODY_BYTES {
        return Err(Unreadable::TooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| Unreadable::NotJson)?;
    let contexts = value
        .get("linkset")
        .and_then(Value::as_array)
        .ok_or(Unreadable::NotALinkset)?;
    let mut ours = contexts.iter().filter(|context| carries_profile(context));
    let context = ours.next().ok_or(Unreadable::NoProfile)?;
    if ours.next().is_some() {
        return Err(Unreadable::AmbiguousProfile);
    }
    let items: &[Value] = match context.get("item") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(items)) => items,
        Some(_) => return Err(Unreadable::NotALinkset),
    };
    if items.len() > MAX_ITEMS {
        return Err(Unreadable::TooManyItems);
    }
    let mut document = Document::default();
    for (index, item) in items.iter().enumerate() {
        match read_item(item) {
            Ok((item, audience_read)) => {
                if document.items.iter().any(|i| i.service == item.service) {
                    document.skipped.push(Skipped {
                        index,
                        reason: "the service id is listed twice (the first is kept)",
                    });
                } else {
                    if audience_read == AudienceRead::Malformed {
                        document.malformed_audience.push(item.service.clone());
                    }
                    document.items.push(item);
                }
            }
            Err(reason) => document.skipped.push(Skipped { index, reason }),
        }
    }
    Ok(document)
}

/// Whether the context object's `profile` links include this contract.
fn carries_profile(context: &Value) -> bool {
    context
        .get("profile")
        .and_then(Value::as_array)
        .is_some_and(|links| {
            links
                .iter()
                .any(|link| link.get("href").and_then(Value::as_str) == Some(PROFILE))
        })
}

/// How an item's `audience` member read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudienceRead {
    /// Absent, or `[]`: *none* by the contract, and nothing to report.
    Unset,
    /// Present, and well formed.
    Well,
    /// Present, and *malformed*: *none*, reported.
    Malformed,
}

fn read_item(item: &Value) -> Result<(Item, AudienceRead), &'static str> {
    let Some(item) = item.as_object() else {
        return Err("the item is not an object");
    };
    let href = item
        .get("href")
        .and_then(Value::as_str)
        .ok_or("the item has no href")?;
    let href = match Url::parse(href) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {
            if !url.username().is_empty() || url.password().is_some() {
                return Err("the href carries credentials");
            }
            url.to_string()
        }
        Ok(_) | Err(_) => return Err("the href is not an absolute http(s) URL"),
    };
    let service = match item
        .get("service")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([Value::String(service)]) if is_valid_agent_id(service) => service.clone(),
        _ => return Err("the service is not an array of one valid agent id"),
    };
    let title = match item.get("title") {
        None | Some(Value::Null) => service.clone(),
        Some(Value::String(title)) => {
            let title = title.trim();
            if title.is_empty() {
                service.clone()
            } else {
                title.chars().take(MAX_TITLE_CHARS).collect()
            }
        }
        Some(_) => return Err("the title is not a string"),
    };
    let tags = item.get("tags").and_then(read_tags).unwrap_or_default();
    let (audience, audience_read) = read_audience(item.get("audience"));
    Ok((
        Item {
            service,
            href,
            title,
            tags,
            audience,
        },
        audience_read,
    ))
}

/// The audience of an item (the contract's *Audience*, "the consumer's rule", point 1): the set of
/// its values when it is an array of texts that [`AgentAudience::from_values`] accepts; *none*
/// ([`AgentAudience::AdminsOnly`]) when the member is absent, is `[]`, is not an array, has an
/// element that is not a text, or is malformed in any other way. Never [`AgentAudience::Static`]:
/// an agent a registry lists is not one of the deployment's own.
fn read_audience(member: Option<&Value>) -> (AgentAudience, AudienceRead) {
    let none = AgentAudience::AdminsOnly;
    let Some(member) = member else {
        return (none, AudienceRead::Unset);
    };
    let Some(values) = member.as_array() else {
        return (none, AudienceRead::Malformed);
    };
    if values.is_empty() {
        return (none, AudienceRead::Unset);
    }
    let texts: Option<Vec<&str>> = values.iter().map(Value::as_str).collect();
    let Some(texts) = texts else {
        return (none, AudienceRead::Malformed);
    };
    match AgentAudience::from_values(texts) {
        AgentAudience::AdminsOnly => (none, AudienceRead::Malformed),
        audience => (audience, AudienceRead::Well),
    }
}

/// The tags, or `None` when they are malformed (then none are kept).
fn read_tags(tags: &Value) -> Option<Vec<String>> {
    let tags = tags.as_array()?;
    if tags.len() > MAX_TAGS {
        return None;
    }
    tags.iter()
        .map(|tag| {
            tag.as_str()
                .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TAG_CHARS)
                .map(str::to_owned)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn doc(items: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"linkset": [{
            "anchor": "https://platform.example.com/registry/v1/agents",
            "profile": [{"href": PROFILE}],
            "item": items,
        }]}))
        .unwrap()
    }

    fn item(service: &str) -> Value {
        json!({"href": format!("https://{service}.example.com/.well-known/agent-card.json"),
               "type": "application/json", "service": [service]})
    }

    #[test]
    fn the_contracts_own_example_reads() {
        let body = serde_json::to_vec(&json!({"linkset": [{
        "anchor": "https://platform.example.com/registry/v1/agents",
        "profile": [{"href": "https://agents.vymalo.com/registry/v1"}],
        "item": [
            {"href": "https://coder.agents.example.com/.well-known/agent-card.json",
             "type": "application/json", "title": "Coder", "service": ["coder"],
             "tags": ["coding", "git"]},
            {"href": "https://researcher.agents.example.com/.well-known/agent-card.json",
             "type": "application/json", "title": "Researcher", "service": ["researcher"]}
        ]}]}))
        .unwrap();
        let document = parse(&body).unwrap();
        assert!(document.skipped.is_empty());
        assert_eq!(
            document.items,
            [
                Item {
                    service: "coder".into(),
                    href: "https://coder.agents.example.com/.well-known/agent-card.json".into(),
                    title: "Coder".into(),
                    tags: vec!["coding".into(), "git".into()],
                    audience: AgentAudience::AdminsOnly,
                },
                Item {
                    service: "researcher".into(),
                    href: "https://researcher.agents.example.com/.well-known/agent-card.json"
                        .into(),
                    title: "Researcher".into(),
                    tags: vec![],
                    audience: AgentAudience::AdminsOnly,
                },
            ]
        );
    }

    #[test]
    fn a_document_with_no_items_is_an_empty_list_not_an_unreadable_one() {
        let body =
            serde_json::to_vec(&json!({"linkset": [{"profile": [{"href": PROFILE}]}]})).unwrap();
        assert_eq!(parse(&body).unwrap(), Document::default());
        assert_eq!(parse(&doc(json!([]))).unwrap(), Document::default());
    }

    #[test]
    fn the_title_defaults_to_the_service_id() {
        let mut no_title = item("coder");
        no_title["title"] = json!("   ");
        let document = parse(&doc(json!([item("plain"), no_title]))).unwrap();
        assert_eq!(document.items[0].title, "plain");
        assert_eq!(document.items[1].title, "coder");
        let mut long = item("long");
        long["title"] = json!("é".repeat(300));
        let document = parse(&doc(json!([long]))).unwrap();
        assert_eq!(document.items[0].title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn an_unreadable_document_is_never_a_partial_list() {
        let cases: [(&[u8], Unreadable); 6] = [
            (b"not json", Unreadable::NotJson),
            (b"[]", Unreadable::NotALinkset),
            (br#"{"linkset": {}}"#, Unreadable::NotALinkset),
            (br#"{"linkset": [{"item": []}]}"#, Unreadable::NoProfile),
            (
                br#"{"linkset": [{"profile": [{"href": "https://agents.vymalo.com/registry/v2"}]}]}"#,
                Unreadable::NoProfile,
            ),
            (
                br#"{"linkset": [{"profile": [{"href": "https://agents.vymalo.com/registry/v1"}]},
                                  {"profile": [{"href": "https://agents.vymalo.com/registry/v1"}]}]}"#,
                Unreadable::AmbiguousProfile,
            ),
        ];
        for (body, why) in cases {
            assert_eq!(
                parse(body).unwrap_err(),
                why,
                "{}",
                String::from_utf8_lossy(body)
            );
        }
        let body = serde_json::to_vec(&json!({"linkset": [{
            "profile": [{"href": PROFILE}], "item": {"href": "x"}}]}))
        .unwrap();
        assert_eq!(parse(&body).unwrap_err(), Unreadable::NotALinkset);
    }

    #[test]
    fn a_context_object_for_another_version_is_ignored_beside_ours() {
        let body = serde_json::to_vec(&json!({"linkset": [
            {"profile": [{"href": "https://agents.vymalo.com/registry/v2"}],
             "item": [{"href": "https://v2.example.com/", "service": ["v2-only"]}]},
            {"profile": [{"href": "https://agents.vymalo.com/registry/v2"}, {"href": PROFILE}],
             "item": [item("coder")]}
        ]}))
        .unwrap();
        let document = parse(&body).unwrap();
        assert_eq!(document.items.len(), 1);
        assert_eq!(document.items[0].service, "coder");
    }

    #[test]
    fn the_limits_are_refusals_not_truncations() {
        let many: Vec<Value> = (0..=MAX_ITEMS).map(|i| item(&format!("a{i}"))).collect();
        assert_eq!(
            parse(&doc(Value::Array(many))).unwrap_err(),
            Unreadable::TooManyItems
        );
        let ok: Vec<Value> = (0..MAX_ITEMS).map(|i| item(&format!("a{i}"))).collect();
        assert_eq!(
            parse(&doc(Value::Array(ok))).unwrap().items.len(),
            MAX_ITEMS
        );
        let big = vec![b' '; MAX_BODY_BYTES + 1];
        assert_eq!(parse(&big).unwrap_err(), Unreadable::TooLarge);
    }

    #[test]
    fn an_invalid_item_is_skipped_and_the_rest_are_kept() {
        let bad = [
            json!("not an object"),
            json!({"service": ["no-href"]}),
            json!({"href": "/relative/card.json", "service": ["relative"]}),
            json!({"href": "ftp://x.example.com/card", "service": ["ftp"]}),
            json!({"href": "https://x.example.com/card"}),
            json!({"href": "https://x.example.com/card", "service": "not-an-array"}),
            json!({"href": "https://x.example.com/card", "service": ["a", "b"]}),
            json!({"href": "https://x.example.com/card", "service": []}),
            json!({"href": "https://x.example.com/card", "service": ["Not_Valid"]}),
            json!({"href": "https://x.example.com/card", "service": [7]}),
            json!({"href": "https://x.example.com/card", "service": ["titled"], "title": 7}),
            json!({"href": "https://u:p@x.example.com/card", "service": ["creds"]}),
            json!({"href": "https://x.example.com/card", "service": [&"a".repeat(64)]}),
        ];
        let mut items = vec![item("first")];
        items.extend(bad.clone());
        items.push(item("last"));
        let document = parse(&doc(Value::Array(items))).unwrap();
        let listed: Vec<_> = document.items.iter().map(|i| i.service.as_str()).collect();
        assert_eq!(listed, ["first", "last"]);
        assert_eq!(document.skipped.len(), bad.len());
        assert_eq!(document.skipped[0].index, 1);
    }

    #[test]
    fn a_service_id_listed_twice_keeps_the_first() {
        let mut second = item("coder");
        second["title"] = json!("Second");
        let mut first = item("coder");
        first["title"] = json!("First");
        let document = parse(&doc(json!([first, second, item("other")]))).unwrap();
        assert_eq!(
            document
                .items
                .iter()
                .map(|i| i.title.as_str())
                .collect::<Vec<_>>(),
            ["First", "other"]
        );
        assert_eq!(document.skipped.len(), 1);
    }

    #[test]
    fn malformed_tags_are_dropped_as_a_whole_and_the_item_is_kept() {
        let long = "x".repeat(MAX_TAG_CHARS + 1);
        let fits = "x".repeat(MAX_TAG_CHARS);
        let many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        let few: Vec<String> = (0..MAX_TAGS).map(|i| format!("t{i}")).collect();
        let cases = [
            (json!("coding"), 0),
            (json!(["coding", 7]), 0),
            (json!(["coding", ""]), 0),
            (json!(["coding", long]), 0),
            (json!(many), 0),
            (json!(["coding", fits]), 2),
            (json!(few), MAX_TAGS),
            (json!([]), 0),
        ];
        for (tags, kept) in cases {
            let mut with = item("coder");
            with["tags"] = tags.clone();
            let document = parse(&doc(json!([with]))).unwrap();
            assert_eq!(document.items.len(), 1, "{tags}");
            assert_eq!(document.items[0].tags.len(), kept, "{tags}");
            assert!(document.skipped.is_empty(), "{tags}");
        }
    }

    #[test]
    fn members_and_attributes_this_reader_does_not_know_are_ignored() {
        let body = serde_json::to_vec(&json!({"linkset": [{
            "profile": [{"href": PROFILE, "type": "text/html"}], "anchor": 7, "x-extra": {},
            "item": [{"href": "https://a.example.com/card", "service": ["a"], "type": 3,
                      "hreflang": ["en"], "x-owner": "someone"}]}], "other": true}))
        .unwrap();
        assert_eq!(parse(&body).unwrap().items.len(), 1);
    }

    #[test]
    fn the_href_is_kept_as_the_url_it_names() {
        let mut bare = item("a");
        bare["href"] = json!("https://a.example.com");
        let mut port = item("b");
        port["href"] = json!("http://b.internal:9000/.well-known/agent-card.json");
        let document = parse(&doc(json!([bare, port]))).unwrap();
        assert_eq!(document.items[0].href, "https://a.example.com/");
        assert_eq!(
            document.items[1].href,
            "http://b.internal:9000/.well-known/agent-card.json"
        );
    }

    // ---- the audience (the contract's section "Audience") ----

    fn with_audience(audience: Value) -> Vec<u8> {
        let mut with = item("coder");
        with["audience"] = audience;
        doc(json!([with]))
    }

    fn read_audience_of(audience: Value) -> AgentAudience {
        let document = parse(&with_audience(audience)).unwrap();
        assert_eq!(document.items.len(), 1, "the item is kept");
        assert!(document.skipped.is_empty());
        document.items.into_iter().next().unwrap().audience
    }

    #[test]
    fn the_contracts_example_item_with_its_audience_reads() {
        let body = serde_json::to_vec(&json!({"linkset": [{
        "anchor": "https://platform.example.com/registry/v1/agents",
        "profile": [{"href": PROFILE}],
        "item": [{
            "href": "http://coder-me.another-agentic-system.svc:8080/.well-known/agent-card.json",
            "type": "application/json", "title": "Coder (me)", "service": ["coder-me"],
            "tags": ["coding", "git"], "audience": ["agent.use:coder-me"]
        }]}]}))
        .unwrap();
        let document = parse(&body).unwrap();
        assert_eq!(
            document.items[0].audience,
            AgentAudience::from_values(["agent.use:coder-me"])
        );
        assert!(document.malformed_audience.is_empty());
    }

    #[test]
    fn an_absent_audience_and_an_empty_one_are_none_and_are_not_reported() {
        let absent = parse(&doc(json!([item("coder")]))).unwrap();
        assert_eq!(absent.items[0].audience, AgentAudience::AdminsOnly);
        assert!(absent.malformed_audience.is_empty());
        let empty = parse(&with_audience(json!([]))).unwrap();
        assert_eq!(empty.items[0].audience, AgentAudience::AdminsOnly);
        assert!(empty.malformed_audience.is_empty());
    }

    #[test]
    fn the_three_cases_a_server_publishes() {
        assert_eq!(read_audience_of(json!(["*"])), AgentAudience::Everyone);
        // a repeat of "*" is a repeat
        assert_eq!(read_audience_of(json!(["*", "*"])), AgentAudience::Everyone);
        let AgentAudience::Roles(roles) = read_audience_of(json!(["agent.use:b", "agent.use:a"]))
        else {
            panic!("values");
        };
        assert_eq!(
            roles.iter().collect::<Vec<_>>(),
            ["agent.use:a", "agent.use:b"],
            "a set, in byte order whatever the document's"
        );
    }

    #[test]
    fn every_malformed_audience_is_none_the_item_is_kept_and_it_is_reported() {
        let long = "x".repeat(65);
        let many: Vec<String> = (0..33).map(|i| format!("r{i}")).collect();
        let malformed = [
            json!("agent.use:coder"),
            json!({"0": "x"}),
            json!(7),
            json!(true),
            json!(null),
            json!([7]),
            json!(["ok", null]),
            json!(["ok", ["nested"]]),
            json!([{"role": "x"}]),
            json!([""]),
            json!(["has space"]),
            json!([" lead"]),
            json!(["trail "]),
            json!(["tab\there"]),
            json!(["new\nline"]),
            json!(["trailing-newline\n"]),
            json!(["del\u{7f}"]),
            json!(["café"]),
            json!([long]),
            json!(many),
            json!(["*", "x"]),
            json!(["x", "*"]),
        ];
        for audience in malformed {
            let document = parse(&with_audience(audience.clone())).unwrap();
            assert_eq!(document.items.len(), 1, "{audience}: the item is kept");
            assert_eq!(
                document.items[0].audience,
                AgentAudience::AdminsOnly,
                "{audience}"
            );
            assert_eq!(document.malformed_audience, ["coder"], "{audience}");
            assert!(document.skipped.is_empty(), "{audience}");
        }
    }

    #[test]
    fn the_edges_of_the_grammar_read_as_the_contract_says() {
        let max = "x".repeat(64);
        let thirty_two: Vec<String> = (0..32).map(|i| format!("r{i:02}")).collect();
        for ok in [
            json!(["!"]),
            json!(["~"]),
            json!([max]),
            json!(thirty_two),
            // repeats do not count twice
            json!(["a", "a", "a"]),
        ] {
            assert!(
                matches!(read_audience_of(ok.clone()), AgentAudience::Roles(_)),
                "{ok}"
            );
        }
    }

    #[test]
    fn an_audience_reaches_no_other_attribute_and_a_bad_one_hides_nothing_else() {
        let mut bad = item("bad");
        bad["audience"] = json!(7);
        let mut good = item("good");
        good["audience"] = json!(["*"]);
        let document = parse(&doc(json!([bad, good]))).unwrap();
        let by_service = |s: &str| {
            document
                .items
                .iter()
                .find(|i| i.service == s)
                .map(|i| i.audience.clone())
        };
        assert_eq!(by_service("bad"), Some(AgentAudience::AdminsOnly));
        assert_eq!(by_service("good"), Some(AgentAudience::Everyone));
        assert_eq!(document.malformed_audience, ["bad"]);
    }
}

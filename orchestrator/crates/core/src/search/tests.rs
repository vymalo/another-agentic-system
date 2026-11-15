#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The vectors of `docs/api/examples/thread-search/` and the page's own numbers.

use proptest::prelude::*;
use serde_json::Value;

use super::*;

const FOLD: &str = include_str!("../../../../../docs/api/examples/thread-search/fold.json");
const QUERY: &str = include_str!("../../../../../docs/api/examples/thread-search/query.json");
const SNIPPET: &str = include_str!("../../../../../docs/api/examples/thread-search/snippet.json");
const PAGE: &str = include_str!("../../../../../docs/api/thread-search-v1.md");

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("a vector is JSON")
}

/// A vector's text: a string, or `{repeat, times, suffix?}`.
fn expand(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => {
            let unit = o["repeat"].as_str().expect("repeat");
            let times = usize::try_from(o["times"].as_u64().expect("times")).expect("times fit");
            let suffix = o.get("suffix").and_then(Value::as_str).unwrap_or("");
            format!("{}{suffix}", unit.repeat(times))
        }
        other => panic!("not a text: {other}"),
    }
}

fn terms_of(v: &Value) -> SearchTerms {
    SearchTerms::from_keys(
        v.as_array()
            .expect("terms")
            .iter()
            .map(expand)
            .collect::<Vec<_>>(),
    )
    .expect("the vector's terms are terms")
}

#[test]
fn every_word_and_key_of_the_fold_vectors() {
    let file = json(FOLD);
    let cases = file["cases"].as_array().expect("cases");
    assert!(cases.len() > 20);
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let text = expand(&case["text"]);
        let got = words(&text);
        if let Some(count) = case.get("wordCount") {
            assert_eq!(
                got.words.len() as u64,
                count.as_u64().expect("count"),
                "{name}"
            );
            assert!(got.more, "{name}: there are more words");
            assert_eq!(
                got.words.last().and_then(|w| w.key.as_deref()),
                case["lastKey"].as_str(),
                "{name}"
            );
            continue;
        }
        let want: Vec<(u64, u64, Option<String>)> = case["words"]
            .as_array()
            .expect("words")
            .iter()
            .map(|w| {
                (
                    w["start"].as_u64().expect("start"),
                    w["end"].as_u64().expect("end"),
                    (!w["key"].is_null()).then(|| expand(&w["key"])),
                )
            })
            .collect();
        let have: Vec<(u64, u64, Option<String>)> = got
            .words
            .iter()
            .map(|w| (w.start as u64, w.end as u64, w.key.clone()))
            .collect();
        assert_eq!(have, want, "{name}");
        assert!(!got.more, "{name}");
    }
}

#[test]
fn every_query_of_the_query_vectors() {
    let file = json(QUERY);
    let cases = file["cases"].as_array().expect("cases");
    assert!(cases.len() > 20);
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let q = expand(&case["q"]);
        let got = SearchTerms::parse(&q);
        if let Some(code) = case.get("error") {
            assert_eq!(
                got.expect_err(name).code(),
                code.as_str().expect("code"),
                "{name}"
            );
        } else {
            let want: Vec<String> = case["terms"]
                .as_array()
                .expect("terms")
                .iter()
                .map(expand)
                .collect();
            assert_eq!(got.expect(name).as_slice(), want.as_slice(), "{name}");
        }
    }
}

#[test]
fn every_snippet_of_the_snippet_vectors() {
    let file = json(SNIPPET);
    let cases = file["cases"].as_array().expect("cases");
    assert!(cases.len() > 20);
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let text = expand(&case["text"]);
        let terms = terms_of(&case["terms"]);
        let got = snippet(&text, &terms);
        match &case["expect"] {
            Value::Null => assert_eq!(got, None, "{name}"),
            want => {
                let got = got.unwrap_or_else(|| panic!("{name}: a snippet"));
                assert_eq!(got.text, want["text"].as_str().expect("text"), "{name}");
                assert_eq!(got.cut_start, want["cutStart"], "{name}");
                assert_eq!(got.cut_end, want["cutEnd"], "{name}");
                let marks: Vec<(u64, u64)> = got
                    .matches
                    .iter()
                    .map(|m| (m.start as u64, m.end as u64))
                    .collect();
                let wanted: Vec<(u64, u64)> = want["matches"]
                    .as_array()
                    .expect("matches")
                    .iter()
                    .map(|m| {
                        (
                            m["start"].as_u64().expect("s"),
                            m["end"].as_u64().expect("e"),
                        )
                    })
                    .collect();
                assert_eq!(marks, wanted, "{name}");
            }
        }
    }
}

/// The constants table of the page is the code's.
#[test]
fn the_page_states_the_numbers_the_code_uses() {
    let value = |name: &str| -> usize {
        let row = PAGE
            .lines()
            .find(|l| l.starts_with(&format!("| `{name}` |")))
            .unwrap_or_else(|| panic!("the page has a row for {name}"));
        row.split('|')
            .nth(2)
            .expect("a value cell")
            .trim()
            .parse()
            .expect("a number")
    };
    assert_eq!(value("MAX_QUERY_LENGTH"), MAX_QUERY_LENGTH);
    assert_eq!(value("MAX_TERMS"), MAX_TERMS);
    assert_eq!(value("MIN_TERM"), MIN_TERM);
    assert_eq!(value("MAX_KEY"), MAX_KEY);
    assert_eq!(value("MAX_WORDS"), MAX_WORDS);
    assert_eq!(value("MAX_MESSAGE_HITS"), MAX_MESSAGE_HITS);
    assert_eq!(value("CONTEXT_WORDS"), CONTEXT_WORDS);
    assert_eq!(value("CONTEXT_UNITS"), CONTEXT_UNITS);
    assert_eq!(value("WINDOW_UNITS"), WINDOW_UNITS);
    assert_eq!(value("EDGE_UNITS"), EDGE_UNITS);
    for code in ["query_empty", "query_too_long"] {
        assert!(PAGE.contains(code), "{code}");
    }
}

#[test]
fn a_term_is_letters_and_numbers_and_nothing_else() {
    for bad in [
        "a:*",
        "it's",
        "a b",
        "a|b",
        "ab'",
        "ab)",
        "x\"",
        "a&b",
        "ab!",
        "a<->",
        "",
        "a",
        "é\u{301}",
    ] {
        assert!(
            SearchTerms::from_keys(vec![bad.to_owned()]).is_err(),
            "{bad:?} is no term"
        );
    }
    assert!(SearchTerms::from_keys(vec!["ab".into(); 9]).is_err());
    assert!(SearchTerms::from_keys(vec!["a".repeat(65)]).is_err());
    assert!(SearchTerms::from_keys(vec!["привет".into(), "東京".into(), "2026".into()]).is_ok());
    // whatever a person types, the terms that come out pass the same check
    for q in [
        "' OR 1=1 --",
        "foo:* | bar & !baz <-> qux",
        "x'); DROP TABLE threads;--",
        "\"quoted\" (paren) [bracket] {brace} \\ backslash `tick`",
        "\u{ff07}full\u{ff07}width \u{2028}line\u{2029}sep",
    ] {
        let terms = SearchTerms::parse(q).expect("a query");
        assert!(
            SearchTerms::from_keys(terms.as_slice().to_vec()).is_ok(),
            "{q}"
        );
    }
}

#[test]
fn matching_needs_every_term_in_one_document() {
    let terms = SearchTerms::parse("tomatoes basil").expect("terms");
    assert!(terms.matches("Start with tomatoes and basil."));
    assert!(!terms.matches("tomatoes are red"));
    assert!(!terms.matches("basil is green"));
    assert!(SearchTerms::parse("a").expect("terms").is_empty());
    assert!(!SearchTerms::parse("a").expect("terms").matches("a a a"));
}

fn event(seq: i64, body: crate::EventBody) -> Event {
    Event {
        seq,
        thread_id: crate::ThreadId(uuid::Uuid::nil()),
        at: jiff::Timestamp::UNIX_EPOCH,
        actor: crate::Actor::system(),
        body,
    }
}

fn agent_message(
    seq: i64,
    id: &str,
    text: &str,
    is_final: bool,
    purpose: Option<MessagePurpose>,
) -> Event {
    event(
        seq,
        crate::EventBody::AgentMessage(crate::AgentMessageData {
            text: text.to_owned(),
            message_id: id.to_owned(),
            is_final,
            purpose,
            via: None,
        }),
    )
}

#[test]
fn only_a_persons_messages_and_an_agents_final_answers_are_searched_and_a_later_final_replaces() {
    let log = vec![
        event(
            1,
            crate::EventBody::UserMessage(crate::UserMessageData::new("question")),
        ),
        agent_message(2, "a", "partial words", false, None),
        agent_message(3, "b", "working words", true, Some(MessagePurpose::Working)),
        agent_message(4, "c", "first draft", true, Some(MessagePurpose::Answer)),
        agent_message(5, "c", "second draft", true, Some(MessagePurpose::Answer)),
        agent_message(6, "d", "an answer", true, None),
        agent_message(
            7,
            "d",
            "now only working words",
            true,
            Some(MessagePurpose::Working),
        ),
    ];
    let seen: Vec<(i64, SearchRole, String)> = searched_messages(&log)
        .into_iter()
        .map(|m| (m.seq, m.role, m.text))
        .collect();
    assert_eq!(
        seen,
        vec![
            (1, SearchRole::Person, "question".to_owned()),
            (5, SearchRole::Agent, "second draft".to_owned()),
        ]
    );
}

#[test]
fn a_word_of_over_3000_words_is_cut_where_the_contract_says() {
    let text = "a ".repeat(MAX_WORDS) + "zebra";
    assert!(!SearchTerms::parse("zebra").expect("terms").matches(&text));
    let text = "zebra ".to_owned() + &"a ".repeat(MAX_WORDS);
    assert!(SearchTerms::parse("zebra").expect("terms").matches(&text));
}

fn utf16_slice(s: &str, start: usize, end: usize) -> Option<String> {
    let units: Vec<u16> = s.encode_utf16().collect();
    String::from_utf16(units.get(start..end)?).ok()
}

fn text_strategy() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        Just("caf\u{e9}".to_owned()),
        Just("C\u{153}ur".to_owned()),
        Just("login".to_owned()),
        Just("\u{1f389}".to_owned()),
        Just("\u{6771}\u{4eac}".to_owned()),
        Just("\u{41f}\u{440}\u{438}\u{432}\u{435}\u{442}".to_owned()),
        Just("\u{1d49c}\u{1d4b7}".to_owned()),
        Just("x\u{301}".to_owned()),
        Just("1234".to_owned()),
        "[a-z]{1,9}",
        Just(", ".to_owned()),
        Just(" ".to_owned()),
        Just("\n".to_owned()),
        Just("\u{a0}".to_owned()),
        Just("-".to_owned()),
    ];
    proptest::collection::vec(piece, 0..60).prop_map(|parts| parts.join(" "))
}

proptest! {
    /// A snippet's offsets are inside its text, on whole characters, and each marked word folds to a
    /// key a term starts; a one-term query has a snippet exactly when it matches.
    #[test]
    fn a_snippet_marks_words_that_match_and_never_splits_a_character(
        text in text_strategy(),
        pick in 0_usize..40,
        cut in 2_usize..6,
    ) {
        let keys = document_keys(&text);
        prop_assume!(!keys.is_empty());
        let key = &keys[pick % keys.len()];
        let term: String = key.chars().take(cut).collect();
        let Ok(terms) = SearchTerms::from_keys(vec![term.clone()]) else {
            return Ok(());
        };
        let found = snippet(&text, &terms);
        prop_assert_eq!(found.is_some(), terms.matches(&text));
        let found = found.expect("a key of the text starts a term made of it");
        let length = found.text.encode_utf16().count();
        prop_assert!(!found.matches.is_empty());
        for m in &found.matches {
            prop_assert!(m.start < m.end && m.end <= length);
            let word = utf16_slice(&found.text, m.start, m.end)
                .expect("a marked word is whole characters");
            prop_assert!(fold(&word).starts_with(&term));
        }
        prop_assert!(found.matches.windows(2).all(|w| w[0].end <= w[1].start));
        prop_assert!(found.text.chars().count() <= text.chars().count());
    }

    /// Every key of a document, and of a query, passes the check a store relies on.
    #[test]
    fn keys_and_terms_are_letters_and_numbers_only(text in text_strategy()) {
        for key in document_keys(&text) {
            prop_assert!(key.chars().count() <= MAX_KEY);
            prop_assert!(key.chars().all(char::is_alphanumeric));
        }
        let terms = SearchTerms::parse(&format!("x {text}")).unwrap_or_default();
        prop_assert!(SearchTerms::from_keys(terms.as_slice().to_vec()).is_ok());
    }
}

/// Inputs chosen to move when a table behind the fold moves: letters added to Unicode in versions 8 to 14
/// (their lower-case mappings come from Rust's `to_lowercase`), the title-case digraphs, compatibility
/// decompositions (`unicode-normalization`), letters that only NFKD makes of a symbol, and the final sigma.
/// The expected keys are what `FOLD_VERSION` 1 makes of them.
const PINNED_FOLDS: &[(&str, &str)] = &[
    ("İstanbul", "istanbul"),
    ("ΌΣΟΣ", "οσοσ"),
    ("ΑΣ", "ασ"),
    ("Ა", "ა"),
    ("𐕰", "𐖗"),
    ("Ꟑ", "ꟑ"),
    ("ǅ", "dz"),
    ("ǈ", "lj"),
    ("ﬃ", "ffi"),
    ("½", "12"),
    ("ﷺ", "صلىاللهعليهوسلم"),
    ("K", "k"),
    ("Å", "a"),
    ("ẞ", "ss"),
    ("𞤀", "𞤢"),
    ("Ꭰ", "ꭰ"),
    ("ǰ", "j"),
    ("ᾈ", "α"),
    ("ꟻ", "ꟻ"),
];

#[test]
fn the_fold_is_pinned_raise_fold_version_when_it_moves() {
    assert_eq!(
        FOLD_VERSION, 1,
        "FOLD_VERSION changed: update PINNED_FOLDS to what the new version makes, in the same change"
    );
    for (word, key) in PINNED_FOLDS {
        assert_eq!(
            fold(word),
            *key,
            "the key of {word:?} moved: the folding rules or a Unicode table behind them changed \
             (a dependency bump?). Every key stored in the search index is now different from the \
             key a query makes. RAISE `orch_core::search::FOLD_VERSION` (and set PINNED_FOLDS to the \
             new keys), then run `orchestrator search backfill` on every deployment"
        );
    }
}

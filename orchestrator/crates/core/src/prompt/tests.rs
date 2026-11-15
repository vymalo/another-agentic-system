#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The vectors of `docs/api/examples/prompts/`, the answer blocks of the UI catalog page, and the
//! page's own numbers.

use std::collections::BTreeSet;

use proptest::prelude::*;
use serde_json::{Value, json};

use super::*;

const REFERENCES: &str = include_str!("../../../../../docs/api/examples/prompts/references.json");
const PROMPTS: &str = include_str!("../../../../../docs/api/examples/prompts/prompts.json");
const RENDER: &str = include_str!("../../../../../docs/api/examples/prompts/render.json");
const PAGE_TEXT: &str = include_str!("../../../../../docs/api/prompts-v1.md");
const CATALOG_PAGE: &str = include_str!("../../../../../docs/api/ui-catalog-v1.md");
const CATALOG_JSON: &str =
    include_str!("../../../../../web/src/features/chat/lib/a2ui/catalog/catalog.json");

fn json(text: &str) -> Value {
    serde_json::from_str(text).expect("a vector is JSON")
}

/// `{"$concat": [piece, ...]}` is the pieces joined (a piece: a string or `{"$repeat", "times"}`),
/// anywhere in a vector.
fn expand(v: &Value) -> Value {
    match v {
        Value::Array(a) => Value::Array(a.iter().map(expand).collect()),
        Value::Object(o) => {
            if let Some(Value::Array(pieces)) = o.get("$concat") {
                let mut out = String::new();
                for p in pieces {
                    match p {
                        Value::String(s) => out.push_str(s),
                        other => out.push_str(&repeat(other)),
                    }
                }
                Value::String(out)
            } else if o.contains_key("$repeat") {
                Value::String(repeat(v))
            } else {
                Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x))).collect())
            }
        }
        other => other.clone(),
    }
}

fn repeat(v: &Value) -> String {
    let unit = v["$repeat"].as_str().expect("$repeat is text");
    let times = usize::try_from(v["times"].as_u64().expect("times")).expect("times fit");
    unit.repeat(times)
}

fn block(name: &str) -> Value {
    let open = format!("```json {name}\n");
    let start = CATALOG_PAGE
        .find(&open)
        .unwrap_or_else(|| panic!("the catalog page has no {name} block"))
        + open.len();
    let end = CATALOG_PAGE[start..].find("```").expect("the block ends") + start;
    json(&CATALOG_PAGE[start..end])
}

fn input(create: &Value) -> Option<PromptInput> {
    let o = create.as_object()?;
    let text = |k: &str| match o.get(k) {
        Some(Value::String(s)) => Some(Some(s.clone())),
        None => Some(None),
        _ => None,
    };
    let variables = match o.get("variables") {
        None => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        _ => return None,
    };
    let audience = match o.get("audience") {
        None => Vec::new(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    Some(PromptInput {
        name: text("name")??,
        title: text("title")??,
        description: text("description")?.unwrap_or_default(),
        body: text("body")??,
        variables,
        audience,
    })
}

fn one_prompt(question: &Value) -> PromptInput {
    PromptInput {
        name: "p".into(),
        title: "T".into(),
        description: String::new(),
        body: format!("{{{{{}}}}}", question["id"].as_str().unwrap_or("q")),
        variables: vec![question.clone()],
        audience: Vec::new(),
    }
}

fn variables_of(questions: &[Value]) -> Variables {
    let body: String = questions
        .iter()
        .map(|q| format!("{{{{{}}}}}", q["id"].as_str().unwrap()))
        .collect::<Vec<_>>()
        .join(" ");
    let definition = PromptDefinition::new(
        PromptId::from_entropy([1; 16]),
        PromptInput {
            name: "p".into(),
            title: "T".into(),
            description: String::new(),
            body,
            variables: questions.to_vec(),
            audience: Vec::new(),
        },
    )
    .expect("these questions are a prompt");
    definition.variables().clone()
}

// ------------------------------------------------------------------------------------- references

#[test]
fn the_references_of_every_vector() {
    let file = json(REFERENCES);
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() > 30);
    for c in cases {
        let body = c["body"].as_str().unwrap();
        let got = parse_body(body);
        let expect = &c["expect"];
        if let Some(segments) = expect["segments"].as_array() {
            let want: Vec<Segment> = segments
                .iter()
                .map(|s| match s {
                    Value::String(t) => Segment::Literal(t.clone()),
                    other => Segment::Ref(other["ref"].as_str().unwrap().to_owned()),
                })
                .collect();
            assert_eq!(got, Ok(want), "{}", c["name"]);
        } else {
            let offset = u32::try_from(expect["offset"].as_u64().unwrap()).unwrap();
            assert_eq!(got, Err(BadReference { offset }), "{}", c["name"]);
        }
    }
}

// ------------------------------------------------------------------------------------- the saves

#[test]
fn every_whole_save_of_prompts_json() {
    let file = json(PROMPTS);
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() > 100);
    let (mut checked, mut shape) = (0, 0);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let create = expand(&c["create"]);
        let expect = &c["expect"];
        let Some(input) = input(&create) else {
            // the shape of the request is the API's: a body of the wrong shape is 400 `bad_request`
            assert_eq!(expect["code"], "bad_request", "{name}");
            shape += 1;
            continue;
        };
        if expect["code"] == "bad_request" {
            shape += 1; // a member nobody has, or a missing one: also the API's
            continue;
        }
        checked += 1;
        let got = validate(&input);
        if expect == "ok" {
            assert_eq!(got, Ok(()), "{name}");
            let definition = PromptDefinition::new(PromptId::from_entropy([2; 16]), input);
            assert!(definition.is_ok(), "{name}");
        } else {
            let error = got.expect_err(name);
            assert_eq!(error.code, expect["code"].as_str().unwrap(), "{name}");
            assert_eq!(Some(error.at.as_str()), expect["at"].as_str(), "{name}");
            let offset = expect["offset"].as_u64().map(|o| u32::try_from(o).unwrap());
            assert_eq!(error.offset, offset, "{name}");
        }
    }
    assert!(shape >= 10 && checked > 100);
}

// ------------------------------------------------------------------------------------- render

#[test]
fn every_render_of_render_json() {
    let file = json(RENDER);
    let cases = file["cases"].as_array().unwrap();
    assert!(cases.len() > 70);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let prompt = expand(&c["prompt"]);
        let request = expand(&c["request"]);
        let expect = expand(&c["expect"]);
        let definition = PromptDefinition::new(
            PromptId::from_entropy([3; 16]),
            PromptInput {
                name: "p".into(),
                title: "T".into(),
                description: String::new(),
                body: prompt["body"].as_str().unwrap().to_owned(),
                variables: prompt["variables"].as_array().unwrap().clone(),
                audience: Vec::new(),
            },
        )
        .unwrap_or_else(|e| panic!("{name}: the prompt is refused: {e:?}"));
        let answers = validate_answers(
            definition.variables(),
            request.get("answers"),
            request.get("note").is_some(),
        );
        let rendered = answers.and_then(|a| {
            render(definition.body(), definition.variables(), &a).map_err(|_| AnswerRefusal {
                question: Some("@too_long".into()),
                rule: "rendered_too_long",
            })
        });
        match (
            &rendered,
            expect.get("text"),
            expect.get("rule"),
            expect.get("code"),
        ) {
            (Ok(text), Some(want), _, _) => assert_eq!(text, want.as_str().unwrap(), "{name}"),
            (Err(r), _, Some(rule), _) => {
                assert_eq!(r.rule, rule.as_str().unwrap(), "{name}");
                assert_eq!(
                    r.question_text(),
                    expect["question"].as_str().unwrap(),
                    "{name}"
                );
            }
            (Err(r), _, _, Some(code)) => {
                assert_eq!(r.rule, code.as_str().unwrap(), "{name}");
            }
            other => panic!("{name}: {other:?}"),
        }
    }
}

// ----------------------------------------------------------------- the catalog page's own blocks

#[test]
fn the_catalogs_vectors_pass_as_one_variable_prompts() {
    let vectors = block("vectors");
    let mut cases = 0;
    for v in vectors.as_array().unwrap() {
        let q = &v["question"];
        let variables = {
            assert_eq!(validate(&one_prompt(q)), Ok(()), "{q}");
            variables_of(std::slice::from_ref(q))
        };
        for case in v["cases"].as_array().unwrap() {
            cases += 1;
            let values = &case[0];
            let want = case[1].as_str();
            let answers = json!([{ "id": q["id"], "values": values }]);
            let got = validate_answers(&variables, Some(&answers), false);
            assert_eq!(got.err().map(|e| e.rule), want, "{q} {values}");
        }
    }
    assert_eq!(cases, 133);
}

#[test]
fn the_catalogs_whole_answers_pass_but_the_ones_a_prompt_cannot_have() {
    let mut skipped = 0;
    for c in block("whole-answers").as_array().unwrap() {
        let context = c["context"].as_object().unwrap();
        let extra = context.keys().any(|k| k != "answers" && k != "note");
        // a prompt has no free box, and a member besides `answers` and `note` is a 400 here
        if c["freeInput"] == true || extra {
            skipped += 1;
            continue;
        }
        let variables = variables_of(c["questions"].as_array().unwrap());
        let got = validate_answers(
            &variables,
            context.get("answers"),
            context.contains_key("note"),
        );
        let got = got.err().map(|e| (e.question_text().to_owned(), e.rule));
        let want = c["refusal"]
            .as_array()
            .map(|r| (r[0].as_str().unwrap().to_owned(), r[1].as_str().unwrap()));
        assert_eq!(got, want, "{c}");
    }
    assert_eq!(skipped, 2);
}

#[test]
fn the_catalogs_invalid_questions_are_bad_variables() {
    let cases = block("invalid-questions");
    assert!(cases.as_array().unwrap().len() > 30);
    for c in cases.as_array().unwrap() {
        let error =
            validate(&one_prompt(&c["question"])).expect_err(c["because"].as_str().unwrap());
        assert_eq!(error.code, "bad_variable", "{}", c["because"]);
    }
}

#[test]
fn the_catalogs_examples_are_questions_a_prompt_takes() {
    let questions = block("questions");
    let questions = questions.as_array().unwrap();
    let body: Vec<String> = questions
        .iter()
        .map(|q| format!("{{{{{}}}}}", q["id"].as_str().unwrap()))
        .collect();
    let input = PromptInput {
        name: "p".into(),
        title: "T".into(),
        description: String::new(),
        body: body.join(" "),
        variables: questions.clone(),
        audience: Vec::new(),
    };
    assert_eq!(validate(&input), Ok(()));
}

#[test]
fn the_choices_schema_has_the_members_the_rules_know() {
    let catalog = json(CATALOG_JSON);
    let members: BTreeSet<&str> =
        catalog["components"]["Choices"]["properties"]["questions"]["items"]["properties"]
            .as_object()
            .expect("the Choices schema")
            .keys()
            .map(String::as_str)
            .collect();
    let known: BTreeSet<&str> = CATALOG_MEMBERS.iter().copied().collect();
    // a version of the catalog that adds a member must be read by this module before it is accepted
    assert_eq!(members, known);
    let kinds: BTreeSet<&str> =
        catalog["components"]["Choices"]["properties"]["questions"]["items"]["properties"]["type"]
            ["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
    let ours: BTreeSet<&str> = KINDS.iter().copied().chain(["images"]).collect();
    assert_eq!(kinds, ours);
}

// ------------------------------------------------------------------------------ the page's numbers

#[test]
fn the_page_and_the_code_agree() {
    let mut rows = std::collections::BTreeMap::new();
    for line in PAGE_TEXT.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let name = cells
            .get(1)
            .map(|c| c.trim_matches('`'))
            .unwrap_or_default();
        let constant = !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_');
        if constant && let Ok(n) = cells[2].parse::<usize>() {
            rows.insert(name.to_owned(), n);
        }
    }
    let code: std::collections::BTreeMap<String, usize> = [
        ("MAX_PROMPTS", MAX_PROMPTS),
        ("MAX_NAME", MAX_NAME),
        ("MAX_TITLE", MAX_TITLE),
        ("MAX_DESCRIPTION", MAX_DESCRIPTION),
        ("MAX_BODY_BYTES", MAX_BODY_BYTES),
        ("MAX_VARIABLES", MAX_VARIABLES),
        ("MAX_AUDIENCE", MAX_AUDIENCE),
        ("MAX_AUDIENCE_VALUE", MAX_AUDIENCE_VALUE),
        ("MAX_RENDERED_BYTES", MAX_RENDERED_BYTES),
        ("MAX_REQUEST_BYTES", MAX_REQUEST_BYTES),
        ("PAGE", PAGE),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    assert_eq!(rows, code);
}

// ------------------------------------------------------------------------------------ the units

#[test]
fn decimals_are_exact() {
    // 0.1 on a step of 0.1 from 0.1: 0.3 is three steps, 0.35 is not
    let q = json!({"id": "t", "question": "T?", "type": "number", "minValue": "0.1", "maxValue": "1", "step": "0.1"});
    let variables = variables_of(std::slice::from_ref(&q));
    let ask = |v: &str| {
        validate_answers(
            &variables,
            Some(&json!([{"id": "t", "values": [v]}])),
            false,
        )
        .err()
        .map(|e| e.rule)
    };
    assert_eq!(ask("0.3"), None);
    assert_eq!(ask("0.35"), Some("off_step"));
    assert_eq!(decimal(Some(&json!("-0"))), None);
    assert_eq!(decimal(Some(&json!("-2.5"))), Some(-2_500_000));
    assert_eq!(
        decimal(Some(&json!("123456789012345.123456"))),
        Some(123_456_789_012_345_123_456)
    );
    assert_eq!(decimal(Some(&json!("1234567890123456"))), None);
}

#[test]
fn days_are_real_days() {
    assert!(real_day("2028-02-29"));
    assert!(!real_day("2027-02-29"));
    assert!(!real_day("0000-01-01"));
    assert!(real_day("9999-12-31"));
    assert!(!real_day("２０２６-12-15"));
}

#[test]
fn an_id_is_made_of_entropy_and_read_back() {
    let id = PromptId::from_entropy([7; 16]);
    assert_eq!(PromptId::parse(id.as_str()), Ok(id.clone()));
    assert_eq!(id.as_str().len(), 14);
    assert_eq!(PromptId::from_entropy([7; 16]), id);
    assert!(PromptId::parse("p-ABCDEFGHIJKL").is_err());
    assert!(PromptId::parse("p-abc").is_err());
    assert!(PromptId::parse("q-abcdefghijkl").is_err());
    // exactly 12 characters: one more or one fewer is no id
    assert!(PromptId::parse("p-abcdefghijkl").is_ok());
    assert!(PromptId::parse("p-abcdefghijklm").is_err());
    assert!(PromptId::parse("p-abcdefghijk").is_err());
}

#[test]
fn an_integer_member_is_a_number_whose_value_is_an_integer_in_range() {
    let n = |text: &str| -> Option<i64> { as_int(&serde_json::from_str::<Value>(text).unwrap()) };
    assert_eq!(n("5"), Some(5));
    assert_eq!(n("5.0"), Some(5));
    assert_eq!(n("5e0"), Some(5));
    assert_eq!(n("0.5e1"), Some(5));
    assert_eq!(n("-3.0"), Some(-3));
    assert_eq!(n("5.5"), None);
    assert_eq!(n("1e30"), None);
    assert_eq!(n("18446744073709551616"), None);
    assert_eq!(n("\"5\""), None);
    assert_eq!(n("true"), None);
    assert_eq!(n("null"), None);
    assert!(int_in(&json!(5.0), 2, 10));
    assert!(!int_in(&json!(11.0), 2, 10));
    assert!(!int_in(&json!(5.5), 2, 10));
}

#[test]
fn a_text_a_person_reads_has_no_control_character_bidi_control_or_separator() {
    for bad in [
        "a\u{0}b",
        "a\nb",
        "a\tb",
        "a\u{7f}b",
        "a\u{85}b",
        "a\u{202a}b",
        "a\u{202e}b",
        "a\u{2066}b",
        "a\u{2069}b",
        "a\u{2028}b",
        "a\u{2029}b",
    ] {
        assert!(is_unsafe_text(bad, &[]), "{bad:?}");
        assert!(!shown_in(Some(&json!(bad)), 1, 20), "{bad:?}");
    }
    // joiners, direction marks and spaces are text
    for fine in [
        "a\u{200d}b",
        "a\u{200e}b",
        "a\u{200f}b",
        "a\u{200b}b",
        "a\u{a0}b",
        "é😀",
    ] {
        assert!(!is_unsafe_text(fine, &[]), "{fine:?}");
    }
    assert!(!is_unsafe_text("a\tb\nc", &['\t', '\n']));
    assert!(is_unsafe_text("a\u{202e}b", &['\t', '\n']));
}

#[test]
fn the_audience_rule_has_no_administrator_exception() {
    let roles = |r: &[&str]| r.iter().map(|s| (*s).to_owned()).collect::<BTreeSet<_>>();
    let team = PromptAudience::from_values(&["team".into()]).unwrap();
    assert!(team.admits(&roles(&["user", "team"])));
    assert!(!team.admits(&roles(&["admin"])));
    assert!(!team.admits(&roles(&["Team"])));
    assert!(PromptAudience::Everyone.admits(&roles(&[])));
    assert!(!PromptAudience::Private.admits(&roles(&["team"])));
    assert_eq!(
        PromptAudience::from_values(&["b".into(), "a".into(), "b".into()])
            .unwrap()
            .values(),
        vec!["a".to_owned(), "b".to_owned()]
    );
}

proptest! {
    /// A text answer is written as it is, as many times as it is used, and the size is what was added up.
    #[test]
    fn render_never_panics_and_its_length_is_the_sum(text in "[a-zA-Z0-9 é😀]{1,40}", uses in 1usize..20, literal in "[a-z é😀]{0,30}") {
        let q = json!({"id": "t", "question": "T?", "type": "text"});
        let body = format!("{literal}{}", "{{t}}".repeat(uses));
        let definition = PromptDefinition::new(
            PromptId::from_entropy([4; 16]),
            PromptInput {
                name: "p".into(), title: "T".into(), description: String::new(),
                body: body.clone(), variables: vec![q], audience: Vec::new(),
            },
        );
        prop_assume!(definition.is_ok());
        let definition = definition.unwrap();
        let answers = validate_answers(
            definition.variables(),
            Some(&json!([{"id": "t", "values": [], "text": text}])),
            false,
        );
        prop_assert!(answers.is_ok());
        let out = render(definition.body(), definition.variables(), &answers.unwrap()).unwrap();
        prop_assert_eq!(out.len(), literal.len() + uses * text.len());
    }
}

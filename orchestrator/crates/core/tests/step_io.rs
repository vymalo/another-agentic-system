//! What a step carries (ADR 0030): `input` once with the start, `output` with the end, both cut,
//! redacted and kept within the job's budget; a bound that holds whatever an agent sends.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::ThreadState::Working;
use orch_core::*;
use proptest::prelude::*;
use serde_json::{Value, json};

fn report(id: &str, state: StepState) -> StepReport {
    StepReport {
        id: id.into(),
        parent: None,
        kind: StepKind::Tool,
        label: format!("run {id}"),
        state,
        icon: None,
        detail: None,
        input: None,
        output: None,
    }
}

fn with_input(mut r: StepReport, input: Value) -> StepReport {
    r.input = input.as_object().cloned();
    r
}

fn with_output(mut r: StepReport, text: &str) -> StepReport {
    r.output = Some(StepOutput {
        text: text.into(),
        ..StepOutput::default()
    });
    r
}

fn clean(r: &StepReport) -> StepReport {
    r.sanitize(StepSource::Agent).unwrap()
}

fn actor() -> Actor {
    Actor::agent(&AgentId::new("coder"), None)
}

/// Records `reports` in order against one job; the events they gave.
fn record(job: &mut Job, reports: &[StepReport]) -> Vec<AgentStepData> {
    let mut out = Vec::new();
    for r in reports {
        let (_, cmds) = record_step(Working, job, actor(), r, StepSource::Agent);
        for c in cmds {
            if let Command::Append(d) = c
                && let EventBody::AgentStep(s) = d.body
            {
                out.push(s);
            }
        }
    }
    out
}

// ---- the input ------------------------------------------------------------------------------

#[test]
fn an_input_that_fits_is_kept_as_it_is() {
    let input = json!({"query": "node 24", "limit": 3, "nested": {"a": [1, 2, null, true]}});
    let r = clean(&with_input(report("a", StepState::Running), input.clone()));
    assert_eq!(Value::Object(r.input.unwrap()), input);
}

#[test]
fn an_empty_input_is_no_input() {
    let r = clean(&with_input(report("a", StepState::Running), json!({})));
    assert_eq!(r.input, None);
}

#[test]
fn a_long_string_in_an_input_is_cut_at_512_characters() {
    let long = "é".repeat(STEP_INPUT_STRING_MAX_CHARS + 100);
    let r = clean(&with_input(
        report("a", StepState::Running),
        json!({"text": long, "short": "ok", "deep": {"list": [long]}}),
    ));
    let input = Value::Object(r.input.unwrap());
    let cut = input["text"].as_str().unwrap();
    assert_eq!(cut.chars().count(), STEP_INPUT_STRING_MAX_CHARS);
    assert!(cut.ends_with('\u{2026}'));
    assert_eq!(input["short"], "ok");
    assert_eq!(
        input["deep"]["list"][0].as_str().unwrap().chars().count(),
        STEP_INPUT_STRING_MAX_CHARS
    );
}

#[test]
fn an_input_still_over_4_kib_is_replaced_by_a_marker_with_its_size() {
    // 20 strings of 300 characters: no string is cut, the whole is 6 KiB
    let many: Vec<String> = (0..20).map(|n| format!("{n:0>300}")).collect();
    let input = json!({"files": many});
    let before = serde_json::to_string(&input).unwrap().len();
    assert!(before > STEP_INPUT_MAX_BYTES);
    let r = clean(&with_input(report("a", StepState::Running), input));
    assert_eq!(
        Value::Object(r.input.unwrap()),
        json!({"_cut": true, "bytes": before})
    );
}

#[test]
fn an_input_at_the_bound_is_kept() {
    // the serialized object is exactly STEP_INPUT_MAX_BYTES long
    let overhead = serde_json::to_string(&json!({"k": ""})).unwrap().len();
    let a = "a".repeat(STEP_INPUT_STRING_MAX_CHARS);
    let mut parts = vec![];
    let mut size = overhead;
    let mut n = 0;
    while size + STEP_INPUT_STRING_MAX_CHARS + 6 <= STEP_INPUT_MAX_BYTES {
        parts.push((format!("k{n:02}"), a.clone()));
        size += STEP_INPUT_STRING_MAX_CHARS + 6 + 4;
        n += 1;
    }
    let map: serde_json::Map<String, Value> = parts
        .into_iter()
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    let len = serde_json::to_string(&map).unwrap().len();
    assert!(len <= STEP_INPUT_MAX_BYTES, "{len}");
    let r = clean(&with_input(
        report("a", StepState::Running),
        Value::Object(map.clone()),
    ));
    assert_eq!(r.input, Some(map));
}

#[test]
fn control_characters_never_reach_the_log() {
    let r = clean(&with_input(
        report("a", StepState::Running),
        json!({"k\u{0}ey": "a\u{0}b\u{7}c\nd\te", "n": 1}),
    ));
    let input = Value::Object(r.input.unwrap());
    assert_eq!(input["key"], "abc\nd\te");
    let mut o = with_output(report("a", StepState::Completed), "x\u{0}y\r\nz\u{1b}[0m");
    o.output.as_mut().unwrap().error = true;
    assert_eq!(clean(&o).output.unwrap().text, "xy\nz[0m");
}

#[test]
fn an_input_nested_too_deep_is_flattened_not_followed() {
    let mut v = json!("leaf");
    for _ in 0..40 {
        v = json!({ "k": v });
    }
    let r = clean(&with_input(
        report("a", StepState::Running),
        json!({"deep": v}),
    ));
    let text = serde_json::to_string(&r.input.unwrap()).unwrap();
    assert!(text.contains('\u{2026}'));
    assert!(!text.contains("leaf"));
}

// ---- the output -----------------------------------------------------------------------------

#[test]
fn an_output_that_fits_is_kept_untouched() {
    let r = clean(&with_output(
        report("a", StepState::Completed),
        "two results\n  indented\tand a tab",
    ));
    assert_eq!(
        r.output,
        Some(StepOutput {
            text: "two results\n  indented\tand a tab".into(),
            ..StepOutput::default()
        })
    );
}

#[test]
fn an_output_at_the_bound_is_not_cut_and_one_byte_over_is() {
    let at = "a".repeat(STEP_OUTPUT_MAX_BYTES);
    let o = clean(&with_output(report("a", StepState::Completed), &at))
        .output
        .unwrap();
    assert_eq!(
        (o.text.len(), o.truncated, o.bytes),
        (STEP_OUTPUT_MAX_BYTES, false, None)
    );
    let over = "a".repeat(STEP_OUTPUT_MAX_BYTES + 1);
    let o = clean(&with_output(report("a", StepState::Completed), &over))
        .output
        .unwrap();
    assert!(o.truncated);
    assert_eq!(o.bytes, Some(STEP_OUTPUT_MAX_BYTES as u64 + 1));
    assert!(o.text.len() <= STEP_OUTPUT_MAX_BYTES);
}

#[test]
fn a_long_output_keeps_its_head_and_its_tail_and_says_what_is_missing() {
    let text = format!("HEAD{}TAIL: exit 1", "x".repeat(50_000));
    let o = clean(&with_output(report("a", StepState::Failed), &text))
        .output
        .unwrap();
    assert!(o.text.starts_with("HEAD"), "{}", &o.text[..20]);
    assert!(o.text.ends_with("TAIL: exit 1"), "errors are at the end");
    assert!(o.text.len() <= STEP_OUTPUT_MAX_BYTES);
    assert!(o.truncated);
    assert_eq!(o.bytes, Some(text.len() as u64));
    // the line between the two parts says how many bytes are not kept
    let marker = o
        .text
        .lines()
        .find(|l| l.contains("bytes not kept"))
        .unwrap();
    let said: usize = marker
        .split_whitespace()
        .find_map(|w| w.parse().ok())
        .unwrap();
    let kept = o.text.len() - marker.len() - 2;
    assert_eq!(said + kept, text.len(), "{marker}");
}

#[test]
fn a_cut_never_splits_a_character() {
    // 3-byte characters, so almost every byte offset is inside one
    for extra in 0..6 {
        let text = "\u{20ac}".repeat(STEP_OUTPUT_MAX_BYTES / 3 + 300 + extra);
        let o = clean(&with_output(report("a", StepState::Completed), &text))
            .output
            .unwrap();
        assert!(o.text.len() <= STEP_OUTPUT_MAX_BYTES, "{}", o.text.len());
        assert!(o.truncated);
    }
}

#[test]
fn an_agent_that_cut_its_output_says_so_and_the_size_it_names_is_kept() {
    let mut r = with_output(report("a", StepState::Completed), "the first 100 lines");
    let out = r.output.as_mut().unwrap();
    out.truncated = true;
    out.bytes = Some(1_000_000);
    let o = clean(&r).output.unwrap();
    assert_eq!((o.truncated, o.bytes), (true, Some(1_000_000)));
    // a size that is smaller than the text is nonsense and is not kept
    let out = r.output.as_mut().unwrap();
    out.bytes = Some(3);
    let o = clean(&r).output.unwrap();
    assert_eq!((o.truncated, o.bytes), (true, None));
    // an agent that names a size without saying it cut: not truncated
    let out = r.output.as_mut().unwrap();
    out.truncated = false;
    out.bytes = Some(1_000_000);
    let o = clean(&r).output.unwrap();
    assert_eq!((o.truncated, o.bytes), (false, None));
}

#[test]
fn an_empty_output_is_none_unless_it_is_an_error() {
    assert_eq!(
        clean(&with_output(report("a", StepState::Completed), "")).output,
        None
    );
    let mut r = with_output(report("a", StepState::Failed), "");
    r.output.as_mut().unwrap().error = true;
    assert!(clean(&r).output.unwrap().error);
}

// ---- where they are logged ------------------------------------------------------------------

#[test]
fn the_input_is_logged_with_the_start_and_the_output_with_the_end() {
    let mut job = Job::default();
    let events = record(
        &mut job,
        &[
            with_input(report("a", StepState::Running), json!({"q": 1})),
            // an update that says it again, and an output on an update: neither is logged
            with_output(
                with_input(report("a", StepState::Running), json!({"q": 1})),
                "early",
            ),
            with_output(report("a", StepState::Completed), "done"),
        ],
    );
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].input, json!({"q": 1}).as_object().cloned());
    assert_eq!((&events[1].input, &events[1].output), (&None, &None));
    assert_eq!(events[2].input, None);
    assert_eq!(events[2].output.as_ref().unwrap().text, "done");
}

#[test]
fn the_first_report_that_has_an_input_logs_it_even_when_it_is_not_the_start() {
    let mut job = Job::default();
    let events = record(
        &mut job,
        &[
            report("a", StepState::Running),
            with_input(report("a", StepState::Running), json!({"q": 1})),
            with_input(report("a", StepState::Running), json!({"q": 2})),
        ],
    );
    assert_eq!(events[0].input, None);
    assert_eq!(events[1].input, json!({"q": 1}).as_object().cloned());
    assert_eq!(events[2].input, None, "once");
}

#[test]
fn a_step_that_ends_in_one_report_carries_both() {
    let mut job = Job::default();
    let r = with_output(
        with_input(report("a", StepState::Failed), json!({"q": 1})),
        "boom",
    );
    let events = record(&mut job, &[r]);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].phase, StepPhase::End);
    assert!(events[0].input.is_some() && events[0].output.is_some());
}

#[test]
fn a_retry_of_a_step_logs_its_input_again() {
    let mut job = Job::default();
    let events = record(
        &mut job,
        &[
            with_input(report("a", StepState::Running), json!({"q": 1})),
            with_output(report("a", StepState::Failed), "no"),
            with_input(report("a", StepState::Running), json!({"q": 2})),
        ],
    );
    assert_eq!(events[2].phase, StepPhase::Start);
    assert_eq!(events[2].input, json!({"q": 2}).as_object().cloned());
}

#[test]
fn a_step_with_neither_serializes_as_it_did_before() {
    let mut job = Job::default();
    let events = record(&mut job, &[report("a", StepState::Running)]);
    let v = serde_json::to_value(&events[0]).unwrap();
    for member in ["input", "output", "ioDropped"] {
        assert!(v.get(member).is_none(), "{member}");
    }
    assert!(job.steps.io_bytes() == 0 && job.steps.open_count() == 1);
}

#[test]
fn the_wire_shape_of_a_step_with_input_and_output() {
    let mut job = Job::default();
    let mut end = with_output(report("a", StepState::Failed), "boom");
    end.output.as_mut().unwrap().error = true;
    let events = record(
        &mut job,
        &[
            with_input(report("a", StepState::Running), json!({"q": "x"})),
            end,
        ],
    );
    let start = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(start["input"], json!({"q": "x"}));
    let end = serde_json::to_value(&events[1]).unwrap();
    assert_eq!(end["output"], json!({"text": "boom", "error": true}));
    // and back
    let back: AgentStepData = serde_json::from_value(end).unwrap();
    assert_eq!(back, events[1]);
}

// ---- the job's budget -----------------------------------------------------------------------

#[test]
fn past_the_jobs_budget_input_and_output_are_dropped_and_the_step_says_so() {
    let mut job = Job::default();
    let big = "o".repeat(STEP_OUTPUT_MAX_BYTES);
    let per_step = STEP_OUTPUT_MAX_BYTES;
    let steps_that_fit = MAX_STEP_IO_BYTES_PER_JOB as usize / per_step;
    let mut dropped_at = None;
    // a thousand steps would be 8 MiB of output; the budget lets 2 MiB through
    for n in 0..(steps_that_fit + 40) {
        let id = format!("s{n}");
        let events = record(
            &mut job,
            &[with_output(report(&id, StepState::Completed), &big)],
        );
        if events[0].output.is_none() {
            dropped_at = Some(n);
            assert!(events[0].io_dropped, "the step says what happened");
            assert_eq!(
                events[0].state,
                StepState::Completed,
                "the step itself is kept"
            );
            break;
        }
        assert!(!events[0].io_dropped);
    }
    assert_eq!(dropped_at, Some(steps_that_fit));
    assert!(job.steps.io_bytes() <= MAX_STEP_IO_BYTES_PER_JOB);
    // later steps are still logged, without their members
    let events = record(
        &mut job,
        &[with_input(
            with_output(report("late", StepState::Completed), "x"),
            json!({"q": 1}),
        )],
    );
    assert_eq!(events.len(), 1);
    assert!(events[0].input.is_none() && events[0].output.is_none() && events[0].io_dropped);
}

#[test]
fn a_member_that_does_not_fit_does_not_stop_a_smaller_one() {
    let mut job = Job::default();
    // fill the budget to within a few bytes
    let big = "o".repeat(STEP_OUTPUT_MAX_BYTES);
    let fills = MAX_STEP_IO_BYTES_PER_JOB as usize / STEP_OUTPUT_MAX_BYTES - 1;
    for n in 0..fills {
        record(
            &mut job,
            &[with_output(
                report(&format!("f{n}"), StepState::Completed),
                &big,
            )],
        );
    }
    record(
        &mut job,
        &[with_output(
            report("filler", StepState::Completed),
            &"f".repeat(8000),
        )],
    );
    let left = MAX_STEP_IO_BYTES_PER_JOB - job.steps.io_bytes();
    assert!(left < STEP_OUTPUT_MAX_BYTES as u32);
    let events = record(
        &mut job,
        &[with_output(report("big", StepState::Completed), &big)],
    );
    assert!(events[0].output.is_none() && events[0].io_dropped);
    let events = record(
        &mut job,
        &[with_output(report("small", StepState::Completed), "tiny")],
    );
    assert!(events[0].output.is_some() && !events[0].io_dropped);
}

#[test]
fn a_new_job_has_a_new_budget() {
    let mut job = Job::default();
    let big = "o".repeat(STEP_OUTPUT_MAX_BYTES);
    record(
        &mut job,
        &[with_output(report("a", StepState::Completed), &big)],
    );
    assert!(job.steps.io_bytes() > 0);
    let next = job.next();
    assert_eq!(next.steps.io_bytes(), 0);
}

#[test]
fn the_budget_survives_the_ledger_being_stored() {
    let mut job = Job::default();
    record(
        &mut job,
        &[with_input(report("a", StepState::Running), json!({"q": 1}))],
    );
    let stored = serde_json::to_value(&job.steps).unwrap();
    assert!(stored["ioBytes"].as_u64().unwrap() > 0);
    let back: StepLedger = serde_json::from_value(stored).unwrap();
    assert_eq!(back, job.steps);
    // the input of the open step was logged: the end does not log one again
    let mut job2 = Job {
        steps: back,
        ..Job::default()
    };
    let events = record(
        &mut job2,
        &[with_input(
            report("a", StepState::Completed),
            json!({"q": 1}),
        )],
    );
    assert_eq!(events[0].input, None);
    // a ledger stored before ADR 0030 has neither member
    let old: StepLedger = serde_json::from_value(json!({"open": {}, "started": 3})).unwrap();
    assert_eq!(old.io_bytes(), 0);
}

// ---- the bound holds whatever an agent sends ------------------------------------------------

fn any_value(depth: u32) -> BoxedStrategy<Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| json!(n)),
        ".{0,2000}".prop_map(Value::String),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    prop_oneof![
        leaf,
        prop::collection::vec(any_value(depth - 1), 0..6).prop_map(Value::Array),
        prop::collection::btree_map(".{0,12}", any_value(depth - 1), 0..6)
            .prop_map(|m| Value::Object(m.into_iter().collect())),
    ]
    .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn no_input_or_output_the_core_keeps_exceeds_its_bound(
        input in prop::collection::btree_map(".{1,12}", any_value(3), 1..8),
        text in "(?s).{0,12000}",
        cut_by_agent in any::<bool>(),
        claimed in any::<Option<u64>>(),
    ) {
        let mut r = report("p", StepState::Failed);
        r.input = Some(input.into_iter().collect());
        r.output = Some(StepOutput { text: text.clone(), truncated: cut_by_agent, bytes: claimed, error: true });
        let c = r.sanitize(StepSource::Agent).unwrap();
        if let Some(input) = &c.input {
            let len = serde_json::to_string(input).unwrap().len();
            prop_assert!(len <= STEP_INPUT_MAX_BYTES, "input of {len} bytes");
        }
        if let Some(o) = &c.output {
            prop_assert!(o.text.len() <= STEP_OUTPUT_MAX_BYTES, "output of {} bytes", o.text.len());
            prop_assert!(!o.text.chars().any(|ch| ch.is_control() && ch != '\n' && ch != '\t'));
            if o.bytes.is_some() { prop_assert!(o.truncated); }
        }
    }

    #[test]
    fn a_job_never_logs_more_than_its_budget(
        sizes in prop::collection::vec(0usize..20_000, 1..80),
    ) {
        let mut job = Job::default();
        for (n, size) in sizes.into_iter().cycle().take(400).enumerate() {
            let mut each = with_output(report(&format!("p{n}"), StepState::Completed), &"o".repeat(size));
            each.input = json!({"q": "q".repeat(size.min(500))}).as_object().cloned();
            let _ = record(&mut job, &[each]);
        }
        prop_assert!(job.steps.io_bytes() <= MAX_STEP_IO_BYTES_PER_JOB);
    }
}

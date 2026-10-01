//! Golden AG-UI streams: `docs/api/examples/agui/<name>.agui.json`, produced purely from the
//! existing `docs/api/examples/<name>.events.json` by the projection.
//!
//! The events goldens are what the real orchestrator emitted for each scripted agent behaviour
//! (`orch-e2e`, `golden.rs`); these are what a **viewer** (a connect stream) is shown for the
//! same log. Each file is an array of frames, `{"id"?: <seq>, "event": <AG-UI event>}`: `id` is
//! the SSE `id:` (present only on resume points). `tools/agui-conformance` writes the frames as
//! SSE and feeds them through the reference consumer of `@ag-ui/client`.
//!
//! `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` rewrites the files;
//! without it any difference fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::path::PathBuf;

use orch_agui_projection::{Audience, Frame, Projector, ThreadMeta};
use orch_agui_proto::testkit::assert_conforms;
use orch_core::{AgentId, AgentTarget, CheckSource, Event, EventBody, GatePolicy, Timestamp};
use serde_json::{Value, json};
use support::{lines, verify};

const THREAD: &str = "00000000-0000-7000-8000-000000000001";
const SCENARIOS: [&str; 14] = [
    "echo",
    "ask",
    "cancel",
    "fail",
    "talk",
    "release",
    "a2ui",
    "verify-green",
    "verify-red",
    "verify-verifier-green",
    "verify-verifier-red",
    "ci",
    "followup",
    "followup-after-cancel",
];

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples")
}

/// The thread record the scenario ran against: the events goldens carry no title or target.
fn meta_of(name: &str, events: &[Event]) -> ThreadMeta {
    let (agent, release) = match name {
        "release" => ("coder", Some("staging".to_owned())),
        _ => ("plain", None),
    };
    let title = match &events[0].body {
        EventBody::UserMessage(m) => m.text.lines().next().unwrap_or("").to_owned(),
        _ => String::new(),
    };
    // The verification scenarios ran under the gate that requires the agent's own checks, or
    // the verifier `reviewer`.
    let gate = match name {
        "verify-green" | "verify-red" => GatePolicy::requiring([CheckSource::AgentChecks]),
        "verify-verifier-green" | "verify-verifier-red" => {
            let mut gate = GatePolicy::requiring([CheckSource::Verifier]);
            gate.verifier = Some(AgentId::new("reviewer"));
            gate
        }
        // The CI scenario ran under a gate that requires CI on the pushed commit.
        "ci" => GatePolicy::requiring([CheckSource::Ci]),
        _ => GatePolicy::default(),
    };
    ThreadMeta {
        thread_id: THREAD.parse().unwrap(),
        title,
        target: AgentTarget {
            agent_id: AgentId::new(agent),
            release,
        },
        gate,
    }
}

/// The events golden with its placeholders made real: a thread id, timestamps, message ids.
fn load_events(name: &str) -> Vec<Event> {
    let path = examples_dir().join(format!("{name}.events.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut raw: Value = serde_json::from_str(&text).unwrap();
    for e in raw.as_array_mut().unwrap() {
        let seq = e["seq"].as_i64().unwrap();
        e["threadId"] = json!(THREAD);
        e["at"] = json!(
            Timestamp::from_second(1_800_000_000 + seq)
                .unwrap()
                .to_string()
        );
        if e["kind"] == "agent_message" {
            e["data"]["messageId"] = json!(format!("msg-{seq}"));
        }
    }
    serde_json::from_value(raw).unwrap()
}

fn project(name: &str) -> Vec<Frame> {
    let events = load_events(name);
    let mut projector = Projector::new(meta_of(name, &events));
    events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect()
}

/// `{"id"?, "event"}` per frame, with the thread id back to its placeholder.
fn render(frames: &[Frame]) -> String {
    let mut value = Value::Array(
        frames
            .iter()
            .map(|f| {
                let mut frame = serde_json::Map::new();
                if let Some(id) = f.resume_id {
                    frame.insert("id".to_owned(), json!(id));
                }
                frame.insert("event".to_owned(), serde_json::to_value(&f.event).unwrap());
                Value::Object(frame)
            })
            .collect(),
    );
    fn placeholder(v: &mut Value) {
        match v {
            Value::String(s) if s == THREAD => *s = "<thread-id>".to_owned(),
            Value::Array(items) => items.iter_mut().for_each(placeholder),
            Value::Object(map) => map.values_mut().for_each(placeholder),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    placeholder(&mut value);
    let mut text = serde_json::to_string_pretty(&value).unwrap();
    text.push('\n');
    text
}

#[test]
fn every_golden_stream_is_well_formed_and_conforms_to_the_schema() {
    for name in SCENARIOS {
        let frames = project(name);
        for frame in &frames {
            assert_conforms(&frame.event);
        }
        let checker = verify::check(&frames).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            !checker.run_open(),
            "{name}: the run is closed at the end of the log"
        );
    }
}

#[test]
fn agui_goldens_match_docs_api_examples() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let dir = examples_dir().join("agui");
    let mut stale = Vec::new();
    for name in SCENARIOS {
        let text = render(&project(name));
        let path = dir.join(format!("{name}.agui.json"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
            continue;
        }
        match std::fs::read_to_string(&path) {
            Ok(want) if want == text => {}
            Ok(want) => stale.push(format!(
                "{}: differs\n--- want\n{want}--- got\n{text}",
                path.display()
            )),
            Err(e) => stale.push(format!("{}: {e}", path.display())),
        }
    }
    assert!(
        stale.is_empty(),
        "AG-UI goldens are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` and review the diff:\n{}",
        stale.join("\n")
    );
}

/// The worked example of `docs/api/agui.md`, frame by frame.
#[test]
fn ask_is_the_worked_example_of_the_binding() {
    let got = lines(&project("ask"));
    let want = [
        "RUN_STARTED run-1",
        "STATE_SNAPSHOT queued",
        "TEXT_MESSAGE_START evt-1 user",
        "TEXT_MESSAGE_CONTENT evt-1 \"ask about branches\"",
        "TEXT_MESSAGE_END evt-1  id:1",
        "SUBAGENT_STARTED sub-2 plain",
        "ACTIVITY_SNAPSHOT evt-2 vymalo.status {\"status\":\"working\"} @sub-2",
        "STATE_SNAPSHOT working  id:2",
        // The question is the agent's words: an assistant message, and the status says no more.
        "TEXT_MESSAGE_START st-3 assistant @sub-2",
        "TEXT_MESSAGE_CONTENT st-3 \"Which branch?\"",
        "TEXT_MESSAGE_END st-3",
        "ACTIVITY_SNAPSHOT evt-3 vymalo.status {\"status\":\"input_required\"} @sub-2",
        "SUBAGENT_FINISHED sub-2 suspended[int-3]  id:3",
        "STATE_SNAPSHOT blocked",
        "RUN_FINISHED run-1 interrupt[int-3:input_required @sub-2]  id:4",
        "RUN_STARTED run-5",
        "STATE_SNAPSHOT queued",
        "TEXT_MESSAGE_START evt-5 user",
        "TEXT_MESSAGE_CONTENT evt-5 \"main\"",
        "TEXT_MESSAGE_END evt-5  id:5",
        "SUBAGENT_STARTED sub-2 plain",
        "ACTIVITY_SNAPSHOT evt-6 vymalo.status {\"status\":\"working\"} @sub-2",
        "STATE_SNAPSHOT working  id:6",
        "ACTIVITY_SNAPSHOT evt-7 vymalo.artifact {\"kind\":\"file\",\"name\":\"result\",\"text\":\"answered: main\",\"uri\":\"https://github.com/acme/demo/pull/1\"} @sub-2  id:7",
        "ACTIVITY_SNAPSHOT evt-8 vymalo.status {\"status\":\"completed\"} @sub-2",
        "SUBAGENT_FINISHED sub-2 success  id:8",
        "STATE_SNAPSHOT done",
        "RUN_FINISHED run-5 success  id:9",
    ];
    assert_eq!(got, want);
}

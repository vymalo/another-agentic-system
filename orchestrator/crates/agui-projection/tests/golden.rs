//! Golden AG-UI streams: `docs/api/examples/agui/<name>.agui.json`, produced purely from the
//! existing `docs/api/examples/<name>.events.json` by the projection.
//!
//! The events goldens are what the real orchestrator emitted for each scripted agent behaviour
//! (`orch-e2e`, `golden.rs`); these are what a **viewer** (a connect stream) is shown for the
//! same log. Each file is an array of frames, `{"id"?: <seq>, "event": <AG-UI event>}`: `id` is
//! the SSE `id:` (present only on resume points). `tools/agui-conformance` writes the frames as
//! SSE and feeds them through the reference consumer of `@ag-ui/client`.
//!
//! A `<name>.feed.json` is the same with live text in it (ADR 0027): an array of `{"event": <a log
//! event, as in the events goldens>}` and `{"live": {agent, messageId, offset, text, end}}` in the
//! order a connection hears them, which no real run can pin down (the pieces and the log travel on
//! different channels), so it is written by hand. Its stream is the projection with the live
//! overlay on top.
//!
//! `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test golden` rewrites the files;
//! without it any difference fails the test.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, LiveOverlay, Projector};
use orch_agui_proto::testkit::assert_conforms;
use orch_core::{AgentId, Event, LiveChunk, LiveEnd, LiveText, Timestamp};
use serde_json::{Value, json};
use support::goldens::{PARENT, SCENARIOS, THREAD, examples_dir, load_events, meta_of};
use support::{lines, verify};

/// The golden streams made of a log and live text.
const FEEDS: [&str; 2] = ["stream", "reasoning-live"];

fn project(name: &str) -> Vec<Frame> {
    let events = load_events(name);
    let mut projector = Projector::new(meta_of(name, &events));
    events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect()
}

/// What a connection hears, in order.
enum Heard {
    Log(Event),
    Live(LiveText),
}

/// A feed golden with its placeholders made real: a thread id and the timestamps.
fn load_feed(name: &str) -> Vec<Heard> {
    let path = examples_dir().join(format!("{name}.feed.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let raw: Vec<Value> = serde_json::from_str(&text).unwrap();
    raw.into_iter()
        .map(|mut item| {
            if let Some(live) = item.get("live") {
                let end = match live["end"].as_str().unwrap() {
                    "open" => LiveEnd::Open,
                    "last" => LiveEnd::Last,
                    "abandoned" => LiveEnd::Abandoned,
                    other => panic!("{name}: unknown end {other}"),
                };
                Heard::Live(LiveText {
                    thread: THREAD.parse().unwrap(),
                    agent: AgentId::new(live["agent"].as_str().unwrap()),
                    chunk: LiveChunk {
                        message_id: live["messageId"].as_str().unwrap().to_owned(),
                        offset: live["offset"].as_u64().unwrap(),
                        text: live["text"].as_str().unwrap().to_owned(),
                        end,
                        kind: match live.get("kind").and_then(Value::as_str) {
                            Some("reasoning") => orch_core::LiveKind::Reasoning,
                            None => orch_core::LiveKind::Reply,
                            Some(other) => panic!("{name}: unknown kind {other}"),
                        },
                    },
                })
            } else {
                let e = &mut item["event"];
                let seq = e["seq"].as_i64().unwrap();
                e["threadId"] = json!(THREAD);
                e["at"] = json!(
                    Timestamp::from_second(1_800_000_000 + seq)
                        .unwrap()
                        .to_string()
                );
                Heard::Log(serde_json::from_value(e.clone()).unwrap())
            }
        })
        .collect()
}

/// What a viewer is shown for a feed: the projection of its log events, with the live overlay.
fn project_feed(name: &str) -> Vec<Frame> {
    let heard = load_feed(name);
    let events: Vec<Event> = heard
        .iter()
        .filter_map(|h| match h {
            Heard::Log(e) => Some(e.clone()),
            Heard::Live(_) => None,
        })
        .collect();
    let mut projector = Projector::new(meta_of(name, &events));
    let mut overlay = LiveOverlay::new();
    let mut frames = Vec::new();
    for h in &heard {
        match h {
            Heard::Log(e) => {
                let said = projector.apply(e, Audience::Viewer);
                frames.extend(overlay.logged(&projector, said));
            }
            Heard::Live(piece) => frames.extend(overlay.live(&projector, piece)),
        }
    }
    frames
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
            // also inside a longer text: the `href` of a kept file names its thread
            Value::String(s) if s.contains(THREAD) => *s = s.replace(THREAD, "<thread-id>"),
            Value::String(s) if s.contains(PARENT) => *s = s.replace(PARENT, "<parent-thread-id>"),
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
    let streams = SCENARIOS
        .iter()
        .map(|name| (*name, project(name)))
        .chain(FEEDS.iter().map(|name| (*name, project_feed(name))));
    for (name, frames) in streams {
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
    let streams = SCENARIOS
        .iter()
        .map(|name| (*name, project(name)))
        .chain(FEEDS.iter().map(|name| (*name, project_feed(name))));
    for (name, frames) in streams {
        let text = render(&frames);
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

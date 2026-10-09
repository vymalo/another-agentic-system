//! The golden event logs of `docs/api/examples/<name>.events.json`, as the projection tests read
//! them: the thread they ran against, its metadata, and the events with their placeholders made
//! real.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::path::PathBuf;

use orch_agui_projection::ThreadMeta;
use orch_core::{AgentId, AgentTarget, CheckSource, Event, EventBody, GatePolicy, Timestamp};
use serde_json::{Value, json};

pub const THREAD: &str = "00000000-0000-7000-8000-000000000001";
/// The thread a fork scenario was cut from.
pub const PARENT: &str = "00000000-0000-7000-8000-000000000002";
pub const SCENARIOS: [&str; 32] = [
    "echo",
    "file",
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
    "catalog",
    "steps",
    "steps-ask",
    "working",
    "reasoning",
    "turn-output",
    "title",
    "description",
    "fork",
    "fork-blocked",
    "tools-attach",
    "tools-relay",
    "steer",
    "stop-and-send",
    "mentions",
    "ask-agent",
    "usage",
];

pub fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../docs/api/examples")
}

/// The thread record the scenario ran against: the events goldens carry no title or target.
pub fn meta_of(name: &str, events: &[Event]) -> ThreadMeta {
    let (agent, release) = match name {
        "release" => ("coder", Some("staging".to_owned())),
        _ => ("plain", None),
    };
    // The thread's title is its first message (a catalog event may come before it).
    let title = events
        .iter()
        .find_map(|e| match &e.body {
            EventBody::UserMessage(m) => Some(m.text.lines().next().unwrap_or("").to_owned()),
            _ => None,
        })
        .unwrap_or_default();
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
        description: None,
        target: AgentTarget {
            agent_id: AgentId::new(agent),
            release,
        },
        gate,
    }
}

/// The events golden with its placeholders made real: a thread id, timestamps, message ids.
pub fn load_events(name: &str) -> Vec<Event> {
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
        if e["kind"] == "agent_reasoning" {
            e["data"]["messageId"] = json!(format!("think-{seq}"));
        }
        if e["kind"] == "thread_forked" {
            e["data"]["from"]["threadId"] = json!(PARENT);
        }
    }
    serde_json::from_value(raw).unwrap()
}

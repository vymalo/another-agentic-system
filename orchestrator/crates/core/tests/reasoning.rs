//! What the agent's model thought (`agent_reasoning`, ADR 0044): `AgentUpdate::Reasoning` is logged as it came,
//! bounded, and is **not the agent's words**: it is no message, no answer, no summary, no title and no part of the
//! conversation a fork continues. The core only records it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::*;

use ThreadState::{Queued, Working};

fn agent() -> AgentId {
    AgentId::new("coder")
}

fn reasoning(id: &str, text: &str, truncated: bool) -> Input {
    Input::Agent {
        agent: agent(),
        revision: Some("r1".into()),
        update: AgentUpdate::Reasoning {
            message_id: id.into(),
            text: text.into(),
            truncated,
        },
    }
}

fn message(id: &str, text: &str) -> Input {
    Input::Agent {
        agent: agent(),
        revision: None,
        update: AgentUpdate::Message {
            message_id: id.into(),
            text: text.into(),
            is_final: true,
            purpose: None,
        },
    }
}

fn bodies(cmds: &[Command]) -> Vec<&EventBody> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(d) => Some(&d.body),
            _ => None,
        })
        .collect()
}

#[test]
fn a_reasoning_is_logged_as_an_event_of_the_agent_and_moves_nothing_else() {
    for state in [Queued, Working] {
        let before = Snapshot::new(state);
        let (after, cmds) =
            transition(&before, &reasoning("R", "The user wants it.", false)).unwrap();
        assert_eq!(after.state, state, "no state moves");
        assert_eq!(after.job, before.job, "the job's ledger is untouched");
        let [Command::Append(draft)] = cmds.as_slice() else {
            panic!("exactly one event: {cmds:?}")
        };
        assert_eq!(draft.actor, Actor::agent(&agent(), Some("r1".into())));
        assert_eq!(
            draft.body,
            EventBody::AgentReasoning(AgentReasoningData {
                message_id: "R".into(),
                text: "The user wants it.".into(),
                truncated: false,
            })
        );
        assert_eq!(draft.body.kind(), EventKind::AgentReasoning);
        assert_eq!(draft.body.kind().as_str(), "agent_reasoning");
    }
}

#[test]
fn a_reasoning_is_not_the_agents_words_so_it_is_no_summary_and_no_last_message() {
    // under a gate that asks a verifier, the agent's last words are what the verifier is shown of its work
    let mut snap = Snapshot::new(Working);
    snap.job.gate = GatePolicy::requiring([CheckSource::Verifier]);
    let (after, _) = transition(&snap, &reasoning("R", "secret chain of thought", false)).unwrap();
    // what the verifier is shown of the agent's work is its words: none of these
    assert_eq!(after.job.summary, snap.job.summary);
    assert!(!format!("{:?}", after.job).contains("secret chain of thought"));
    // and the words that follow are the words
    let (after, cmds) = transition(&after, &message("S", "Done.")).unwrap();
    assert_eq!(after.job.summary.as_deref(), Some("Done."));
    assert_eq!(bodies(&cmds).len(), 1);
}

#[test]
fn the_text_is_bounded_again_at_the_door_and_says_it_was_cut() {
    let long = "é".repeat(MAX_REASONING_BYTES);
    let (_, cmds) = transition(&Snapshot::new(Working), &reasoning("R", &long, false)).unwrap();
    let [EventBody::AgentReasoning(d)] = bodies(&cmds).as_slice() else {
        panic!("{cmds:?}")
    };
    assert_eq!(d.text.len(), MAX_REASONING_BYTES);
    assert!(d.truncated, "cut at the bound, and says so");
    // an adapter that said it was cut keeps saying it
    let (_, cmds) = transition(&Snapshot::new(Working), &reasoning("R", "short", true)).unwrap();
    let [EventBody::AgentReasoning(d)] = bodies(&cmds).as_slice() else {
        panic!("{cmds:?}")
    };
    assert!(d.truncated);
}

#[test]
fn a_blank_reasoning_logs_nothing_and_control_characters_do_not_reach_the_log() {
    let (_, cmds) = transition(&Snapshot::new(Working), &reasoning("R", " \n\t ", false)).unwrap();
    assert!(cmds.is_empty(), "{cmds:?}");
    let (_, cmds) = transition(&Snapshot::new(Working), &reasoning("R", "a\u{0}b", false)).unwrap();
    let [EventBody::AgentReasoning(d)] = bodies(&cmds).as_slice() else {
        panic!("{cmds:?}")
    };
    assert_eq!(d.text, "ab");
}

#[test]
fn the_wire_shape_round_trips_through_the_log() {
    let body = EventBody::AgentReasoning(AgentReasoningData {
        message_id: "R".into(),
        text: "t".into(),
        truncated: true,
    });
    let value = body.data_value();
    assert_eq!(
        value,
        serde_json::json!({"messageId": "R", "text": "t", "truncated": true})
    );
    assert_eq!(
        EventBody::from_parts(EventKind::AgentReasoning, value).unwrap(),
        body
    );
}

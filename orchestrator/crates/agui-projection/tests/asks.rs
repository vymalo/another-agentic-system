//! Asked agents (ADR 0026, `docs/api/agui.md`, "Asked agents as subagents"), row by row, on
//! hand-written logs: an ask is a subagent named after the asked agent, `sub-ask-<n>`, nested under
//! the subagent that asked, with a `vymalo.ask` activity that says what was asked and how it
//! ended; the steps of the asked agent are attributed to it; what is open closes with the
//! invocation that holds it.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Connect, Follow, Frame, Projector};
use orch_core::{
    Actor, AgentId, AgentStatus, AgentStatusData, AgentStepData, AskArtifact, AskFinishedData,
    AskOutcome, AskStartedData, Caller, Event, EventBody, JobStartedData, StepKind, StepPhase,
    StepState, ThreadState, ThreadStateData, Timestamp, UserId, UserMessageData,
};
use serde_json::{Value, json};
use support::log::{meta, thread_id};
use support::{lines, verify};

fn ev(seq: i64, actor: Actor, body: EventBody) -> Event {
    Event {
        seq,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_000 + seq).unwrap(),
        actor,
        body,
    }
}

fn agent(id: &str) -> Actor {
    Actor::agent(&AgentId::new(id), None)
}

fn plain() -> Actor {
    agent("plain")
}

fn user(seq: i64, text: &str) -> Event {
    ev(
        seq,
        Actor::user(&UserId::new("alice@example.com")),
        EventBody::UserMessage(UserMessageData::new(text)),
    )
}

fn status(seq: i64, status: AgentStatus, detail: Option<&str>) -> Event {
    ev(
        seq,
        plain(),
        EventBody::AgentStatus(AgentStatusData {
            status,
            detail: detail.map(str::to_owned),
        }),
    )
}

fn thread(seq: i64, state: ThreadState) -> Event {
    ev(
        seq,
        Actor::system(),
        EventBody::ThreadState(ThreadStateData { state }),
    )
}

/// `ask_started`: `by` asks `to` (`n`th ask), saying `text`.
fn asked(seq: i64, n: u32, by: Caller, to: &str, text: &str, parent_step: Option<&str>) -> Event {
    let asker = match by {
        Caller::Main => plain(),
        Caller::Ask(_) => agent("coder"),
    };
    ev(
        seq,
        asker,
        EventBody::AskStarted(AskStartedData {
            ask: n,
            agent: AgentId::new(to),
            by,
            depth: if by.is_main() { 1 } else { 2 },
            text: text.to_owned(),
            step_id: format!("ask-{n}"),
            parent_step_id: parent_step.map(str::to_owned),
        }),
    )
}

fn data(n: u32, state: AskOutcome) -> AskFinishedData {
    AskFinishedData {
        ask: n,
        state,
        text: None,
        question: None,
        artifacts: Vec::new(),
        error: None,
    }
}

/// `ask_finished` by the asked agent `who`.
fn answered(seq: i64, who: &str, d: AskFinishedData) -> Event {
    ev(seq, agent(who), EventBody::AskFinished(d))
}

/// The orchestrator's own end of an ask (a cancel, a deadline, a delivery failure).
fn ended(seq: i64, d: AskFinishedData) -> Event {
    ev(seq, Actor::system(), EventBody::AskFinished(d))
}

fn step(
    seq: i64,
    id: &str,
    path: &[&str],
    kind: StepKind,
    state: StepState,
    phase: StepPhase,
) -> Event {
    ev(
        seq,
        agent("coder"),
        EventBody::AgentStep(AgentStepData {
            id: id.to_owned(),
            path: path.iter().map(|p| (*p).to_owned()).collect(),
            kind,
            label: format!("label {id}"),
            state,
            phase,
            icon: None,
            detail: None,
            input: None,
            output: None,
            io_dropped: false,
        }),
    )
}

/// Projects `events`, checking the stream, one entry per event.
fn project(events: &[Event]) -> Vec<Vec<Frame>> {
    let mut projector = Projector::new(meta());
    let frames: Vec<Vec<Frame>> = events
        .iter()
        .map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    verify::check(&support::flatten(&frames)).unwrap();
    frames
}

fn all_lines(events: &[Event]) -> Vec<String> {
    lines(&support::flatten(&project(events)))
}

/// The lines after the first one that starts with `marker`, the marker's own included.
fn from(got: &[String], marker: &str) -> Vec<String> {
    got.iter()
        .skip_while(|l| !l.starts_with(marker))
        .cloned()
        .collect()
}

/// The activity `id` as its last snapshot said it.
fn activity(frames: &[Frame], id: &str) -> Value {
    frames
        .iter()
        .rev()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.message_id.as_str() == id => {
                Some(serde_json::to_value(e).unwrap())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no activity {id}"))
}

/// The agent works (seq 2, its invocation `sub-2`) and asks `coder` (seq 3).
fn asking() -> Vec<Event> {
    vec![
        user(1, "go @coder"),
        status(2, AgentStatus::Working, None),
        asked(3, 1, Caller::Main, "coder", "find the data", None),
    ]
}

const STARTED_AT: &str = "2027-01-15T08:00:03Z";

#[test]
fn an_ask_is_a_subagent_of_the_asker_and_a_running_activity_that_says_what_was_asked() {
    let got = all_lines(&asking());
    assert_eq!(
        from(&got, "SUBAGENT_STARTED sub-ask-1"),
        [
            "SUBAGENT_STARTED sub-ask-1 coder in sub-2".to_owned(),
            format!(
                "ACTIVITY_SNAPSHOT ask-1 vymalo.ask {{\"agent\":\"coder\",\"ask\":1,\"by\":\"main\",\"depth\":1,\"startedAt\":\"{STARTED_AT}\",\"state\":\"running\",\"stepId\":\"ask-1\",\"text\":\"find the data\"}} @sub-2  id:3"
            ),
        ]
    );
    // attributed: the subagent is the asked agent's, the activity the asker's words
    let frames = support::flatten(&project(&asking()));
    let started = frames
        .iter()
        .find_map(|f| match &f.event {
            orch_agui_proto::Event::SubagentStarted(e)
                if e.subagent_run_id.as_str() == "sub-ask-1" =>
            {
                Some(serde_json::to_value(e).unwrap())
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(started["metadata"]["vymalo.actor"]["name"], "coder");
    assert_eq!(started["parentSubagentRunId"], "sub-2");
    let snapshot = activity(&frames, "ask-1");
    assert_eq!(snapshot["replace"], true);
    assert_eq!(snapshot["metadata"]["vymalo.actor"]["name"], "plain");
}

#[test]
fn a_call_that_names_its_step_nests_the_ask_under_the_sub_agent_step_while_it_is_open() {
    let mut events = vec![
        user(1, "go @coder"),
        status(2, AgentStatus::Working, None),
        step(
            3,
            "t/plan",
            &[],
            StepKind::Subagent,
            StepState::Running,
            StepPhase::Start,
        ),
        asked(4, 1, Caller::Main, "coder", "q", Some("t/plan")),
    ];
    let got = all_lines(&events);
    assert!(
        got.contains(&"SUBAGENT_STARTED sub-ask-1 coder in sub-step-3".to_owned()),
        "{got:#?}"
    );
    // a step that is not a sub-agent step, or is not open, is the invocation
    events[3] = asked(4, 1, Caller::Main, "coder", "q", Some("t/not-open"));
    let got = all_lines(&events);
    assert!(got.contains(&"SUBAGENT_STARTED sub-ask-1 coder in sub-2".to_owned()));
}

#[test]
fn an_answer_is_the_activity_said_again_and_the_end_of_the_subagent() {
    let mut events = asking();
    events.push(answered(
        4,
        "coder",
        AskFinishedData {
            text: Some("Pictures".to_owned()),
            artifacts: vec![AskArtifact {
                name: "pitch.png".to_owned(),
                uri: None,
                mime_type: Some("image/png".to_owned()),
            }],
            ..data(1, AskOutcome::Completed)
        },
    ));
    let frames = project(&events);
    let got = lines(&support::flatten(&frames));
    assert_eq!(
        from(
            &got,
            "ACTIVITY_SNAPSHOT ask-1 vymalo.ask {\"agent\":\"coder\",\"answer\":\"Pictures\""
        )
        .len(),
        2,
        "{got:#?}"
    );
    assert_eq!(
        got.last().unwrap(),
        "SUBAGENT_FINISHED sub-ask-1 success result={\"state\":\"completed\"}  id:4"
    );
    let content = activity(&support::flatten(&frames), "ask-1")["content"].clone();
    assert_eq!(content["state"], "completed");
    assert_eq!(content["answer"], "Pictures");
    assert_eq!(content["text"], "find the data", "what was asked stays");
    assert_eq!(content["artifacts"][0]["name"], "pitch.png");
    assert_eq!(content["artifacts"][0]["mimeType"], "image/png");
    assert_eq!(content["startedAt"], STARTED_AT);
    assert_eq!(content["at"], "2027-01-15T08:00:04Z");
    // the asker's invocation is still open: the ask did not end the thread's work
    assert!(!got.iter().any(|l| l.starts_with("RUN_FINISHED")));
}

#[test]
fn a_question_back_a_cancel_are_finished_and_a_failure_a_refusal_and_a_deadline_are_errors() {
    for (state, text, finished) in [
        (AskOutcome::InputRequired, None, true),
        (AskOutcome::AuthRequired, None, true),
        (AskOutcome::Canceled, None, true),
        (AskOutcome::Failed, Some("it broke"), false),
        (AskOutcome::Rejected, None, false),
        (AskOutcome::TimedOut, None, false),
    ] {
        let mut events = asking();
        events.push(ended(
            4,
            AskFinishedData {
                error: text.map(str::to_owned),
                ..data(1, state)
            },
        ));
        let got = all_lines(&events);
        let last = got.last().unwrap();
        if finished {
            assert_eq!(
                last,
                &format!(
                    "SUBAGENT_FINISHED sub-ask-1 success result={{\"state\":\"{}\"}}  id:4",
                    state.as_str()
                ),
                "{state:?}"
            );
        } else {
            let (code, message) = match state {
                AskOutcome::TimedOut => ("ask_timed_out", "the asked agent did not answer in time"),
                AskOutcome::Failed => ("ask_failed", "it broke"),
                _ => ("ask_failed", "coder failed"),
            };
            assert_eq!(
                last,
                &format!("SUBAGENT_ERROR sub-ask-1 {code} {message:?}  id:4"),
                "{state:?}"
            );
        }
        // the activity says the state in every case
        let frames = support::flatten(&project(&events));
        assert_eq!(
            activity(&frames, "ask-1")["content"]["state"],
            state.as_str()
        );
    }
}

#[test]
fn an_ask_of_an_asked_agent_is_nested_under_its_ask_and_ends_before_it() {
    let mut events = asking();
    events.push(asked(4, 2, Caller::Ask(1), "reviewer", "check it", None));
    let got = all_lines(&events);
    assert_eq!(
        from(&got, "SUBAGENT_STARTED sub-ask-2")[..2],
        [
            "SUBAGENT_STARTED sub-ask-2 reviewer in sub-ask-1".to_owned(),
            "ACTIVITY_SNAPSHOT ask-2 vymalo.ask {\"agent\":\"reviewer\",\"ask\":2,\"by\":\"ask:1\",\"depth\":2,\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"running\",\"stepId\":\"ask-2\",\"text\":\"check it\"} @sub-ask-1  id:4".to_owned(),
        ]
    );
    // the core ends the child with its parent, and logs the parent first: the child's subagent
    // ends before the parent's (nesting stays whole), and the child's own end, when the log gets
    // to it, says its activity and no subagent
    events.push(answered(5, "coder", data(1, AskOutcome::Completed)));
    events.push(ended(6, {
        let mut d = data(2, AskOutcome::Canceled);
        d.error = Some("the asking task ended".to_owned());
        d
    }));
    let frames = project(&events);
    let got = lines(&support::flatten(&frames));
    let child = got
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-ask-2"))
        .expect("the child's subagent ends");
    let parent = got
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-ask-1"))
        .expect("the parent's subagent ends");
    assert!(child < parent, "{got:#?}");
    assert!(
        got[child].contains("{\"status\":\"canceled\"}"),
        "{}",
        got[child]
    );
    // the child's own end, from the log, is its activity again, with what the core said
    let after = &got[parent..];
    assert!(
        after
            .iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT ask-2 vymalo.ask")
                && l.contains("\"error\":\"the asking task ended\"")
                && l.contains("\"state\":\"canceled\"")),
        "{got:#?}"
    );
    assert_eq!(
        got.iter().filter(|l| l.starts_with("SUBAGENT_")).count(),
        // sub-2 started, both asks started, both asks ended
        5,
        "{got:#?}"
    );
}

#[test]
fn the_steps_of_the_asked_agent_are_attributed_to_its_subagent() {
    let mut events = asking();
    events.push(step(
        4,
        "tool-c1",
        &["ask-1"],
        StepKind::Tool,
        StepState::Running,
        StepPhase::Start,
    ));
    events.push(step(
        5,
        "tool-c1",
        &["ask-1"],
        StepKind::Tool,
        StepState::Completed,
        StepPhase::End,
    ));
    let got = all_lines(&events);
    assert_eq!(
        from(&got, "ACTIVITY_SNAPSHOT step-4"),
        [
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"tool-c1\",\"kind\":\"tool\",\"label\":\"label tool-c1\",\"path\":[\"ask-1\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"running\"} @sub-ask-1  id:4",
            "ACTIVITY_SNAPSHOT step-4 vymalo.step {\"id\":\"tool-c1\",\"kind\":\"tool\",\"label\":\"label tool-c1\",\"path\":[\"ask-1\"],\"startedAt\":\"2027-01-15T08:00:04Z\",\"state\":\"completed\"} @sub-ask-1  id:5",
        ]
    );
    // after the ask ended its subagent is closed: a step that comes later is the invocation's
    events.push(answered(6, "coder", data(1, AskOutcome::Completed)));
    events.push(step(
        7,
        "tool-late",
        &["ask-1"],
        StepKind::Tool,
        StepState::Completed,
        StepPhase::End,
    ));
    let got = all_lines(&events);
    assert!(
        got.last().unwrap().ends_with("@sub-2  id:7"),
        "{:?}",
        got.last()
    );
}

#[test]
fn an_ask_the_log_does_not_end_is_canceled_with_the_invocation_that_holds_it() {
    // a log that ends the asking task without the asks' ends (the core writes them, a cut copy may
    // not): nothing stays open, and no spinner is left
    let mut events = asking();
    events.push(asked(4, 2, Caller::Ask(1), "reviewer", "check it", None));
    events.push(status(5, AgentStatus::Completed, None));
    events.push(thread(6, ThreadState::Done));
    let frames = support::flatten(&project(&events));
    let got = lines(&frames);
    let tail = from(
        &got,
        "ACTIVITY_SNAPSHOT ask-2 vymalo.ask {\"agent\":\"reviewer\",\"ask\":2,\"by\":\"ask:1\",\"depth\":2,\"error\"",
    );
    assert!(
        tail[1].starts_with("SUBAGENT_FINISHED sub-ask-2")
            && tail[1].contains("{\"status\":\"canceled\"}"),
        "{got:#?}"
    );
    for n in [1, 2] {
        assert_eq!(
            activity(&frames, &format!("ask-{n}"))["content"]["state"],
            "canceled"
        );
    }
    // the deepest first: sub-ask-2, then sub-ask-1, then the invocation
    let order: Vec<&String> = got
        .iter()
        .filter(|l| l.starts_with("SUBAGENT_FINISHED"))
        .collect();
    assert!(
        order[0].contains("sub-ask-2")
            && order[1].contains("sub-ask-1")
            && order[2].contains("sub-2"),
        "{order:?}"
    );
}

#[test]
fn the_persons_cancel_ends_the_asks_before_the_threads_own_end() {
    // as the core logs it: the asks end with the person's stop, then the task, then the state
    let mut events = asking();
    events.push(ended(4, {
        let mut d = data(1, AskOutcome::Canceled);
        d.error = Some("the person stopped the job".to_owned());
        d
    }));
    events.push(status(5, AgentStatus::Canceled, None));
    events.push(thread(6, ThreadState::Cancelled));
    let got = all_lines(&events);
    assert!(
        got.iter()
            .any(|l| l
                == "SUBAGENT_FINISHED sub-ask-1 success result={\"state\":\"canceled\"}  id:4"),
        "{got:#?}"
    );
    assert!(got.last().unwrap().starts_with("RUN_FINISHED"));
}

#[test]
fn an_ask_that_outlives_a_suspended_invocation_ends_in_a_run_of_its_own() {
    // the agent waits for the person while its ask runs: the ask's subagent is suspended with the
    // invocation, and its end, in a run of its own, says the activity and nothing more
    let mut events = asking();
    events.push(status(4, AgentStatus::InputRequired, Some("which one?")));
    events.push(thread(5, ThreadState::Blocked));
    events.push(answered(6, "coder", {
        let mut d = data(1, AskOutcome::Completed);
        d.text = Some("late".to_owned());
        d
    }));
    let got = all_lines(&events);
    let suspended = got
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-ask-1 suspended"))
        .expect("the ask's subagent suspends with the invocation");
    let invocation = got
        .iter()
        .position(|l| l.starts_with("SUBAGENT_FINISHED sub-2 suspended"))
        .unwrap();
    assert!(suspended < invocation, "{got:#?}");
    let late = from(&got, "RUN_STARTED run-6");
    assert!(
        late.iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT ask-1 vymalo.ask")),
        "{late:#?}"
    );
    assert!(
        !late.iter().any(|l| l.starts_with("SUBAGENT_")),
        "its subagent was ended already: {late:#?}"
    );
}

#[test]
fn a_new_job_forgets_the_asks_of_the_old_one() {
    let mut events = asking();
    events.push(answered(4, "coder", data(1, AskOutcome::Completed)));
    events.push(status(5, AgentStatus::Completed, None));
    events.push(thread(6, ThreadState::Done));
    events.push(user(7, "again @coder"));
    events.push(ev(
        8,
        Actor::system(),
        EventBody::JobStarted(JobStartedData { job: 2 }),
    ));
    events.push(status(9, AgentStatus::Working, None));
    events.push(asked(10, 1, Caller::Main, "coder", "second", None));
    let got = all_lines(&events);
    // ask 1 of job 2 is a subagent of its own invocation, numbered again from 1
    assert!(
        got.iter()
            .filter(|l| l.starts_with("SUBAGENT_STARTED sub-ask-1"))
            .count()
            == 2,
        "{got:#?}"
    );
}

#[test]
fn a_client_that_joins_while_an_ask_runs_is_told_its_subagent_again_parents_first() {
    let mut events = asking();
    events.push(asked(4, 2, Caller::Ask(1), "reviewer", "check it", None));
    for cursor in [3, 4] {
        let mut connect = Connect::new(meta(), cursor, 4, Follow::Forever);
        let mut frames = Vec::new();
        for e in &events {
            frames.extend(connect.feed(e));
        }
        let got = lines(&frames);
        let started: Vec<&String> = got
            .iter()
            .filter(|l| l.starts_with("SUBAGENT_STARTED"))
            .collect();
        // from the middle of the run (before ask 2) or from its end: the same subagents, whether
        // the preamble or the event says them
        let want = [
            "SUBAGENT_STARTED sub-2 plain",
            "SUBAGENT_STARTED sub-ask-1 coder in sub-2",
            "SUBAGENT_STARTED sub-ask-2 reviewer in sub-ask-1",
        ];
        assert_eq!(started, want, "cursor {cursor}: {got:#?}");
        verify::check(&frames).unwrap();
    }
}

#[test]
fn the_asked_agents_own_words_are_untrusted_text_the_projection_only_carries() {
    // what was asked and what came back are agents' words: they travel as strings in the content,
    // never as anything the projection reads
    let text = "<script>alert(1)</script> {\"a\":1} \u{202e}";
    let mut events = vec![
        user(1, "go @coder"),
        status(2, AgentStatus::Working, None),
        asked(3, 1, Caller::Main, "coder", text, None),
    ];
    events.push(answered(4, "coder", {
        let mut d = data(1, AskOutcome::Completed);
        d.text = Some(text.to_owned());
        d
    }));
    let frames = support::flatten(&project(&events));
    let content = activity(&frames, "ask-1")["content"].clone();
    assert_eq!(content["text"], text);
    assert_eq!(content["answer"], text);
    let _ = json!(null);
}

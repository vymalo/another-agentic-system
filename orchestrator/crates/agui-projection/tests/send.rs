//! A message sent while an agent works (ADR 0036): what the projection says when a `user_message`
//! arrives inside an open run, for a steer and for a stop, on the logs the core writes.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_projection::{Audience, Connect, Follow, Frame, Projector};
use orch_agui_proto::Event as Wire;
use orch_core::{AgentTaskState, Event, GatePolicy, UserId};
use support::log::{Action, build, build_under, gate, meta, meta_under, verifier_gate};
use support::{flatten, lines, project_each_with, verify};

fn said(text: &str) -> Action {
    Action::User {
        text: text.to_owned(),
        ids: true,
    }
}

fn stop(text: &str) -> Action {
    Action::StopAndSend {
        text: text.to_owned(),
        ids: true,
    }
}

fn working() -> Action {
    Action::Status(AgentTaskState::Working, None)
}

fn task(state: AgentTaskState) -> Action {
    Action::Status(state, None)
}

/// What a viewer reads of the log, one line a frame.
fn story(events: &[Event]) -> Vec<String> {
    let frames = flatten(&project_each_with(events, meta()));
    verify::check(&frames).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines(&frames)));
    lines(&frames)
}

/// The same under a gate that requires the agent's own checks.
fn gated_story(events: &[Event]) -> Vec<String> {
    story_under(events, gate())
}

fn story_under(events: &[Event], gate: GatePolicy) -> Vec<String> {
    let frames = flatten(&project_each_with(events, meta_under(gate)));
    verify::check(&frames).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines(&frames)));
    lines(&frames)
}

/// The story without the resume points.
fn bare(story: Vec<String>) -> Vec<String> {
    story
        .into_iter()
        .map(|l| l.split("  id:").next().unwrap().to_owned())
        .collect()
}

#[test]
fn a_steered_message_ends_the_run_it_arrives_in_and_opens_its_own() {
    let events = build(&[
        said("go"),
        working(),
        said("you were wrong since line 1"),
        task(AgentTaskState::Completed),
    ]);
    assert_eq!(
        bare(story(&events)),
        [
            "RUN_STARTED r-1",
            "STATE_SNAPSHOT queued",
            "TEXT_MESSAGE_START m-1 user",
            "TEXT_MESSAGE_CONTENT m-1 \"go\"",
            "TEXT_MESSAGE_END m-1",
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT evt-2 vymalo.status {\"status\":\"working\"} @sub-2",
            "STATE_SNAPSHOT working",
            // the message: the run ends, the work does not (the invocation is suspended, with no
            // interrupt to wait for), and the thread is still working
            "SUBAGENT_FINISHED sub-2 suspended[]",
            "STATE_SNAPSHOT working",
            "RUN_FINISHED r-1 success",
            "RUN_STARTED r-2",
            "STATE_SNAPSHOT working",
            "TEXT_MESSAGE_START m-2 user",
            "TEXT_MESSAGE_CONTENT m-2 \"you were wrong since line 1\"",
            "TEXT_MESSAGE_END m-2",
            // the agent's next event re-opens the same invocation, as after an answered question
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT evt-4 vymalo.status {\"status\":\"completed\"} @sub-2",
            "SUBAGENT_FINISHED sub-2 success",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED r-2 success",
        ]
    );
}

fn start_metadata(frames: &[Frame], message_id: &str) -> serde_json::Value {
    frames
        .iter()
        .find_map(|f| match &f.event {
            Wire::TextMessageStart(e) if e.message_id.as_str() == message_id => {
                Some(serde_json::to_value(e.base.metadata.clone()).unwrap())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no TEXT_MESSAGE_START for {message_id}"))
}

#[test]
fn the_user_message_says_how_it_reached_the_agent() {
    let events = build(&[
        said("go"),
        working(),
        said("steer me"),
        stop("stop me"),
        said("after the stop"),
    ]);
    let frames = flatten(&project_each_with(&events, meta()));
    let delivery = |id: &str| start_metadata(&frames, id)["vymalo.delivery"].clone();
    assert_eq!(delivery("m-2"), "steer");
    assert_eq!(delivery("m-3"), "interrupt");
    // a message while a stop is landing joins it, and says so
    assert_eq!(delivery("m-4"), "interrupt");
    // the actor stays
    assert_eq!(
        start_metadata(&frames, "m-2")["vymalo.actor"]["type"],
        "user"
    );
}

#[test]
fn a_stop_shows_the_cancelled_task_and_the_next_job_in_the_run_of_its_message() {
    let events = build(&[
        said("go"),
        working(),
        stop("do X instead"),
        task(AgentTaskState::Canceled),
        working(),
        Action::Artifact,
        task(AgentTaskState::Completed),
    ]);
    assert_eq!(
        bare(story(&events))[8..],
        [
            "SUBAGENT_FINISHED sub-2 suspended[]",
            "STATE_SNAPSHOT working",
            "RUN_FINISHED r-1 success",
            "RUN_STARTED r-2",
            "STATE_SNAPSHOT working",
            "TEXT_MESSAGE_START m-2 user",
            "TEXT_MESSAGE_CONTENT m-2 \"do X instead\"",
            "TEXT_MESSAGE_END m-2",
            // the stopped task says it was cancelled, as the invocation it was
            "SUBAGENT_STARTED sub-2 plain",
            "ACTIVITY_SNAPSHOT evt-4 vymalo.status {\"status\":\"canceled\"} @sub-2",
            "SUBAGENT_FINISHED sub-2 success result={\"status\":\"canceled\"}",
            // the next job: no `done`, no `cancelled` for the abandoned one
            "ACTIVITY_SNAPSHOT job-2 vymalo.job {\"job\":2}",
            "STATE_SNAPSHOT queued",
            "SUBAGENT_STARTED sub-6 plain",
            "ACTIVITY_SNAPSHOT evt-6 vymalo.status {\"status\":\"working\"} @sub-6",
            "STATE_SNAPSHOT working",
            "ACTIVITY_SNAPSHOT evt-7 vymalo.artifact {\"kind\":\"file\",\"name\":\"result\",\"uri\":\"https://example.com/pr/1\"} @sub-6",
            "ACTIVITY_SNAPSHOT evt-8 vymalo.status {\"status\":\"completed\"} @sub-6",
            "SUBAGENT_FINISHED sub-6 success",
            "STATE_SNAPSHOT done",
            "RUN_FINISHED r-2 success",
        ]
    );
}

#[test]
fn every_end_of_a_stopped_task_leaves_one_run_open_until_the_next_job_is_judged() {
    // the task ends in each way the core lets it (row 5): the run of the message goes on, and the
    // job boundary ends what the task left open
    for ending in [
        AgentTaskState::Completed,
        AgentTaskState::Failed,
        AgentTaskState::Canceled,
        AgentTaskState::Rejected,
    ] {
        let events = build(&[said("go"), working(), stop("next"), task(ending)]);
        let s = bare(story(&events));
        assert_eq!(
            s.iter().filter(|l| l.starts_with("RUN_STARTED")).count(),
            2,
            "{ending:?}: {s:#?}"
        );
        assert!(
            !s.iter().any(|l| l.contains("RUN_ERROR")),
            "{ending:?}: the abandoned job's failure does not fail the run: {s:#?}"
        );
        assert_eq!(s.last().unwrap(), "STATE_SNAPSHOT queued", "{ending:?}");
        let mut projector = Projector::new(meta());
        for e in &events {
            projector.apply(e, Audience::Viewer);
        }
        assert!(projector.run_open(), "{ending:?}: the next job is running");
    }
}

#[test]
fn a_stopped_job_is_not_verified_when_its_task_still_completes() {
    // the gate asks for the agent's own checks: the abandoned job's `completed` starts none
    let events = build_under(
        &[
            said("go"),
            working(),
            stop("do X instead"),
            task(AgentTaskState::Completed),
            working(),
            Action::Branch { commit: 1 },
            Action::Checks {
                passed: true,
                commit: 1,
            },
            task(AgentTaskState::Completed),
        ],
        &gate(),
    );
    let s = bare(gated_story(&events));
    let job = s.iter().position(|l| l.contains("vymalo.job")).unwrap();
    assert!(
        !s[..job].iter().any(|l| l == "STATE_SNAPSHOT verifying"),
        "{s:#?}"
    );
    // the next job is verified from its first attempt, and it is the first verification
    assert!(
        s[job..]
            .iter()
            .any(|l| l.starts_with("ACTIVITY_SNAPSHOT check-1-1-agent_checks")),
        "{s:#?}"
    );
    assert_eq!(s.last().unwrap(), "RUN_FINISHED r-2 success");
}

#[test]
fn a_stopped_task_that_asks_is_logged_and_nobody_waits_for_the_answer() {
    // row 6: no interrupt, and the invocation is not suspended
    let events = build(&[
        said("go"),
        working(),
        stop("do X instead"),
        Action::Status(AgentTaskState::InputRequired, Some("Which branch?".into())),
        task(AgentTaskState::Canceled),
    ]);
    let s = bare(story(&events));
    assert!(!s.iter().any(|l| l.contains("interrupt[")), "{s:#?}");
    assert!(
        s.iter().any(|l| l.starts_with("TEXT_MESSAGE_START st-4")),
        "the question is shown: {s:#?}"
    );
    assert!(
        s.iter()
            .any(|l| l == "SUBAGENT_FINISHED sub-2 success result={\"status\":\"canceled\"}"),
        "{s:#?}"
    );
}

#[test]
fn a_stop_the_agent_refused_for_good_goes_back_to_being_judged() {
    // row 7: the job goes on with the message steered to it, so its `completed` is verified
    let refused = |retryable: bool| {
        build_under(
            &[
                said("go"),
                working(),
                stop("do X instead"),
                Action::CancelRejected { retryable },
                Action::Branch { commit: 1 },
                Action::Checks {
                    passed: true,
                    commit: 1,
                },
                task(AgentTaskState::Completed),
            ],
            &gate(),
        )
    };
    let s = bare(gated_story(&refused(false)));
    assert!(s.iter().any(|l| l == "STATE_SNAPSHOT verifying"), "{s:#?}");
    // a refusal that may pass keeps the stop on its way: nothing is verified
    let s = bare(gated_story(&refused(true)));
    assert!(!s.iter().any(|l| l == "STATE_SNAPSHOT verifying"), "{s:#?}");
}

#[test]
fn a_second_message_while_the_stop_lands_is_a_run_of_its_own() {
    let events = build(&[
        said("go"),
        working(),
        stop("one"),
        said("two"),
        task(AgentTaskState::Canceled),
    ]);
    let s = bare(story(&events));
    assert_eq!(
        s.iter()
            .filter(|l| l.starts_with("RUN_STARTED") || l.starts_with("RUN_FINISHED"))
            .collect::<Vec<_>>(),
        [
            "RUN_STARTED r-1",
            "RUN_FINISHED r-1 success",
            "RUN_STARTED r-2",
            "RUN_FINISHED r-2 success",
            "RUN_STARTED r-3",
        ]
    );
    // the invocation was suspended once: there was none open for the second message to suspend
    assert_eq!(
        s.iter()
            .filter(|l| l.starts_with("SUBAGENT_FINISHED") && l.contains("suspended"))
            .count(),
        1
    );
}

#[test]
fn a_message_while_the_work_is_verified_opens_its_own_run_and_abandons_the_verification() {
    // a verifier is out when the person writes
    let events = build_under(
        &[
            said("go"),
            working(),
            Action::Branch { commit: 1 },
            task(AgentTaskState::Completed),
            said("wait, the logout too"),
        ],
        &verifier_gate(),
    );
    let s = bare(story_under(&events, verifier_gate()));
    let from = s
        .iter()
        .position(|l| l.starts_with("SUBAGENT_STARTED sub-verify-1"))
        .unwrap();
    assert_eq!(
        s[from + 2..],
        [
            "SUBAGENT_FINISHED sub-verify-1 success result={\"status\":\"canceled\"}",
            "STATE_SNAPSHOT queued",
            "RUN_FINISHED r-1 success",
            "RUN_STARTED r-2",
            "STATE_SNAPSHOT queued",
            "TEXT_MESSAGE_START m-2 user",
            "TEXT_MESSAGE_CONTENT m-2 \"wait, the logout too\"",
            "TEXT_MESSAGE_END m-2",
        ]
    );
    // nothing is running, so the message is not a steer: no `vymalo.delivery`
    let frames = flatten(&project_each_with(&events, meta_under(verifier_gate())));
    assert_eq!(
        start_metadata(&frames, "m-2")["vymalo.delivery"],
        serde_json::Value::Null
    );
}

#[test]
fn the_requester_of_a_mid_run_message_does_not_get_it_back() {
    let events = build(&[said("go"), working(), said("steer me")]);
    let held: BTreeSet<String> = ["m-2".to_owned()].into();
    let mut projector = Projector::new(meta());
    let mut out = Vec::new();
    for e in &events {
        out.extend(projector.apply(
            e,
            Audience::Requester {
                held_message_ids: &held,
            },
        ));
    }
    let s = lines(&out);
    assert!(s.iter().any(|l| l.starts_with("RUN_STARTED r-2")));
    assert!(!s.iter().any(|l| l.contains("m-2")), "{s:#?}");
}

#[test]
fn the_view_of_a_thread_with_a_mid_run_message_has_the_new_run_open() {
    let events = build(&[said("go"), working(), said("steer me")]);
    let mut projector = Projector::new(meta());
    for e in &events {
        projector.apply(e, Audience::Viewer);
    }
    let orch_agui_projection::ThreadView::Known(view) = projector.view(&UserId::new("a@b.c"))
    else {
        panic!("a known thread");
    };
    assert!(view.run_open);
    assert!(view.run_ids.contains("r-1") && view.run_ids.contains("r-2"));
    assert!(view.message_ids.contains("m-2"));
}

// ---- reconnecting ---------------------------------------------------------------------------

/// What a connect stream writes from `cursor`, after the whole log is replayed.
fn connected(events: &[Event], cursor: i64) -> Vec<Frame> {
    let head = i64::try_from(events.len()).unwrap();
    let mut connect = Connect::new(meta(), cursor, head, Follow::Forever);
    events.iter().flat_map(|e| connect.feed(e)).collect()
}

#[test]
fn a_reconnect_at_the_message_gets_the_run_it_opened_and_what_follows() {
    let events = build(&[
        said("go"),
        working(),
        stop("do X instead"),
        task(AgentTaskState::Canceled),
        working(),
        task(AgentTaskState::Completed),
    ]);
    // the message is event 3; the frames of event 3 end with a resume point
    let whole = flatten(&project_each_with(&events, meta()));
    let at = whole.iter().position(|f| f.resume_id == Some(3)).unwrap();
    let got = connected(&events, 3);
    verify::check(&got).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines(&got)));
    assert_eq!(
        lines(&got[..2]),
        ["RUN_STARTED r-2", "STATE_SNAPSHOT working"],
        "the run the message opened, with nothing of the one it ended"
    );
    assert!(got[..2].iter().all(|f| f.resume_id.is_none()));
    assert_eq!(&got[2..], &whole[at + 1..]);
}

#[test]
fn a_reconnect_before_the_message_gets_the_old_run_and_then_its_end_and_the_new_run() {
    let events = build(&[
        said("go"),
        working(),
        said("steer me"),
        task(AgentTaskState::Completed),
    ]);
    let got = connected(&events, 2);
    verify::check(&got).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines(&got)));
    let s = bare(lines(&got));
    assert_eq!(
        s[..6],
        [
            "RUN_STARTED r-1",
            "SUBAGENT_STARTED sub-2 plain",
            "STATE_SNAPSHOT working",
            "SUBAGENT_FINISHED sub-2 suspended[]",
            "STATE_SNAPSHOT working",
            "RUN_FINISHED r-1 success",
        ]
    );
    assert_eq!(s[6], "RUN_STARTED r-2");
}

#[test]
fn a_reconnect_after_the_agent_went_on_re_opens_the_run_with_the_reopened_invocation() {
    let events = build(&[
        said("go"),
        working(),
        said("steer me"),
        working(),
        Action::Artifact,
        task(AgentTaskState::Completed),
    ]);
    // after the agent's first word in the new run (event 4)
    let got = connected(&events, 4);
    verify::check(&got).unwrap_or_else(|e| panic!("{e}\n{:#?}", lines(&got)));
    assert_eq!(
        bare(lines(&got))[..3],
        [
            "RUN_STARTED r-2",
            "SUBAGENT_STARTED sub-2 plain",
            "STATE_SNAPSHOT working"
        ],
        "the same invocation, in the run that follows the message"
    );
}

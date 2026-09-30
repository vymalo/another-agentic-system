//! CI reports in the projection (ADR 0017, `docs/api/agui.md`): every report is a `vymalo.ci`
//! activity, counted by the gate or not, with a stable id that depends on the report alone.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use orch_agui_projection::{Audience, Frame, Projector};
use orch_agui_proto::testkit::assert_conforms;
use orch_core::{
    Actor, AgentTaskState, CiConclusion, CiProvider, CiReport, Event, EventBody, GatePolicy,
    ThreadState, Timestamp,
};
use serde_json::{Value, json};
use support::log::{Action, build_under, ci_gate, ci_report, gate, meta_under, thread_id};
use support::{lines, verify};

const WORKING: Action = Action::Status(AgentTaskState::Working, None);
const COMPLETED: Action = Action::Status(AgentTaskState::Completed, None);

fn user() -> Action {
    Action::User {
        text: "fix the login".to_owned(),
        ids: true,
    }
}

fn ci(name: &'static str, commit: u8, conclusion: CiConclusion) -> Action {
    Action::Ci {
        name,
        commit,
        conclusion,
    }
}

fn project(actions: &[Action], gate: &GatePolicy) -> (Vec<Event>, Vec<Frame>, Projector) {
    let events = build_under(actions, gate);
    let mut projector = Projector::new(meta_under(gate.clone()));
    let frames: Vec<Frame> = events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect();
    for frame in &frames {
        assert_conforms(&frame.event);
    }
    verify::check(&frames).unwrap();
    (events, frames, projector)
}

/// The content of every `vymalo.ci` snapshot, with its message id and `replace`.
fn cards(frames: &[Frame]) -> Vec<(String, Option<bool>, Value)> {
    frames
        .iter()
        .filter_map(|f| match &f.event {
            orch_agui_proto::Event::ActivitySnapshot(e) if e.activity_type == "vymalo.ci" => {
                Some((
                    e.message_id.as_str().to_owned(),
                    e.replace,
                    Value::Object(e.content.clone()),
                ))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn a_report_is_a_vymalo_ci_card_about_a_commit_and_a_check() {
    let (_, frames, _) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("ci/build", 1, CiConclusion::Failure),
        ],
        &ci_gate(),
    );
    let cards = cards(&frames);
    assert_eq!(cards.len(), 1);
    let (id, replace, content) = &cards[0];
    assert_eq!(id, &format!("ci-{:040x}-ci/build", 1));
    assert_eq!(*replace, Some(true));
    assert_eq!(
        content,
        &json!({
            "name": "ci/build",
            "conclusion": "failure",
            "passed": false,
            "sha": format!("{:040x}", 1),
            "shortSha": "0000000",
            "provider": "generic",
            "repository": "github.com/acme/demo",
            "branch": "agent/x",
            "url": "https://ci.example.com/runs/1",
            "summary": "2 tests failed",
        })
    );
}

#[test]
fn the_card_comes_before_what_the_report_decided() {
    // A red report for the pushed commit: the card, then the failed check, then the rework.
    let (_, frames, _) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("ci/build", 1, CiConclusion::Failure),
        ],
        &ci_gate(),
    );
    let story: Vec<String> = lines(&frames)
        .into_iter()
        .filter(|l| {
            l.contains("vymalo.ci") || l.contains("vymalo.check") || l.contains("vymalo.rework")
        })
        .collect();
    assert_eq!(story.len(), 4, "{story:?}");
    assert!(
        story[0].contains("check-1-1-ci vymalo.check"),
        "the pending card: {story:?}"
    );
    assert!(story[1].contains("vymalo.ci"), "{story:?}");
    assert!(story[2].contains("check-1-1-ci vymalo.check"), "{story:?}");
    assert!(story[3].contains("rework-2"), "{story:?}");
}

#[test]
fn a_check_that_runs_again_replaces_its_card_and_another_commit_has_its_own() {
    let (_, frames, _) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("build", 1, CiConclusion::Failure),
            ci("build", 1, CiConclusion::Success),
            ci("lint", 1, CiConclusion::Success),
            ci("build", 2, CiConclusion::Success),
        ],
        &GatePolicy::default(),
    );
    let cards = cards(&frames);
    let ids: Vec<&str> = cards.iter().map(|(id, _, _)| id.as_str()).collect();
    let one = format!("ci-{:040x}-build", 1);
    assert_eq!(
        ids,
        [
            one.as_str(),
            one.as_str(),
            &format!("ci-{:040x}-lint", 1),
            &format!("ci-{:040x}-build", 2),
        ],
        "the same check on the same commit keeps its id, so the second replaces the first"
    );
    assert!(cards.iter().all(|(_, replace, _)| *replace == Some(true)));
    assert_eq!(cards[0].2["conclusion"], "failure");
    assert_eq!(cards[1].2["conclusion"], "success");
    assert_eq!(cards[1].2["passed"], true);
}

#[test]
fn a_report_nobody_counts_is_still_a_card_and_changes_nothing_else() {
    // No gate requires CI: the report is a card and the thread stays as it was.
    let (events, frames, projector) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("build", 1, CiConclusion::Failure),
        ],
        &GatePolicy::default(),
    );
    assert_eq!(cards(&frames).len(), 1);
    assert_eq!(projector.thread_state(), ThreadState::Done);
    assert!(matches!(
        events.last().unwrap().body,
        EventBody::CiResult(_)
    ));
    // A gate on the agent's own checks does not count it either.
    let (_, frames, _) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("build", 1, CiConclusion::Success),
        ],
        &gate(),
    );
    assert_eq!(cards(&frames).len(), 1);
}

#[test]
fn a_report_after_the_job_ended_opens_a_run_of_its_own_and_closes_it() {
    let (events, frames, projector) = project(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("build", 1, CiConclusion::Success),
            ci("lint", 1, CiConclusion::Failure),
        ],
        &ci_gate(),
    );
    // The first report is the verdict (the job is done); the second arrives afterwards.
    assert_eq!(projector.thread_state(), ThreadState::Done);
    assert!(!projector.run_open());
    let story = lines(&frames);
    let runs = story
        .iter()
        .filter(|l| l.starts_with("RUN_STARTED"))
        .count();
    let ends = story
        .iter()
        .filter(|l| l.starts_with("RUN_FINISHED") || l.starts_with("RUN_ERROR"))
        .count();
    assert_eq!((runs, ends), (2, 2), "{story:?}");
    assert!(matches!(
        events.last().unwrap().body,
        EventBody::CiResult(_)
    ));
    assert_eq!(cards(&frames).len(), 2);
}

#[test]
fn the_ids_do_not_depend_on_what_was_folded_before() {
    // The same report projects to the same card wherever it sits in the log, and a viewer that
    // joins late (a fresh projector over the same events) gets the same ids.
    let events = build_under(
        &[
            user(),
            WORKING,
            Action::Branch { commit: 1 },
            COMPLETED,
            ci("ci/build", 1, CiConclusion::Failure),
            ci("ci/build", 2, CiConclusion::Success),
        ],
        &ci_gate(),
    );
    let ids = |events: &[Event]| -> Vec<String> {
        let mut p = Projector::new(meta_under(ci_gate()));
        let frames: Vec<Frame> = events
            .iter()
            .flat_map(|e| p.apply(e, Audience::Viewer))
            .collect();
        cards(&frames).into_iter().map(|c| c.0).collect()
    };
    assert_eq!(ids(&events), ids(&events));
    assert_eq!(ids(&events).len(), 2);
    let just_the_report: Vec<Event> = events
        .iter()
        .filter(|e| matches!(e.body, EventBody::CiResult(_)))
        .cloned()
        .collect();
    assert_eq!(ids(&just_the_report)[0], ids(&events)[0]);
}

/// The text of a report comes from outside: it is carried as plain strings, never interpreted,
/// and a link that is not http(s) is not passed on.
#[test]
fn untrusted_text_is_carried_as_text_and_only_http_links_are_passed() {
    let hostile = "<script>alert(1)</script> [click](javascript:alert(1)) \u{202e}";
    let event = |url: Option<&str>| Event {
        seq: 1,
        thread_id: thread_id(),
        at: Timestamp::from_second(1_800_000_001).unwrap(),
        actor: Actor::system(),
        body: EventBody::CiResult(CiReport {
            provider: CiProvider::Github,
            repository: "github.com/acme/demo".to_owned(),
            sha: format!("{:040x}", 7),
            branch: None,
            name: hostile.to_owned(),
            conclusion: CiConclusion::ActionRequired,
            url: url.map(str::to_owned),
            summary: Some(hostile.to_owned()),
        }),
    };
    let card_of = |url: Option<&str>| {
        let mut p = Projector::new(meta_under(GatePolicy::default()));
        let frames = p.apply(&event(url), Audience::Viewer);
        for frame in &frames {
            assert_conforms(&frame.event);
        }
        cards(&frames).remove(0).2
    };
    let card = card_of(Some("https://ci.example.com/run"));
    assert_eq!(card["summary"], hostile);
    assert_eq!(card["name"], hostile);
    assert_eq!(card["passed"], false, "action_required is not a pass");
    assert_eq!(card["provider"], "github");
    assert_eq!(card["url"], "https://ci.example.com/run");
    for bad in [
        "javascript:alert(1)",
        "data:text/html,x",
        "file:///etc/passwd",
        "//evil",
        "",
    ] {
        assert!(
            card_of(Some(bad)).get("url").is_none(),
            "{bad:?} is not passed on"
        );
    }
    assert!(card_of(None).get("url").is_none());
}

#[test]
fn every_conclusion_says_whether_it_passed() {
    let cases = [
        (CiConclusion::Success, "success", true),
        (CiConclusion::Neutral, "neutral", true),
        (CiConclusion::Skipped, "skipped", true),
        (CiConclusion::Failure, "failure", false),
        (CiConclusion::Cancelled, "cancelled", false),
        (CiConclusion::TimedOut, "timed_out", false),
        (CiConclusion::ActionRequired, "action_required", false),
        (CiConclusion::Stale, "stale", false),
        (CiConclusion::StartupFailure, "startup_failure", false),
    ];
    for (conclusion, name, passed) in cases {
        let mut p = Projector::new(meta_under(GatePolicy::default()));
        let e = Event {
            seq: 1,
            thread_id: thread_id(),
            at: Timestamp::from_second(1_800_000_001).unwrap(),
            actor: Actor::system(),
            body: EventBody::CiResult(ci_report("build", 1, conclusion)),
        };
        let card = cards(&p.apply(&e, Audience::Viewer)).remove(0).2;
        assert_eq!(card["conclusion"], name);
        assert_eq!(card["passed"], passed, "{name}");
    }
}

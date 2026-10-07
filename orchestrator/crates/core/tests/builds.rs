//! The builds of the agents a thread worked with (ADR 0053): a note in the job ledger, not an
//! event, bounded again at the door, deduplicated and kept for the whole conversation.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_core::ThreadState::{Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::*;

fn build(agent: &str, version: &str) -> AgentBuild {
    AgentBuild::new(AgentId::new(agent), Some("Adam"), Some(version), [])
}

fn noted(snap: &Snapshot, b: AgentBuild) -> Snapshot {
    let (next, cmds) = transition(snap, &Input::AgentBuild { build: b }).unwrap();
    assert!(
        cmds.is_empty(),
        "a note writes nothing to the log: {cmds:?}"
    );
    next
}

#[test]
fn a_build_is_noted_in_every_state_and_writes_no_event() {
    for state in [Queued, Working, Verifying, Done, Failed, Cancelled] {
        let snap = Snapshot::new(state);
        let after = noted(&snap, build("adam", "0.3.0+abc1234"));
        assert_eq!(after.state, state);
        assert_eq!(after.job.builds.len(), 1, "{state:?}");
        assert_eq!(after.job.builds[0].job, 1);
    }
}

#[test]
fn the_same_build_again_changes_nothing_and_a_new_one_is_added() {
    let one = noted(&Snapshot::new(Working), build("adam", "0.3.0+abc1234"));
    let again = noted(&one, build("adam", "0.3.0+abc1234"));
    assert_eq!(again, one);
    // another agent, and the same agent upgraded
    let two = noted(&one, build("chat", "1.0.0"));
    let three = noted(&two, build("adam", "0.4.0+def5678"));
    let versions: Vec<_> = three
        .job
        .builds
        .iter()
        .map(|b| (b.agent.as_str(), b.version.as_deref().unwrap()))
        .collect();
    assert_eq!(
        versions,
        [
            ("adam", "0.3.0+abc1234"),
            ("chat", "1.0.0"),
            ("adam", "0.4.0+def5678")
        ]
    );
    // downgraded back: it is a change from the latest, so it is noted again
    let four = noted(&three, build("adam", "0.3.0+abc1234"));
    assert_eq!(four.job.builds.len(), 4);
}

#[test]
fn the_conversation_keeps_its_builds_into_the_next_job_and_records_the_job_it_was_seen_in() {
    let first = noted(&Snapshot::new(Done), build("adam", "0.3.0+abc1234"));
    let (two, _) = transition(
        &first,
        &Input::UserMessage {
            user: UserId::new("u@x.io"),
            text: "go on".into(),
            message_id: None,
            run_id: None,
            origin: Origin::Agui,
            catalog: None,
            mentions: Vec::new(),
        },
    )
    .unwrap();
    assert_eq!(two.job.number, 2);
    assert_eq!(two.job.builds, first.job.builds, "kept by the next job");
    let upgraded = noted(&two, build("adam", "0.4.0+def5678"));
    let jobs: Vec<u32> = upgraded.job.builds.iter().map(|b| b.job).collect();
    assert_eq!(jobs, [1, 2], "the job each build was first seen in");
    // the caller cannot say which job: the core does
    let mut lie = build("adam", "9.9.9");
    lie.job = 77;
    assert_eq!(noted(&upgraded, lie).job.builds.last().unwrap().job, 2);
}

#[test]
fn what_a_card_says_is_cut_to_the_limits_at_the_door() {
    let long = "x".repeat(10_000);
    let params = (0..40).map(|i| (format!("{i:02}{}", "k".repeat(500)), long.clone()));
    let b = AgentBuild::new(AgentId::new("adam"), Some(&long), Some("  "), params);
    assert_eq!(b.name.as_deref().unwrap().len(), MAX_BUILD_TEXT_BYTES);
    assert_eq!(b.version, None, "blank is nothing");
    assert_eq!(b.build.len(), MAX_BUILD_PARAMS);
    assert!(
        b.build
            .iter()
            .all(|(k, v)| k.len() <= MAX_BUILD_KEY_BYTES && v.len() == MAX_BUILD_VALUE_BYTES)
    );
    // an input that skipped the constructor is cut again by the core
    let raw = AgentBuild {
        agent: AgentId::new("adam"),
        name: Some(long.clone()),
        version: None,
        build: [("k".to_owned(), long)].into(),
        job: 1,
    };
    let after = noted(&Snapshot::new(Working), raw);
    assert_eq!(
        after.job.builds[0].name.as_deref().unwrap().len(),
        MAX_BUILD_TEXT_BYTES
    );
    assert_eq!(after.job.builds[0].build["k"].len(), MAX_BUILD_VALUE_BYTES);
}

#[test]
fn a_ledger_keeps_the_newest_builds_and_survives_json() {
    let mut snap = Snapshot::new(Working);
    for i in 0..(MAX_BUILDS + 5) {
        snap = noted(&snap, build("adam", &format!("0.{i}.0")));
    }
    assert_eq!(snap.job.builds.len(), MAX_BUILDS);
    assert_eq!(
        snap.job.builds.last().unwrap().version.as_deref(),
        Some(format!("0.{}.0", MAX_BUILDS + 4).as_str())
    );
    let back: Job = serde_json::from_value(serde_json::to_value(&snap.job).unwrap()).unwrap();
    assert_eq!(back, snap.job);
    // a ledger stored before the field existed has none
    let old: Job = serde_json::from_value(serde_json::json!({"number": 2})).unwrap();
    assert!(old.builds.is_empty());
    assert!(
        serde_json::to_value(Job::default())
            .unwrap()
            .get("builds")
            .is_none(),
        "absent when empty, so an older reader sees what it always saw"
    );
}

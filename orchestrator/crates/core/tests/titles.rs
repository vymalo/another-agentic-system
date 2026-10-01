//! A thread's title: a person's rename is valid in every state, is recorded with its writer, is
//! remembered by the thread's ledger across jobs and across a restart, and changes nothing else.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use ThreadState::{Blocked, Cancelled, Done, Failed, Queued, Verifying, Working};
use orch_core::*;
use serde_json::json;

const STATES: [ThreadState; 7] = [Queued, Working, Verifying, Blocked, Done, Failed, Cancelled];

fn user() -> UserId {
    UserId::new("me@example.com")
}

fn rename(title: &str) -> Input {
    Input::Rename {
        user: user(),
        title: title.into(),
    }
}

fn message(text: &str) -> Input {
    Input::UserMessage {
        user: user(),
        text: text.into(),
        message_id: None,
        run_id: None,
        origin: Origin::Agui,
        catalog: None,
    }
}

fn titled(cmds: &[Command]) -> Vec<(&str, TitledBy, &Actor)> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Append(EventDraft {
                actor,
                body: EventBody::ThreadTitled(d),
            }) => Some((d.title.as_str(), d.source, actor)),
            _ => None,
        })
        .collect()
}

fn set_titles(cmds: &[Command]) -> Vec<&str> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::SetTitle(t) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_new_thread_has_its_first_words_and_no_event_for_them() {
    assert_eq!(Job::default().title.source(), TitleSource::FirstMessage);
    assert!(Job::default().title.is_empty());
}

#[test]
fn a_rename_says_who_wrote_it_and_stores_it_in_every_state() {
    for state in STATES {
        let before = Snapshot::new(state);
        let (after, cmds) = transition(&before, &rename("Fix the build")).unwrap();
        assert_eq!(after.state, state, "{state:?}");
        assert_eq!(
            titled(&cmds),
            vec![("Fix the build", TitledBy::User, &Actor::user(&user()))],
            "{state:?}"
        );
        assert_eq!(set_titles(&cmds), ["Fix the build"], "{state:?}");
        assert_eq!(
            cmds.len(),
            2,
            "{state:?}: nothing but the event and the title"
        );
        assert_eq!(after.job.title.source(), TitleSource::User, "{state:?}");
    }
}

#[test]
fn a_rename_touches_nothing_of_the_job_but_the_title() {
    let mut gate = GatePolicy::requiring([CheckSource::Verifier]);
    gate.max_attempts = 3;
    let before = Snapshot {
        state: Blocked,
        job: Job {
            number: 2,
            gate,
            attempt: 2,
            verification: 4,
            task: Some("the task".into()),
            hold: Some(Hold::VerifierFailed),
            ..Job::default()
        },
    };
    let (after, _) = transition(&before, &rename("Fix the build")).unwrap();
    assert_eq!(
        after.job,
        Job {
            title: after.job.title,
            ..before.job.clone()
        }
    );
    assert_eq!(after.job.title.source(), TitleSource::User);
    // a blocked thread stays blocked for the reason it was
    assert_eq!(after.state, Blocked);
    assert_eq!(after.job.hold, Some(Hold::VerifierFailed));
}

#[test]
fn the_last_rename_is_the_title() {
    let (snap, first) = transition(&Snapshot::new(Working), &rename("One")).unwrap();
    let (snap, second) = transition(&snap, &rename("Two")).unwrap();
    assert_eq!(set_titles(&first), ["One"]);
    assert_eq!(set_titles(&second), ["Two"]);
    assert_eq!(snap.job.title.source(), TitleSource::User);
}

#[test]
fn the_next_job_keeps_the_title_the_person_wrote() {
    for state in [Done, Failed, Cancelled] {
        let (renamed, _) = transition(&Snapshot::new(state), &rename("Mine")).unwrap();
        let (next, cmds) = transition(&renamed, &message("and now this")).unwrap();
        assert_eq!(next.state, Queued, "{state:?}");
        assert_eq!(next.job.number, 2, "{state:?}");
        assert_eq!(next.job.title.source(), TitleSource::User, "{state:?}");
        // the message does not rename the thread
        assert!(titled(&cmds).is_empty(), "{state:?}");
        assert!(set_titles(&cmds).is_empty(), "{state:?}");
    }
    let job = Job::default().next();
    assert_eq!(job.title.source(), TitleSource::FirstMessage);
}

#[test]
fn a_ledger_written_before_titles_existed_reads_as_the_first_words() {
    let old: Job = serde_json::from_value(json!({"number": 2, "attempt": 1})).unwrap();
    assert_eq!(old.title.source(), TitleSource::FirstMessage);
    let empty: Job = serde_json::from_value(json!({})).unwrap();
    assert_eq!(empty, Job::default());
    // and a job that kept the first words says nothing of it
    assert!(
        serde_json::to_value(Job::default())
            .unwrap()
            .get("title")
            .is_none()
    );
}

#[test]
fn the_ledger_survives_the_store_as_json() {
    let (renamed, _) = transition(&Snapshot::new(Working), &rename("Mine")).unwrap();
    let stored = serde_json::to_value(&renamed.job).unwrap();
    assert_eq!(stored["title"], json!({"source": "user"}));
    let read: Job = serde_json::from_value(stored).unwrap();
    assert_eq!(read, renamed.job);
    assert_eq!(read.title.source(), TitleSource::User);
}

#[test]
fn a_clients_view_of_the_job_does_not_say_who_wrote_the_title() {
    // the title itself is `Thread.title`; the ledger is the thread's own business
    let mut gate = GatePolicy::requiring([CheckSource::Verifier]);
    gate.max_attempts = 2;
    let snap = Snapshot::queued(gate);
    let (renamed, _) = transition(&snap, &rename("Mine")).unwrap();
    let view = serde_json::to_value(renamed.job.view().unwrap()).unwrap();
    assert!(view.get("title").is_none(), "{view}");
}

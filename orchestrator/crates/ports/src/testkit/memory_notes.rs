//! `MemoryNoteStore` conformance cases: what the API and the memory tools rely on from any store of
//! a person's notes and grants (ADR 0091, `docs/api/memory-v1.md`), stated once and run against the
//! in-memory store and the Postgres store.
//!
//! An implementation supplies a fresh store (`async fn() -> Option<S>`; `None` skips the suite).
//! Each case uses people of its own (so a store shared by the cases of one run is never confused)
//! and gives up after 60 seconds.
//!
//! What is deliberately not asserted: how a note is kept, and the wording of any error. The rules of
//! what a note is and what a read returns are `orch-core`'s `memory` module (it has its own vectors);
//! the cases here are the ones only a store can break: **whose** a note is, what it keeps, in which
//! order it comes back, and how many a person holds.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use orch_core::{
    ForgetInput, Grant, MAX_NOTE_BYTES, MAX_NOTES, MAX_READ_NOTES, MemorySettings, Note, ReadInput,
    SaveInput, Timestamp, UserId,
};
use uuid::Uuid;

use crate::{MemoryNoteStore, MemoryNoteStoreError};

/// How long a case may take before it fails.
const CASE_TIMEOUT: Duration = Duration::from_secs(60);

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(CASE_TIMEOUT, f)
        .await
        .expect("the case took too long")
}

/// A person no other case uses.
fn owner() -> UserId {
    UserId::new(&format!("{}@conformance.test", Uuid::now_v7()))
}

fn fresh_id() -> String {
    format!("m-{}", &Uuid::now_v7().simple().to_string()[20..])
}

fn at(second: i64) -> Timestamp {
    Timestamp::from_second(1_700_000_000 + second).expect("a time")
}

fn text(words: &str) -> SaveInput {
    SaveInput(words.to_owned())
}

/// Every note, as a read with no filter and the largest limit would return it.
fn everything() -> ReadInput {
    ReadInput {
        query: None,
        limit: MAX_READ_NOTES,
    }
}

fn query(words: &str) -> ReadInput {
    ReadInput {
        query: Some(words.to_owned()),
        limit: MAX_READ_NOTES,
    }
}

async fn save<S: MemoryNoteStore>(store: &S, owner: &UserId, words: &str) -> Note {
    store
        .save_note(owner, &text(words), "person", &fresh_id(), at(0))
        .await
        .expect("save")
}

async fn ids<S: MemoryNoteStore>(store: &S, owner: &UserId) -> Vec<String> {
    store
        .list_notes(owner)
        .await
        .expect("list")
        .into_iter()
        .map(|n| n.id)
        .collect()
}

/// A note comes back whole, with every field it was saved with, from every way of reading it: the
/// answer of the save, a listing and a read. The longest allowed text, a text with a newline and a
/// non-ASCII character, and both kinds of writer (an agent, the person) are kept byte for byte.
pub async fn a_note_round_trips_every_field<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let long = "a".repeat(MAX_NOTE_BYTES);
        let cases = [
            (
                fresh_id(),
                "I deploy with Helm\nsecond line, caf\u{e9}".to_owned(),
                "coder",
                at(7),
            ),
            (fresh_id(), long, "person", at(9)),
        ];
        for (id, words, writer, when) in &cases {
            let saved = store
                .save_note(&a, &text(words), writer, id, *when)
                .await
                .expect("save");
            let expected = Note {
                id: id.clone(),
                text: words.clone(),
                saved_at: when.to_string(),
                written_by: (*writer).to_owned(),
            };
            assert_eq!(saved, expected, "the answer of the save");
            let listed = store.list_notes(&a).await.expect("list");
            assert!(listed.contains(&expected), "the listing lacks the note");
            let read = store.read_notes(&a, &everything()).await.expect("read");
            assert!(read.notes.contains(&expected), "the read lacks the note");
        }
        let read = store.read_notes(&a, &everything()).await.expect("read");
        assert_eq!(read.total, 2);
        assert!(!read.truncated);
    })
    .await;
}

/// A person who never touched their settings has memory off: no agent has a grant, and the default
/// grants nothing.
pub async fn a_new_persons_settings_read_as_off<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let settings = store.memory_settings(&a).await.expect("settings");
        assert_eq!(settings, MemorySettings::default());
        assert_eq!(
            settings.default,
            Grant {
                read: false,
                write: false
            }
        );
        assert!(settings.agents.is_empty());
        assert_eq!(settings.grant("coder"), None);
        // reading settings creates nothing: a second read is the same, and no note appeared
        assert_eq!(
            store.memory_settings(&a).await.expect("settings"),
            MemorySettings::default()
        );
        let read = store.read_notes(&a, &everything()).await.expect("read");
        assert_eq!(
            (read.total, read.notes.len(), read.truncated),
            (0, 0, false)
        );
        assert!(store.list_notes(&a).await.expect("list").is_empty());
    })
    .await;
}

/// A grant is kept per agent and replaced by the next one of that agent; an agent never touched
/// stays off, and the default stays off.
pub async fn a_grant_round_trips_and_replaces<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let read_only = Grant {
            read: true,
            write: false,
        };
        let both = Grant {
            read: true,
            write: true,
        };
        store
            .set_memory_grant(&a, "coder", read_only)
            .await
            .expect("set");
        store
            .set_memory_grant(&a, "reviewer", both)
            .await
            .expect("set");
        let settings = store.memory_settings(&a).await.expect("settings");
        assert_eq!(settings.grant("coder"), Some(read_only));
        assert_eq!(settings.grant("reviewer"), Some(both));
        assert_eq!(settings.grant("planner"), None);
        assert_eq!(settings.default, Grant::default());
        assert_eq!(settings.agents.len(), 2);

        store
            .set_memory_grant(&a, "coder", Grant::default())
            .await
            .expect("set");
        let settings = store.memory_settings(&a).await.expect("settings");
        assert_eq!(settings.grant("coder"), Some(Grant::default()));
        assert_eq!(settings.grant("reviewer"), Some(both));
        assert_eq!(settings.agents.len(), 2, "a replace adds no entry");
    })
    .await;
}

/// **The decisive case**: one person never reads, writes over or deletes another's note, and never
/// counts it. There is no edit in the contract, so "edits" is the other write: a save under the id
/// another person holds, which neither fails because of it nor changes it. Settings are as private.
pub async fn another_person_never_reads_edits_or_deletes_a_note<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let held = fresh_id();
        let kept = store
            .save_note(&a, &text("a-secret-of-person-a"), "coder", &held, at(1))
            .await
            .expect("save");
        let another = save(&store, &a, "the second note of person a").await;

        // reads: the unfiltered read, the filtered read that would match, and the listing
        let read = store.read_notes(&b, &everything()).await.expect("read");
        assert_eq!((read.total, read.notes.len()), (0, 0), "b read a's notes");
        let read = store
            .read_notes(&b, &query("a-secret-of-person-a"))
            .await
            .expect("read");
        assert_eq!((read.total, read.notes.len()), (0, 0), "b found a's text");
        assert!(store.list_notes(&b).await.expect("list").is_empty());

        // deletes: by a's id, answering exactly what an id that never existed answers
        let theirs = store
            .forget_note(&b, &ForgetInput(held.clone()))
            .await
            .expect("forget");
        let nobodys = store
            .forget_note(&b, &ForgetInput(fresh_id()))
            .await
            .expect("forget");
        assert!(!theirs, "b deleted a's note");
        assert_eq!(theirs, nobodys, "the answer tells whose an id is");
        assert_eq!(ids(&store, &a).await.len(), 2);

        // writes: b saves under a's id; it is b's own note, and a's is unchanged
        let mine = store
            .save_note(&b, &text("what b wrote"), "person", &held, at(2))
            .await
            .expect("b may use an id a holds");
        assert_eq!(mine.text, "what b wrote");
        let a_read = store.read_notes(&a, &everything()).await.expect("read");
        assert_eq!(a_read.total, 2);
        assert!(a_read.notes.contains(&kept), "a's note changed");
        assert!(a_read.notes.contains(&another));
        assert!(
            !a_read.notes.iter().any(|n| n.text == "what b wrote"),
            "b's note reached a"
        );
        // and b's delete of that id removes b's, never a's
        assert!(
            store
                .forget_note(&b, &ForgetInput(held.clone()))
                .await
                .expect("forget")
        );
        assert!(ids(&store, &b).await.is_empty());
        let a_read = store.read_notes(&a, &everything()).await.expect("read");
        assert!(a_read.notes.contains(&kept), "b's delete reached a's note");

        // a's own delete still works
        assert!(
            store
                .forget_note(&a, &ForgetInput(held))
                .await
                .expect("forget")
        );
        assert_eq!(ids(&store, &a).await.len(), 1);
    })
    .await;
}

/// One person's grants are not another's: the settings of a person who touched nothing are off
/// whatever somebody else enabled.
pub async fn another_persons_grants_are_not_mine<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        store
            .set_memory_grant(
                &a,
                "coder",
                Grant {
                    read: true,
                    write: true,
                },
            )
            .await
            .expect("set");
        assert_eq!(
            store.memory_settings(&b).await.expect("settings"),
            MemorySettings::default()
        );
        // and b's change of the same agent leaves a's alone
        store
            .set_memory_grant(&b, "coder", Grant::default())
            .await
            .expect("set");
        let settings = store.memory_settings(&a).await.expect("settings");
        assert_eq!(
            settings.grant("coder"),
            Some(Grant {
                read: true,
                write: true
            })
        );
    })
    .await;
}

/// Notes come back newest first, whatever the instant they were saved at: the later of two notes
/// saved at the same second comes first. `total` counts the matches and `truncated` says when the
/// limit cut them.
pub async fn a_read_is_newest_first_and_cut_at_the_limit<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let mut saved = Vec::new();
        for n in 0..5 {
            // every note at the same instant: only the order of saving can tell them apart
            saved.push(save(&store, &a, &format!("note number {n}")).await.id);
        }
        let all = store.read_notes(&a, &everything()).await.expect("read");
        let order: Vec<&str> = all.notes.iter().map(|n| n.id.as_str()).collect();
        let newest_first: Vec<&str> = saved.iter().rev().map(String::as_str).collect();
        assert_eq!(order, newest_first);
        assert_eq!(all.total, 5);
        assert!(!all.truncated);

        let two = store
            .read_notes(
                &a,
                &ReadInput {
                    query: None,
                    limit: 2,
                },
            )
            .await
            .expect("read");
        let order: Vec<&str> = two.notes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(order, newest_first[..2]);
        assert_eq!(two.total, 5);
        assert!(two.truncated, "a short list must say it is short");

        // the listing is the same order
        let listed = ids(&store, &a).await;
        assert_eq!(
            listed.iter().map(String::as_str).collect::<Vec<_>>(),
            newest_first
        );
    })
    .await;
}

/// A limit of zero is one, and a limit over [`MAX_READ_NOTES`] is [`MAX_READ_NOTES`]; the total
/// is still every match.
pub async fn the_limit_is_clamped<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        for n in 0..MAX_READ_NOTES + 5 {
            save(&store, &a, &format!("note {n}")).await;
        }
        let one = store
            .read_notes(
                &a,
                &ReadInput {
                    query: None,
                    limit: 0,
                },
            )
            .await
            .expect("read");
        assert_eq!(one.notes.len(), 1);
        assert_eq!(one.total, MAX_READ_NOTES + 5);
        assert!(one.truncated);
        let most = store
            .read_notes(
                &a,
                &ReadInput {
                    query: None,
                    limit: 10_000,
                },
            )
            .await
            .expect("read");
        assert_eq!(most.notes.len(), MAX_READ_NOTES);
        assert_eq!(most.total, MAX_READ_NOTES + 5);
        assert!(most.truncated);
    })
    .await;
}

/// The filter is a case-insensitive substring of the text and nothing else: `%` and `_` are
/// characters, not wildcards, and a word that is not there matches nothing.
pub async fn a_query_is_a_case_insensitive_substring<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let helm = save(&store, &a, "I deploy with Helm").await;
        let percent = save(&store, &a, "the discount is 100% off").await;
        let xyz = save(&store, &a, "xyz").await;
        save(&store, &a, "the staging cluster is kestrel").await;

        let found = |read: orch_core::ReadResult| -> Vec<String> {
            read.notes.into_iter().map(|n| n.id).collect()
        };
        let by = |q: &'static str| {
            let (store, a) = (&store, &a);
            async move { found(store.read_notes(a, &query(q)).await.expect("read")) }
        };
        assert_eq!(by("HELM").await, vec![helm.id.clone()]);
        assert_eq!(by("deploy WITH").await, vec![helm.id.clone()]);
        assert_eq!(by("100%").await, vec![percent.id.clone()]);
        assert_eq!(by("%").await, vec![percent.id.clone()], "% is a character");
        assert!(by("x_z").await.is_empty(), "_ is not a wildcard");
        assert_eq!(by("xyz").await, vec![xyz.id.clone()]);
        assert!(by("no such word").await.is_empty());
        let none = store
            .read_notes(&a, &query("no such word"))
            .await
            .expect("read");
        assert_eq!((none.total, none.truncated), (0, false));
    })
    .await;
}

/// A delete answers `true` once and `false` after, and a note that was deleted is gone from every
/// read.
pub async fn forgetting_is_idempotent<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let note = save(&store, &a, "to be forgotten").await;
        let keep = save(&store, &a, "to be kept").await;
        let id = ForgetInput(note.id.clone());
        assert!(store.forget_note(&a, &id).await.expect("forget"));
        assert!(!store.forget_note(&a, &id).await.expect("forget"));
        assert!(
            !store
                .forget_note(&a, &ForgetInput(fresh_id()))
                .await
                .expect("forget")
        );
        assert_eq!(ids(&store, &a).await, vec![keep.id.clone()]);
        let read = store
            .read_notes(&a, &query("forgotten"))
            .await
            .expect("read");
        assert_eq!(read.total, 0);
    })
    .await;
}

/// A person who saves an id they already hold is refused, and nothing changes.
pub async fn an_id_the_person_holds_is_exists<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let first = store
            .save_note(&a, &text("the first"), "person", "m-same", at(1))
            .await
            .expect("save");
        let again = store
            .save_note(&a, &text("the second"), "coder", "m-same", at(2))
            .await;
        assert!(
            matches!(again, Err(MemoryNoteStoreError::Exists)),
            "{again:?}"
        );
        assert_eq!(store.list_notes(&a).await.expect("list"), vec![first]);
    })
    .await;
}

/// At most [`MAX_NOTES`] per person, **per person**: the 201st is refused and nothing is stored,
/// another person is not affected, and a forget makes room.
pub async fn at_most_two_hundred_per_person<S: MemoryNoteStore>(store: S) {
    within(async {
        let a = owner();
        let b = owner();
        let mut first = None;
        for n in 0..MAX_NOTES {
            let note = save(&store, &a, &format!("note {n}")).await;
            if first.is_none() {
                first = Some(note.id);
            }
        }
        let over = store
            .save_note(&a, &text("one too many"), "person", &fresh_id(), at(0))
            .await;
        match over {
            Err(MemoryNoteStoreError::TooMany { limit }) => assert_eq!(limit, MAX_NOTES),
            other => panic!("expected TooMany, got {other:?}"),
        }
        assert_eq!(ids(&store, &a).await.len(), MAX_NOTES);
        let read = store
            .read_notes(&a, &query("too many"))
            .await
            .expect("read");
        assert_eq!(read.total, 0, "the refused note was stored");

        // b is not a's count
        save(&store, &b, "b is not limited by a").await;
        assert_eq!(ids(&store, &b).await.len(), 1);

        // a forget makes room, for one
        let id = ForgetInput(first.expect("a first note"));
        assert!(store.forget_note(&a, &id).await.expect("forget"));
        save(&store, &a, "room again").await;
        assert!(matches!(
            store
                .save_note(&a, &text("full again"), "person", &fresh_id(), at(0))
                .await,
            Err(MemoryNoteStoreError::TooMany { .. })
        ));
    })
    .await;
}

/// Writers racing for the last places never pass the limit: of eight saves with two places free,
/// exactly two win.
pub async fn concurrent_saves_never_pass_the_limit<S: MemoryNoteStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let a = owner();
        for n in 0..MAX_NOTES - 2 {
            save(&*store, &a, &format!("note {n}")).await;
        }
        let mut writers = Vec::new();
        for n in 0..8 {
            let (store, a) = (Arc::clone(&store), a.clone());
            writers.push(tokio::spawn(async move {
                store
                    .save_note(
                        &a,
                        &text(&format!("race {n}")),
                        "person",
                        &fresh_id(),
                        at(0),
                    )
                    .await
            }));
        }
        let mut won = 0;
        for w in writers {
            match w.await.expect("task") {
                Ok(_) => won += 1,
                Err(MemoryNoteStoreError::TooMany { .. }) => {}
                Err(e) => panic!("unexpected: {e:?}"),
            }
        }
        assert_eq!(won, 2, "two places were free");
        assert_eq!(ids(&*store, &a).await.len(), MAX_NOTES);
    })
    .await;
}

/// Writers saving one id have one winner; the others find it taken.
pub async fn concurrent_saves_of_one_id_have_one_winner<S: MemoryNoteStore>(store: S) {
    within(async {
        let store = Arc::new(store);
        let a = owner();
        let id = fresh_id();
        let mut writers = Vec::new();
        for n in 0..8 {
            let (store, a, id) = (Arc::clone(&store), a.clone(), id.clone());
            writers.push(tokio::spawn(async move {
                store
                    .save_note(&a, &text(&format!("same {n}")), "person", &id, at(0))
                    .await
            }));
        }
        let mut won = 0;
        for w in writers {
            match w.await.expect("task") {
                Ok(_) => won += 1,
                Err(MemoryNoteStoreError::Exists) => {}
                Err(e) => panic!("unexpected: {e:?}"),
            }
        }
        assert_eq!(won, 1, "one writer wins the id");
        assert_eq!(ids(&*store, &a).await, vec![id]);
    })
    .await;
}

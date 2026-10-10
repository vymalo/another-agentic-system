//! The frames of every golden log, pinned by digest under a fixed `ThreadMeta`
//! (`tests/projection-digests.txt`, `docs/api/history.md` rule 7).
//!
//! A client that stores frames (ADR 0060) trusts that the frames written for an event already in a
//! log do not change while [`PROJECTION_VERSION`] stays the same. This is the guard: the first line
//! of the table is the version, every other line a golden and the SHA-256 of its frames, and a
//! change to a frame fails here. A new golden adds a line and changes nothing else. A change to an
//! existing line must raise the version in the same change (`tools/projection-digests-check.sh`
//! checks that against the base branch in CI).
//!
//! `UPDATE_DIGESTS=1 cargo test -p orch-agui-projection --test projection_digests` rewrites the
//! table; review the diff, and raise `PROJECTION_VERSION` when a line that was there changed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::fmt::Write as _;
use std::path::PathBuf;

use orch_agui_projection::{Audience, Frame, PROJECTION_VERSION, Projector};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use support::goldens::{SCENARIOS, load_events, meta_of};

fn table_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/projection-digests.txt")
}

/// The digest of a stream: one line per frame, `{"id"?,"event"}` as the history route writes it.
fn digest(frames: &[Frame]) -> String {
    let mut hash = Sha256::new();
    for frame in frames {
        let mut item = serde_json::Map::new();
        if let Some(id) = frame.resume_id {
            item.insert("id".to_owned(), json!(id));
        }
        item.insert(
            "event".to_owned(),
            serde_json::to_value(&frame.event).unwrap(),
        );
        hash.update(serde_json::to_string(&Value::Object(item)).unwrap());
        hash.update(b"\n");
    }
    let mut out = String::new();
    for byte in hash.finalize() {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

fn frames_of(name: &str, title: Option<&str>) -> Vec<Frame> {
    let events = load_events(name);
    let mut meta = meta_of(name, &events);
    if let Some(title) = title {
        title.clone_into(&mut meta.title);
    }
    let mut projector = Projector::new(meta);
    events
        .iter()
        .flat_map(|e| projector.apply(e, Audience::Viewer))
        .collect()
}

fn table() -> String {
    let mut text = format!("projection {PROJECTION_VERSION}\n");
    for name in SCENARIOS {
        writeln!(text, "{name} {}", digest(&frames_of(name, None))).unwrap();
    }
    text
}

#[test]
fn the_digest_table_matches_the_frames_of_every_golden() {
    let want = table();
    if std::env::var("UPDATE_DIGESTS").is_ok_and(|v| v == "1") {
        std::fs::write(table_path(), &want).unwrap();
        return;
    }
    let got = std::fs::read_to_string(table_path()).unwrap_or_default();
    assert_eq!(
        got, want,
        "the frames of a golden changed (or a golden was added): if the change is intended, run \
         `UPDATE_DIGESTS=1 cargo test -p orch-agui-projection --test projection_digests`, review \
         the diff, and raise PROJECTION_VERSION when a line that was already there changed"
    );
}

/// Why the table is pinned over a fixed `ThreadMeta`: the snapshots before the first title or
/// description event say the thread's *current* title, so the same log folded under another title
/// writes other frames. A client therefore stores what a page returned and takes the title from
/// the resource (rule 2).
#[test]
fn the_meta_is_part_of_the_frames_before_the_first_title_event() {
    let name = "echo";
    let a = digest(&frames_of(name, Some("one")));
    let b = digest(&frames_of(name, Some("another")));
    assert_ne!(
        a, b,
        "the title is in the snapshots of a log that never renames the thread"
    );
    assert_eq!(
        a,
        digest(&frames_of(name, Some("one"))),
        "and it is a function of the meta"
    );
}

//! The carry of a page (`docs/api/history.md`, "Carry"): the page's carry and the frames of the pages
//! the reader holds give the state a replay of the whole thread gives. Over every golden and over
//! generated logs (usage of tasks, steps and asked agents, totals, files), for every page of every
//! walk, with and without a byte cap that cuts inside a turn.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_projection::{Frame, Page, Window};
use orch_agui_proto as agui;
use orch_core::Event;
use proptest::prelude::*;
use serde_json::{Value, json};
use support::goldens::{SCENARIOS, load_events, meta_of};
use support::log::{Action, arb_actions, world};
use support::pages::{limits, read, walk};
use support::usage_ref::Usage;

/// The frames of the pages from `pages[k]` (the oldest held) to `pages[0]`, in log order.
fn frames_from(pages: &[Page], k: usize) -> Vec<&Frame> {
    pages[..=k]
        .iter()
        .rev()
        .flat_map(|p| p.frames.iter())
        .collect()
}

/// The usage state of a reader that holds `pages[..=k]` and the carry of `pages[k]`.
fn held_usage(pages: &[Page], k: usize) -> Usage {
    let mut state = Usage::from_carry(pages[k].carry.as_ref().and_then(|c| c.usage.as_ref()));
    fold_frames(&mut state, frames_from(pages, k));
    state
}

fn fold_frames<'a>(state: &mut Usage, frames: impl IntoIterator<Item = &'a Frame>) {
    for frame in frames {
        if let agui::Event::Custom(e) = &frame.event {
            state.fold(&e.name, &e.value);
        }
    }
}

/// The kept files a frame says, by hash.
fn file_hash(frame: &Frame) -> Option<String> {
    let agui::Event::ActivitySnapshot(e) = &frame.event else {
        return None;
    };
    (e.activity_type == "vymalo.artifact"
        && e.content.get("kind").and_then(Value::as_str) == Some("file"))
    .then(|| {
        e.content
            .get("sha256")
            .and_then(Value::as_str)
            .map(str::to_owned)
    })
    .flatten()
}

/// Runs that ended having drawn something of the agent's, by the frames alone (the rule of `carry.rs`, written again).
fn turns_in<'a>(frames: impl IntoIterator<Item = &'a Frame>) -> u64 {
    let (mut turns, mut output) = (0, false);
    for frame in frames {
        match &frame.event {
            agui::Event::RunStarted(_) => output = false,
            agui::Event::RunFinished(_) | agui::Event::RunError(_) => {
                turns += u64::from(std::mem::take(&mut output));
            }
            agui::Event::TextMessageStart(e) => {
                output |= e.role != Some(agui::TextMessageRole::User);
            }
            agui::Event::ReasoningStart(_) | agui::Event::ReasoningMessageStart(_) => output = true,
            agui::Event::ActivitySnapshot(e) => {
                let status = e.content.get("status").and_then(Value::as_str);
                output |= match e.activity_type.as_str() {
                    "vymalo.status" => {
                        status.is_some_and(|s| s != "completed" && s != "input_required")
                    }
                    "vymalo.artifact" | "vymalo.step" | "vymalo.ask" | "vymalo.check"
                    | "vymalo.ci" | "vymalo.rework" | "vymalo.action" | "vymalo.error"
                    | "a2ui-surface" => true,
                    _ => false,
                };
            }
            _ => {}
        }
    }
    turns
}

/// The pages of a walk as the surface writes them, newest first: what the web's tests read.
fn walk_json(events: &[Event], meta: &orch_agui_projection::ThreadMeta, turns: usize) -> Value {
    let pages = walk(events, meta, turns, limits(100, 4 << 20));
    let pages: Vec<Value> = pages
        .iter()
        .map(|p| {
            let frames: Vec<Value> = p
                .frames
                .iter()
                .map(|f| {
                    let event = serde_json::to_value(&f.event).unwrap();
                    match f.resume_id {
                        Some(id) => json!({"id": id, "event": event}),
                        None => json!({"event": event}),
                    }
                })
                .collect();
            let mut body = json!({
                "start": p.start,
                "end": p.end,
                "head": p.head,
                "earlier": p.earlier,
                "projection": orch_agui_projection::PROJECTION_VERSION,
                "frames": frames,
            });
            if let Some(carry) = &p.carry {
                body["carry"] = carry.to_value();
            }
            body
        })
        .collect();
    json!({ "pages": pages })
}

/// Writes (`UPDATE_GOLDEN=1`) or checks a fixture of `docs/api/examples/history`.
fn pinned(name: &str, value: &Value, stale: &mut Vec<String>) {
    let update = std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let dir = support::goldens::examples_dir().join("history");
    let mut text = serde_json::to_string_pretty(value).unwrap();
    text.push('\n');
    let path = dir.join(name);
    if update {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, &text).unwrap();
    } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        stale.push(path.display().to_string());
    }
}

/// Every claim of the carry, for every page of one walk.
fn assert_carry(events: &[Event], pages: &[Page], what: &str) {
    let whole = pages.len() - 1;
    let mut all = Usage::default();
    fold_frames(&mut all, frames_from(pages, whole));
    let wanted = all.summarize();
    let all_files: Vec<String> = frames_from(pages, whole)
        .into_iter()
        .filter_map(file_hash)
        .collect::<Vec<_>>();
    let all_turns = turns_in(frames_from(pages, whole));
    for (k, page) in pages.iter().enumerate() {
        // the oldest page has nothing before it
        assert_eq!(page.carry.is_some(), page.earlier, "{what}: page {k}");
        let Some(carry) = &page.carry else { continue };

        // the token totals, groups and latest call: the state of a reader that holds these pages
        assert_eq!(
            held_usage(pages, k).summarize(),
            wanted,
            "{what}: the usage of a reader of {k} pages back differs from the whole thread's ({} events)",
            events.len()
        );

        // the files: the carry's, then the held pages', are every file of the thread, once each
        let held: Vec<String> = frames_from(pages, k)
            .into_iter()
            .filter_map(file_hash)
            .collect();
        let mut union: Vec<String> = carry
            .files
            .iter()
            .filter_map(|f| f.get("sha256").and_then(Value::as_str).map(str::to_owned))
            .collect();
        for hash in held {
            if !union.contains(&hash) {
                union.push(hash);
            }
        }
        let mut once: Vec<String> = Vec::new();
        for hash in &all_files {
            if !once.contains(hash) {
                once.push(hash.clone());
            }
        }
        assert_eq!(union, once, "{what}: the files of a reader of {k} pages");

        // the turns before the page, and the turns of the pages held, are the thread's
        assert_eq!(
            carry.turns + turns_in(frames_from(pages, k)),
            all_turns,
            "{what}: the turns of a reader of {k} pages"
        );
    }
}

#[test]
fn the_carry_and_the_pages_held_give_the_state_of_the_whole_thread_on_every_golden() {
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        for turns in [1, 2, 3] {
            for cap in [4 << 20, 1] {
                let pages = walk(&events, &meta, turns, limits(100, cap));
                assert_carry(
                    &events,
                    &pages,
                    &format!("{name} at {turns} turns, cap {cap}"),
                );
            }
        }
    }
}

#[test]
fn a_catch_up_carries_nothing() {
    let events = load_events("followup");
    let meta = meta_of("followup", &events);
    let page = read(
        &events,
        &meta,
        Window::After { after: 3 },
        limits(100, 4 << 20),
    );
    assert!(page.carry.is_none());
}

#[test]
fn the_page_that_reaches_the_first_event_has_no_carry() {
    let events = load_events("followup");
    let meta = meta_of("followup", &events);
    let page = read(
        &events,
        &meta,
        Window::Turns {
            before: None,
            turns: 100,
        },
        limits(100, 4 << 20),
    );
    assert_eq!(page.start, 1);
    assert!(page.carry.is_none());
}

/// Six turns that each spent tokens, in two tasks (a task is resumed by a later turn), by the agent and
/// under a step, with the totals of a task said at the end of a turn.
fn usage_turns() -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let mut actions = Vec::new();
    for n in 0..6u8 {
        actions.push(Action::User {
            text: format!("turn {n}"),
            ids: false,
        });
        for (step, total) in [(None, false), (Some(n), false), (None, true)] {
            actions.push(Action::Usage {
                task: n,
                step,
                total,
            });
        }
        actions.push(Action::Status(orch_core::AgentTaskState::Completed, None));
    }
    world(0, &actions)
}

/// A thread that spent tokens in many turns, so the carry of the newest page is the sum of what it
/// leaves out: the tokens, the groups and the latest call all come from before the page.
#[test]
fn a_page_of_one_turn_carries_the_tokens_of_the_turns_before_it() {
    let (events, meta) = usage_turns();
    let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
    assert!(pages.len() >= 6, "{} pages", pages.len());
    assert_carry(&events, &pages, "six turns of usage");
    let usage = pages[0]
        .carry
        .as_ref()
        .and_then(|c| c.usage.as_ref())
        .expect("the newest page leaves tokens out");
    assert!(usage["tasks"].as_array().is_some_and(|t| !t.is_empty()));
    assert!(usage["groups"].as_array().is_some_and(|t| !t.is_empty()));
    assert!(usage.get("latest").is_some());
}

/// The `file` golden's turn repeated `turns` times, each with a file of its own (`n % distinct`
/// distinct hashes): a thread whose every turn handed over a kept file.
fn file_turns(turns: usize, distinct: usize) -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let base = load_events("file");
    let per = i64::try_from(base.len()).unwrap();
    let mut events = Vec::new();
    for t in 0..turns {
        for e in &base {
            let mut e = e.clone();
            e.seq += per * i64::try_from(t).unwrap();
            if let orch_core::EventBody::Artifact(a) = &mut e.body
                && let Some(file) = &mut a.file
            {
                file.sha256 = format!("{:064x}", t % distinct);
            }
            events.push(e);
        }
    }
    let meta = meta_of("file", &events);
    (events, meta)
}

#[test]
fn the_files_before_a_page_are_carried_once_each_in_the_order_they_came() {
    let (events, meta) = file_turns(5, 3);
    let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
    assert_eq!(pages.len(), 5);
    assert_carry(&events, &pages, "files");
    let carried: Vec<String> = pages[0]
        .carry
        .as_ref()
        .unwrap()
        .files
        .iter()
        .map(|f| f["sha256"].as_str().unwrap().to_owned())
        .collect();
    // turns 0..=3 left hashes 0, 1, 2 (the fourth turn repeats the first); the newest page is turn 4
    assert_eq!(
        carried,
        (0..3).map(|n| format!("{n:064x}")).collect::<Vec<_>>()
    );
    // what a viewer needs to draw it: the route, the size and the preview kind
    let first = &pages[0].carry.as_ref().unwrap().files[0];
    assert!(first["href"].as_str().unwrap().contains("/artifacts/"));
    assert_eq!(first["kind"], "file");
}

#[test]
fn the_carry_names_at_most_five_hundred_files_the_newest() {
    let (events, meta) = file_turns(510, 1000);
    let page = read(
        &events,
        &meta,
        Window::Turns {
            before: None,
            turns: 1,
        },
        limits(100, 4 << 20),
    );
    let carried: Vec<String> = page
        .carry
        .unwrap()
        .files
        .iter()
        .map(|f| f["sha256"].as_str().unwrap().to_owned())
        .collect();
    // 509 turns came before the page: the newest 500 of their files, 9 to 508
    assert_eq!(
        carried,
        (9..509).map(|n| format!("{n:064x}")).collect::<Vec<_>>()
    );
}

/// The pages of two logs as the surface writes them, for the web's tests of the same invariants with the web's own
/// folds (`web/src/features/chat/lib/agui/history-carry.test.ts`). `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection
/// --test carry` rewrites them.
#[test]
fn the_pages_of_the_carry_fixtures_are_pinned() {
    let mut stale = Vec::new();
    let (events, meta) = usage_turns();
    pinned(
        "usage-turns.walk.json",
        &walk_json(&events, &meta, 1),
        &mut stale,
    );
    let (events, meta) = file_turns(4, 3);
    pinned(
        "file-turns.walk.json",
        &walk_json(&events, &meta, 1),
        &mut stale,
    );
    assert!(
        stale.is_empty(),
        "the carry fixtures are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test carry`: {stale:?}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Over generated logs: the carry of every page of every walk, with and without a small byte cap.
    #[test]
    fn the_carry_and_the_pages_held_give_the_state_of_the_whole_thread_on_generated_logs(
        actions in arb_actions(),
        gating in 0_u8..3,
    ) {
        let (events, meta) = world(gating, &actions);
        for turns in [1usize, 2, 7] {
            for cap in [4usize << 20, 3000] {
                let pages = walk(&events, &meta, turns, limits(100, cap));
                assert_carry(&events, &pages, &format!("generated at {turns} turns, cap {cap}"));
            }
        }
        // every page names the files by hash once, and at most 500
        let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
        for page in &pages {
            if let Some(carry) = &page.carry {
                let hashes: BTreeSet<_> = carry
                    .files
                    .iter()
                    .filter_map(|f| f.get("sha256").and_then(Value::as_str))
                    .collect();
                prop_assert_eq!(hashes.len(), carry.files.len());
                prop_assert!(carry.files.len() <= 500);
            }
        }
    }
}

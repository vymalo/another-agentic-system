//! The carry of a page (`docs/api/history.md`, "Carry"): the page's carry and the frames of the pages
//! the reader holds give the state a replay of the whole thread gives. Over every golden and over
//! generated logs (usage of tasks, steps and asked agents, totals, files), for every page of every
//! walk, with and without a byte cap that cuts inside a turn.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use orch_agui_projection::{Frame, Page, Window};
use orch_agui_proto as agui;
use orch_core::{AgentTaskState, Event};
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

/// `usage_turns` with a call said again: the first call of the second turn right after itself, and, with `across`, the
/// very first call of the thread once more in the fourth turn (a task that is resumed reports what it already reported).
fn usage_turns_with_repeats(across: bool) -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let (events, meta) = usage_turns();
    let calls: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.body, orch_core::EventBody::ModelUsage(_)))
        .map(|(i, _)| i)
        .collect();
    let users: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.body, orch_core::EventBody::UserMessage(_)))
        .map(|(i, _)| i)
        .collect();
    let after = |user: usize| *calls.iter().find(|i| **i > users[user]).unwrap();
    let (twice, again) = (after(1), after(3));
    let mut out = Vec::new();
    for (i, event) in events.iter().enumerate() {
        out.push(event.clone());
        if i == twice {
            out.push(event.clone());
        }
        if across && i == again {
            out.push(events[calls[0]].clone());
        }
    }
    for (n, event) in out.iter_mut().enumerate() {
        event.seq = i64::try_from(n).unwrap() + 1;
    }
    (out, meta)
}

/// The usage a reader of the whole thread shows.
fn whole_usage(events: &[Event], meta: &orch_agui_projection::ThreadMeta) -> Usage {
    let pages = walk(events, meta, 1000, limits(100, 4 << 20));
    let mut state = Usage::default();
    fold_frames(&mut state, frames_from(&pages, pages.len() - 1));
    state
}

#[test]
fn a_call_said_twice_in_a_turn_is_counted_once_by_the_carry_and_by_the_pages_held() {
    let (events, meta) = usage_turns_with_repeats(false);
    let (plain, _) = usage_turns();
    assert_eq!(events.len(), plain.len() + 1);
    for turns in [1, 2, 3] {
        let pages = walk(&events, &meta, turns, limits(100, 4 << 20));
        assert_carry(
            &events,
            &pages,
            &format!("a repeated call at {turns} turns"),
        );
    }
    // the thread spent what it did without the repeat: the same totals and the same number of calls
    assert_eq!(
        whole_usage(&events, &meta).summarize(),
        whole_usage(&plain, &meta).summarize()
    );
}

/// The one thing a carry cannot do: it is a summary and names no call, so a call that a page held says again after the
/// carry has counted it is counted twice by that reader, where a replay counts it once. Agents say a call once per task
/// (`usage/v1`); the reader that holds a repeat's page and the carry that covers the first sight is the only one off, by
/// that call, and a reader of the whole thread, or of pages from before the first sight, is exact.
#[test]
fn a_call_said_again_after_the_carry_counted_it_is_counted_twice_by_that_reader_alone() {
    let (events, meta) = usage_turns_with_repeats(true);
    let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
    let wanted = whole_usage(&events, &meta).summarize();
    let reader = |k: usize| held_usage(&pages, k).summarize();
    // the reader of every page holds the thread: exact
    assert_eq!(reader(pages.len() - 1), wanted);
    // the readers that hold the repeat's page and the carry that covers the first sight are one call over, and no other is
    let off: Vec<usize> = (0..pages.len()).filter(|k| reader(*k) != wanted).collect();
    assert!(
        !off.is_empty(),
        "the repeat is invisible: the case tests nothing"
    );
    for k in off {
        let (got, want) = (reader(k), &wanted);
        assert_eq!(got.groups[0].1, want.groups[0].1 + 1, "reader of {k} pages");
        assert_eq!(got.latest, want.latest);
        assert_eq!(
            got.models, want.models,
            "the totals of a task are the task's own"
        );
    }
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

/// A turn: a person's message, the agent working, and the `middle` actions, then `end`.
fn turn(n: usize, middle: Vec<Action>, end: Action) -> Vec<Action> {
    let mut actions = vec![
        Action::User {
            text: format!("turn {n}"),
            ids: true,
        },
        Action::Status(AgentTaskState::Working, None),
    ];
    actions.extend(middle);
    actions.push(end);
    actions
}

fn say(slot: u8, text: &str, fin: bool) -> Action {
    Action::Say {
        slot,
        more: text.to_owned(),
        fin,
    }
}

fn done() -> Action {
    Action::Status(AgentTaskState::Completed, None)
}

/// A surface `s0` that is made in the first turn, updated in the next two and taken down in the fifth,
/// beside one (`s1`) that lives in the fourth alone: a surface that spans turns.
fn surface_turns() -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let surface = |surface, op| Action::Surface { surface, op };
    let mut actions = Vec::new();
    actions.extend(turn(1, vec![surface(0, 0), say(0, "a card", true)], done()));
    actions.extend(turn(2, vec![surface(0, 1), surface(0, 2)], done()));
    actions.extend(turn(
        3,
        vec![surface(0, 2), say(0, "updated", true)],
        done(),
    ));
    actions.extend(turn(4, vec![surface(1, 0), surface(0, 1)], done()));
    actions.extend(turn(5, vec![surface(0, 3), surface(1, 2)], done()));
    world(0, &actions)
}

/// A message sent while the agent works (a steer: the run goes on in the same chain, with two messages of the
/// person's), each followed by a turn of its own, and a steer of a steer.
fn steer_turns() -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let steer = |text: &str| Action::User {
        text: text.to_owned(),
        ids: true,
    };
    let mut actions = Vec::new();
    actions.extend(turn(
        1,
        vec![
            say(0, "starting", false),
            steer("and this too"),
            say(0, " and this", true),
        ],
        done(),
    ));
    actions.extend(turn(2, vec![say(1, "a follow-up", true)], done()));
    actions.extend(turn(
        3,
        vec![
            say(1, "working on three", false),
            steer("also this"),
            say(1, " with this", false),
            steer("and then that"),
            say(1, " and that", true),
        ],
        done(),
    ));
    actions.extend(turn(4, vec![say(0, "the follow-up of it", true)], done()));
    actions.extend(turn(5, vec![say(0, "and one more", true)], done()));
    world(0, &actions)
}

/// Questions: the first turn ends on one and the next turn answers it; a later one is answered by a turn that asks
/// again, and the newest ends on one nobody has answered.
fn form_turns() -> (Vec<Event>, orch_agui_projection::ThreadMeta) {
    let ask = |what: &str| Action::Status(AgentTaskState::InputRequired, Some(what.to_owned()));
    let mut actions = Vec::new();
    actions.extend(turn(
        1,
        vec![say(0, "which branch?", true)],
        ask("which branch?"),
    ));
    actions.extend(turn(2, vec![say(0, "main it is", true)], done()));
    actions.extend(turn(
        3,
        vec![say(1, "which file?", true)],
        ask("which file?"),
    ));
    actions.extend(turn(
        4,
        vec![say(1, "and which line?", true)],
        ask("which line?"),
    ));
    actions.extend(turn(
        5,
        vec![say(0, "which commit?", true)],
        ask("which commit?"),
    ));
    world(0, &actions)
}

#[test]
fn the_join_fixtures_are_threads_of_several_pages() {
    for (name, (events, meta)) in [
        ("surface", surface_turns()),
        ("steer", steer_turns()),
        ("form", form_turns()),
    ] {
        let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
        assert!(pages.len() >= 4, "{name}: {} pages", pages.len());
        assert_carry(&events, &pages, name);
    }
    // a steer is one chain with more than one message of the person's
    let (events, meta) = steer_turns();
    let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
    let users = |p: &Page| {
        p.frames
            .iter()
            .filter(|f| {
                matches!(&f.event, agui::Event::TextMessageStart(e)
                    if e.role == Some(agui::TextMessageRole::User))
            })
            .count()
    };
    // newest first: a follow-up, a follow-up, a turn steered twice, a follow-up, a turn steered once
    assert_eq!(pages.iter().map(users).collect::<Vec<_>>(), [1, 1, 3, 1, 2]);
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
    for (name, (events, meta)) in [
        ("surface-turns.walk.json", surface_turns()),
        ("steer-turns.walk.json", steer_turns()),
        ("form-turns.walk.json", form_turns()),
    ] {
        pinned(name, &walk_json(&events, &meta, 1), &mut stale);
    }
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

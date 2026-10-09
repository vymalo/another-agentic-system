//! The history fold (`docs/api/history.md`): pages tile a replay, a page is a whole number of
//! settled chains, the cut falls at chain starts only, and the ids a page repeats are replacing
//! snapshots. Over every golden and over generated logs, for every `limit` and with the byte cap
//! small enough to cut inside a turn.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeMap;

use orch_agui_projection::{
    Anchor, Audience, Frame, HistoryLimits, Page, Projector, ThreadMeta, Window,
};
use orch_agui_proto as agui;
use orch_core::{Event, EventBody};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::goldens::{SCENARIOS, load_events, meta_of};
use support::log::{arb_actions, world};
use support::pages::{head, limits, read, walk};
use support::{flatten, project_each_with};

/// The frames of a replay (no live text), event by event, up to and including event `end`.
fn replay_to(events: &[Event], meta: &ThreadMeta, end: i64) -> Vec<Frame> {
    let kept: Vec<Event> = events.iter().filter(|e| e.seq <= end).cloned().collect();
    flatten(&project_each_with(&kept, meta.clone()))
}

/// The ids a page says that a page before it said first, with the kind of frame that said them
/// again and whether it replaces what was there.
fn repeated(pages: &[Page]) -> Vec<(String, String, bool)> {
    fn ids(frame: &Frame) -> Option<(&'static str, String, bool)> {
        match &frame.event {
            agui::Event::TextMessageStart(e) => Some(("message", e.message_id.to_string(), false)),
            agui::Event::ReasoningStart(e) => Some(("reasoning", e.message_id.to_string(), false)),
            agui::Event::ActivitySnapshot(e) => Some((
                "activity",
                e.message_id.to_string(),
                e.replace.unwrap_or(true),
            )),
            _ => None,
        }
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = Vec::new();
    for (index, page) in pages.iter().rev().enumerate() {
        let mut said_here = BTreeMap::new();
        for frame in &page.frames {
            if let Some((kind, id, replaces)) = ids(frame) {
                if seen.get(&id).is_some_and(|first| *first < index) {
                    out.push((kind.to_owned(), id.clone(), replaces));
                }
                said_here.insert(id, index);
            }
        }
        for (id, at) in said_here {
            seen.entry(id).or_insert(at);
        }
    }
    out
}

/// Pages tile the replay, are contiguous and end at a settled point.
fn assert_tiles(events: &[Event], meta: &ThreadMeta, pages: &[Page]) {
    let newest = &pages[0];
    let got: Vec<Frame> = pages.iter().rev().flat_map(|p| p.frames.clone()).collect();
    assert_eq!(
        got,
        replay_to(events, meta, newest.end),
        "the pages do not tile the replay up to {}",
        newest.end
    );
    for pair in pages.windows(2) {
        assert_eq!(
            pair[1].end + 1,
            pair[0].start,
            "a gap or an overlap between pages: {:?} then {:?}",
            (pair[1].start, pair[1].end),
            (pair[0].start, pair[0].end)
        );
    }
    let oldest = pages.last().unwrap();
    assert!(!oldest.earlier && oldest.start == 1, "{oldest:?}");
    for page in pages {
        assert_eq!(page.earlier, page.start > 1, "{page:?}");
        assert_eq!(page.head, head(events));
        assert!(page.end <= page.head);
        assert!(page_events(page) >= 0, "{page:?}");
    }
}

/// How many events a page accounts for.
fn page_events(page: &Page) -> i64 {
    page.end - page.start + 1
}

/// Whether a run is open after the last frame of `frames`, by the frames alone.
fn run_open_after(frames: &[Frame]) -> bool {
    let mut open = false;
    for frame in frames {
        match frame.event {
            agui::Event::RunStarted(_) => open = true,
            agui::Event::RunFinished(_) | agui::Event::RunError(_) => open = false,
            _ => {}
        }
    }
    open
}

/// A page is whole runs: no run is open at its end, and its first frame opens one.
fn assert_settled(pages: &[Page]) {
    for page in pages {
        if page.frames.is_empty() {
            continue;
        }
        assert!(
            !run_open_after(&page.frames),
            "a run is open at {}",
            page.end
        );
        assert!(
            matches!(page.frames[0].event, agui::Event::RunStarted(_)),
            "a page must open with RUN_STARTED: {:?}",
            page.frames[0]
        );
    }
}

#[test]
fn pages_tile_the_replay_of_every_golden() {
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        for turns in [1, 2, 3, 5, 100] {
            let pages = walk(&events, &meta, turns, limits(100, 4 << 20));
            assert_tiles(&events, &meta, &pages);
            assert_settled(&pages);
        }
    }
}

#[test]
fn a_byte_cap_smaller_than_a_chain_cuts_between_chains_and_still_tiles() {
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        // every page is one chain at most, and the newest chain is returned whole
        let pages = walk(&events, &meta, 100, limits(100, 1));
        assert_tiles(&events, &meta, &pages);
        assert_settled(&pages);
    }
}

#[test]
fn the_newest_page_ends_at_the_end_of_a_settled_log() {
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        let page = read(
            &events,
            &meta,
            Window::Turns {
                before: None,
                turns: 100,
            },
            HistoryLimits::default(),
        );
        // every golden ends outside a run, so the whole log is one page
        assert_eq!((page.start, page.end), (1, head(&events)), "{name}");
        assert!(!page.earlier, "{name}");
        assert_eq!(
            page.frames,
            replay_to(&events, &meta, head(&events)),
            "{name}"
        );
    }
}

#[test]
fn the_ids_a_page_repeats_are_replacing_snapshots() {
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        for turns in [1, 2] {
            let pages = walk(&events, &meta, turns, limits(100, 4 << 20));
            let bad: Vec<_> = repeated(&pages)
                .into_iter()
                .filter(|(kind, _, replaces)| kind != "activity" || !replaces)
                .collect();
            assert!(bad.is_empty(), "{name} at {turns}: {bad:?}");
        }
    }
}

/// An A2UI surface is snapshotted again, whole, by a later event that touches it, in a later turn
/// of the same job too: the one id a page repeats on purpose. The goldens do not cover it (the
/// surface of `a2ui` is in one turn), so this builds the log: a surface and a question, the
/// answer, and an update of the surface in the turn that follows.
#[test]
fn a_surface_updated_in_a_later_turn_is_said_again_as_a_replacing_snapshot() {
    use support::log::Action;
    let actions = vec![
        Action::User {
            text: "one".to_owned(),
            ids: false,
        },
        Action::Surface { surface: 0, op: 0 },
        Action::Surface { surface: 0, op: 1 },
        Action::Status(orch_core::AgentTaskState::InputRequired, None),
        Action::User {
            text: "two".to_owned(),
            ids: false,
        },
        Action::Surface { surface: 0, op: 2 },
        Action::Status(orch_core::AgentTaskState::Completed, None),
    ];
    let (events, meta) = world(0, &actions);
    let pages = walk(&events, &meta, 1, limits(100, 4 << 20));
    assert!(
        pages.len() >= 2,
        "the log has two turns: {} pages",
        pages.len()
    );
    assert_tiles(&events, &meta, &pages);
    let again = repeated(&pages);
    assert!(
        again
            .iter()
            .any(|(kind, id, replaces)| kind == "activity" && id.starts_with("a2ui-") && *replaces),
        "the surface should be said again: {again:?}"
    );
    assert!(
        again
            .iter()
            .all(|(kind, _, replaces)| kind == "activity" && *replaces),
        "{again:?}"
    );
}

/// A thread nobody typed in (a webhook or CI driven one): every rename is a chain of its own and no
/// chain is a turn, so only the chain cap and the byte cap bound a page; pages still tile.
#[test]
fn a_thread_with_no_persons_message_is_bounded_by_its_chains() {
    use support::log::Action;
    // the agent finishes with nobody having spoken, then the thread is renamed again and again:
    // each rename is a run of its own, opened and closed by one event
    let mut actions = vec![Action::Status(orch_core::AgentTaskState::Completed, None)];
    actions.extend((0..1500u32).map(|n| Action::Rename { n: (n % 4) as u8 }));
    let (events, meta) = world(0, &actions);
    assert!(events.len() >= 1000, "{} events", events.len());
    let pages = walk(&events, &meta, 20, limits(100, 4 << 20));
    assert!(
        pages.len() >= 2,
        "a page may not hold the whole of it: {} pages",
        pages.len()
    );
    assert_tiles(&events, &meta, &pages);
    assert_settled(&pages);
    let widest = pages.iter().map(|p| p.end - p.start + 1).max().unwrap();
    assert!(
        widest <= 512,
        "{widest} events in a page of at most 512 chains"
    );
}

fn user_chain_starts(events: &[Event]) -> Vec<i64> {
    events
        .iter()
        .filter(|e| matches!(e.body, EventBody::UserMessage(_)))
        .map(|e| e.seq)
        .collect()
}

#[test]
fn a_steered_chain_is_kept_whole() {
    // `steer` and `stop-and-send` close the open run and open another in one event: the chain goes
    // on, so no page may start at the steering message.
    for name in ["steer", "stop-and-send"] {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        let steering: Vec<i64> = events
            .iter()
            .filter(|e| matches!(&e.body, EventBody::UserMessage(m) if m.delivery.is_some()))
            .map(|e| e.seq)
            .collect();
        assert!(!steering.is_empty(), "{name} has no steering message");
        for turns in 1..=4 {
            let pages = walk(&events, &meta, turns, limits(100, 1));
            assert_tiles(&events, &meta, &pages);
            for page in &pages {
                for seq in &steering {
                    assert_ne!(
                        page.start, *seq,
                        "{name}: a page starts at the steering message {seq}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_chain_still_open_is_not_in_the_page_and_the_stream_says_it() {
    // cut every golden before its last event: wherever a run is open, the page stops before it
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        for upto in 1..=events.len() {
            let log = &events[..upto];
            let page = read(
                log,
                &meta,
                Window::Turns {
                    before: None,
                    turns: 100,
                },
                HistoryLimits::default(),
            );
            let mut projector = Projector::new(meta.clone());
            let mut open_at: Vec<bool> = Vec::new();
            for event in log {
                projector.apply(event, Audience::Viewer);
                open_at.push(projector.run_open());
            }
            // `end` is a settled point: no run is open after it
            if page.end > 0 {
                assert!(!open_at[(page.end - 1) as usize], "{name} {upto}: {page:?}");
            }
            assert_settled(std::slice::from_ref(&page));
            assert_eq!(
                page.frames,
                replay_to(log, &meta, page.end),
                "{name} {upto}"
            );
            // a settled log is the whole page
            if !open_at[upto - 1] {
                assert_eq!(page.end, upto as i64, "{name} {upto}");
            } else {
                assert!(page.end < upto as i64, "{name} {upto}");
            }
        }
    }
}

#[test]
fn a_thread_whose_only_chain_is_open_is_an_empty_page() {
    let events = load_events("echo");
    let meta = meta_of("echo", &events);
    // the user's message and the first status: the run is open
    let open = &events[..2];
    let page = read(
        open,
        &meta,
        Window::Turns {
            before: None,
            turns: 20,
        },
        HistoryLimits::default(),
    );
    assert_eq!((page.start, page.end, page.earlier), (1, 0, false));
    assert!(page.frames.is_empty());
    assert_eq!(page.head, 2);
}

#[test]
fn the_boundaries() {
    let events = load_events("followup");
    let meta = meta_of("followup", &events);
    let all = HistoryLimits::default();
    let turn = |before, turns| read(&events, &meta, Window::Turns { before, turns }, all);
    // before=1: nothing precedes the first chain
    let p = turn(Some(1), 5);
    assert_eq!((p.start, p.end, p.earlier), (1, 0, false));
    // before beyond the log counts as absent
    assert_eq!(turn(Some(10_000), 5), turn(None, 5));
    // a limit larger than the thread is the whole thread
    assert_eq!(
        (turn(None, 100).start, turn(None, 100).end),
        (1, head(&events))
    );
    // before in the middle of a chain: the chain that holds it is not in the page
    let second = user_chain_starts(&events)[1];
    let p = turn(Some(second + 1), 100);
    assert_eq!(p.end, second - 1);
    assert_eq!(p.start, 1);
    // an empty log
    let p = read(
        &[],
        &meta,
        Window::Turns {
            before: None,
            turns: 5,
        },
        all,
    );
    assert_eq!((p.start, p.end, p.head, p.earlier), (1, 0, 0, false));
}

#[test]
fn since_goes_back_to_the_chain_that_holds_the_event() {
    let events = load_events("followup");
    let meta = meta_of("followup", &events);
    let starts = user_chain_starts(&events);
    assert!(starts.len() >= 2);
    let second = starts[1];
    for since in [second, second + 1] {
        let page = read(
            &events,
            &meta,
            Window::Since {
                before: None,
                since,
            },
            HistoryLimits::default(),
        );
        assert!(page.start <= since, "{page:?}");
        assert!(page.start > 1 || since <= page.end, "{page:?}");
        assert_eq!(page.end, head(&events));
        assert_eq!(
            page.frames,
            replay_to(&events, &meta, page.end)[replay_to(&events, &meta, page.start - 1).len()..]
        );
    }
    // the turn cap wins over `since`
    let page = read(
        &events,
        &meta,
        Window::Since {
            before: None,
            since: 1,
        },
        limits(1, 4 << 20),
    );
    assert!(page.start > 1, "{page:?}");
}

#[test]
fn a_catch_up_is_the_settled_chains_after_the_one_that_holds_the_event() {
    let events = load_events("followup");
    let meta = meta_of("followup", &events);
    let starts = user_chain_starts(&events);
    let second = starts[1];
    // from a settled point: the rest
    let page = read(
        &events,
        &meta,
        Window::After { after: second - 1 },
        HistoryLimits::default(),
    );
    assert_eq!((page.start, page.end), (second, head(&events)));
    assert_eq!(
        page.frames,
        replay_to(&events, &meta, head(&events))[replay_to(&events, &meta, second - 1).len()..]
    );
    let Anchor { seq, run_id } = page.anchor.clone().expect("a run had ended by then");
    assert!(seq < second, "{seq}");
    assert!(run_id.starts_with("run-"), "{run_id}");
    // from the end: nothing, and the same anchor as the last run
    let page = read(
        &events,
        &meta,
        Window::After {
            after: head(&events),
        },
        HistoryLimits::default(),
    );
    assert!(page.frames.is_empty());
    assert_eq!((page.start, page.end), (head(&events) + 1, head(&events)));
    assert!(page.anchor.is_some());
    // from nothing: the whole log, and no anchor
    let page = read(
        &events,
        &meta,
        Window::After { after: 0 },
        HistoryLimits::default(),
    );
    assert_eq!((page.start, page.end), (1, head(&events)));
    assert!(page.anchor.is_none());
    // from the middle of a chain: the next chain on, and a start that is not after+1 says so
    let page = read(
        &events,
        &meta,
        Window::After { after: 2 },
        HistoryLimits::default(),
    );
    assert!(page.start > 3, "{page:?}");
    // the turn cap: a catch-up holds as many turns as a page may, from where it was asked
    let page = read(
        &events,
        &meta,
        Window::After { after: 0 },
        limits(1, 4 << 20),
    );
    assert_eq!(page.start, 1);
    assert!(page.end < head(&events), "{page:?}");
    assert_eq!(page.frames, replay_to(&events, &meta, page.end));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Over generated logs (steering, forks, surfaces, usage, an open chain at the end, a thread
    /// with no person's message, a frameless tail): pages tile, pages are whole settled runs, the
    /// ids repeated are replacing snapshots, for every `limit`, with and without a small byte cap.
    #[test]
    fn pages_tile_every_generated_log(actions in arb_actions(), gating in 0_u8..3) {
        let (events, meta) = world(gating, &actions);
        for turns in [1usize, 2, 3, 7, 100] {
            for cap in [4usize << 20, 1, 3000] {
                let pages = walk(&events, &meta, turns, limits(100, cap));
                assert_tiles(&events, &meta, &pages);
                assert_settled(&pages);
                let bad: Vec<_> = repeated(&pages)
                    .into_iter()
                    .filter(|(kind, _, replaces)| kind != "activity" || !replaces)
                    .collect();
                prop_assert!(bad.is_empty(), "turns {turns} cap {cap}: {bad:?}");
            }
        }
    }

    /// A catch-up from any settled `end` the walk produced, and from any other event, is the rest
    /// of the replay from the next chain on.
    #[test]
    fn a_catch_up_continues_where_a_page_ended(actions in arb_actions(), gating in 0_u8..3, at in any::<u16>()) {
        let (events, meta) = world(gating, &actions);
        prop_assume!(!events.is_empty());
        let after = i64::from(at) % (head(&events) + 1);
        let page = read(&events, &meta, Window::After { after }, HistoryLimits::default());
        prop_assert!(page.start > after, "{page:?}");
        let all = replay_to(&events, &meta, page.end);
        let before = replay_to(&events, &meta, page.start - 1).len();
        prop_assert_eq!(&page.frames[..], &all[before..], "after {}", after);
        assert_settled(std::slice::from_ref(&page));
    }
}

/// The boundaries of the pages of every golden, written to `docs/api/examples/history/<name>.pages.json`: for `limit` 1
/// and 2, and with a byte cap smaller than a chain, newest page first. The web's mock folds the same logs with its own copy
/// of the fold and has to find the same boundaries (`web/mock/history.test.ts`), so that the mock a page is tried against
/// tells what the orchestrator does. `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test history` rewrites them.
#[test]
fn the_boundaries_of_the_pages_of_every_golden_are_pinned() {
    let update = std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let dir = support::goldens::examples_dir().join("history");
    let mut stale = Vec::new();
    for name in SCENARIOS {
        let events = load_events(name);
        let meta = meta_of(name, &events);
        let walks = |turns: usize, cap: usize| -> Value {
            let pages = walk(&events, &meta, turns, limits(100, cap));
            Value::Array(
                pages
                    .iter()
                    .map(|p| {
                        json!({
                            "start": p.start,
                            "end": p.end,
                            "earlier": p.earlier,
                            // the agent turns before the page (`carry.turns`), which the web checks
                            // against the turns it counts itself
                            "turnsBefore": p.carry.as_ref().map_or(0, |c| c.turns),
                        })
                    })
                    .collect(),
            )
        };
        let value = json!({
            "limit1": walks(1, 4 << 20),
            "limit2": walks(2, 4 << 20),
            "capped": walks(100, 1),
        });
        let mut text = serde_json::to_string_pretty(&value).unwrap();
        text.push('\n');
        let path = dir.join(format!("{name}.pages.json"));
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&path, &text).unwrap();
        } else if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            stale.push(path.display().to_string());
        }
    }
    assert!(
        stale.is_empty(),
        "history goldens are out of date; run `UPDATE_GOLDEN=1 cargo test -p orch-agui-projection --test history`: {stale:?}"
    );
}

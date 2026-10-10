//! What a history page costs on a thread of 10 000 events (ADR 0059, decision 4): the fold of the
//! newest page, and the cost of reading a thread back to its start, page by page. Not a test of
//! behaviour: `#[ignore]`d, run in release with `--nocapture` to print the numbers the ADR quotes.
//!
//! `cargo test --release -p orch-agui-projection --test history_cost -- --ignored --nocapture`
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    missing_docs,
    clippy::print_stderr
)]

mod support;

use std::time::Instant;

use orch_agui_projection::{Flow, History, HistoryLimits, Window};
use orch_core::{AgentTaskState, Event};
use support::log::{Action, world};

/// A thread of `turns` turns of the shape of a coder's: a message, the agent working, two steps, its
/// words, a file and the end.
fn thread(turns: usize) -> Vec<Event> {
    let mut actions = Vec::new();
    for n in 0..turns {
        actions.push(Action::User {
            text: format!("please fix item {n}"),
            ids: false,
        });
        actions.push(Action::Status(
            AgentTaskState::Working,
            Some("on it".to_owned()),
        ));
        for id in 0..2u8 {
            actions.push(Action::Step {
                id,
                parent: None,
                kind: 1,
                state: 0,
                by_orchestrator: false,
            });
            actions.push(Action::Step {
                id,
                parent: None,
                kind: 1,
                state: 2,
                by_orchestrator: false,
            });
        }
        actions.push(Action::Usage {
            task: 0,
            step: None,
            total: false,
        });
        actions.push(Action::Say {
            slot: 0,
            more: "done with this one".to_owned(),
            fin: true,
        });
        actions.push(Action::Artifact);
        actions.push(Action::Status(AgentTaskState::Completed, None));
    }
    world(0, &actions).0
}

fn read(events: &[Event], window: Window) -> (usize, i64, usize, usize) {
    let (_, meta) = world(0, &[]);
    let head = events.last().map_or(0, |e| e.seq);
    let mut history = History::new(meta, window, HistoryLimits::default(), head);
    let mut folded = 0;
    for event in events {
        folded += 1;
        if history.feed(event) == Flow::Done {
            break;
        }
    }
    let peak = history.peak_bytes();
    let page = history.finish();
    (folded, page.start, page.frames.len(), peak)
}

#[test]
#[ignore = "a measurement, run in release with --nocapture"]
fn what_a_page_costs_on_ten_thousand_events() {
    let events = thread(1000);
    eprintln!("events: {}", events.len());
    let median = |mut runs: Vec<f64>| {
        runs.sort_by(f64::total_cmp);
        runs[runs.len() / 2]
    };
    let timed = |window: Window| {
        median(
            (0..9)
                .map(|_| {
                    let started = Instant::now();
                    let out = read(&events, window);
                    std::hint::black_box(out);
                    started.elapsed().as_secs_f64() * 1000.0
                })
                .collect(),
        )
    };
    let newest = Window::Turns {
        before: None,
        turns: 12,
    };
    let (_, _, frames, peak) = read(&events, newest);
    eprintln!("newest 12 turns: {frames} frames, the fold held {peak} bytes at most");
    eprintln!(
        "newest 12 turns: {:.1} ms (p50 of 9)",
        timed(Window::Turns {
            before: None,
            turns: 12
        })
    );
    // reading back to the start, constant pages and growing pages
    for (name, sizes) in [
        ("pages of 20", vec![20usize]),
        ("pages growing 20, 40, 80, 100", vec![20, 40, 80, 100]),
    ] {
        let started = Instant::now();
        let (mut before, mut pages, mut folded) = (None::<i64>, 0usize, 0usize);
        loop {
            let turns = sizes[pages.min(sizes.len() - 1)];
            let (n, start, _, _) = read(&events, Window::Turns { before, turns });
            folded += n;
            pages += 1;
            if start <= 1 {
                break;
            }
            before = Some(start);
        }
        eprintln!(
            "{name}: {pages} pages, {folded} events folded, {:.0} ms in all",
            started.elapsed().as_secs_f64() * 1000.0
        );
    }
}

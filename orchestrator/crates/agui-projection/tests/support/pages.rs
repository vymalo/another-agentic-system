//! Reading a log in pages, the way the surface does it, for the tests of the history fold.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use orch_agui_projection::{Flow, History, HistoryLimits, Page, ThreadMeta, Window};
use orch_core::Event;

pub fn head(events: &[Event]) -> i64 {
    events.last().map_or(0, |e| e.seq)
}

/// One read of the log, the way the surface does it: events in order until the fold is done.
pub fn read(events: &[Event], meta: &ThreadMeta, window: Window, limits: HistoryLimits) -> Page {
    let mut history = History::new(meta.clone(), window, limits, head(events));
    for event in events {
        if history.feed(event) == Flow::Done {
            break;
        }
    }
    history.finish()
}

pub fn limits(max_turns: usize, max_page_bytes: usize) -> HistoryLimits {
    HistoryLimits {
        max_turns,
        max_page_bytes,
    }
}

/// Every page back from the newest, `turns` turns at a time: newest first.
pub fn walk(events: &[Event], meta: &ThreadMeta, turns: usize, l: HistoryLimits) -> Vec<Page> {
    let mut pages: Vec<Page> = Vec::new();
    let mut before = None;
    loop {
        let page = read(events, meta, Window::Turns { before, turns }, l);
        if let Some(previous) = pages.last() {
            assert!(
                page.end < previous.start || !previous.earlier,
                "no progress: {page:?} after {previous:?}"
            );
        }
        let earlier = page.earlier;
        before = Some(page.start);
        pages.push(page);
        if !earlier {
            return pages;
        }
        assert!(pages.len() <= events.len() + 2, "a walk that never ends");
    }
}

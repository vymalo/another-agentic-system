//! A Rust model of the reference consumer's rules (`verifyEvents` of `@ag-ui/client` 1.0.0, read
//! from its source) plus the rules of this projection: the stream begins with `RUN_STARTED`,
//! runs are balanced and never nested, nothing is open when a run ends (text messages, subagent
//! invocations), ids are unique, attribution names an open invocation, and every event is valid
//! against the vendored schema. `tools/agui-conformance` runs the real reference consumer on the
//! goldens; this checker is what lets the property tests apply the same rules to every random log.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;

use orch_agui_projection::Frame;
use orch_agui_proto::testkit::event_errors;
use orch_agui_proto::{Event, RunFinishedOutcome, SubagentFinishedOutcome};

#[derive(Default)]
pub struct Checker {
    started: bool,
    run: Option<String>,
    ended: bool,
    texts: BTreeSet<String>,
    subagents: BTreeSet<String>,
    closed_subagents: BTreeSet<String>,
    suspended_ids: Vec<String>,
    message_ids: BTreeSet<String>,
    /// Ids that name activity messages: an `ACTIVITY_SNAPSHOT` may say the same id again, and
    /// replaces what it said (`replace` defaults to true), which is how an A2UI surface is
    /// re-sent whole. Only a `replace: false` snapshot of an id that exists would be ignored.
    activity_ids: BTreeSet<String>,
    run_ids: BTreeSet<String>,
    interrupt_ids: BTreeSet<String>,
    last_resume_id: i64,
}

impl Checker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a text message is open now.
    pub fn text_open(&self) -> bool {
        !self.texts.is_empty()
    }

    /// Whether a run is open now.
    pub fn run_open(&self) -> bool {
        self.run.is_some()
    }

    /// Feeds one event; the error says which rule broke.
    pub fn feed(&mut self, event: &Event) -> Result<(), String> {
        let json = serde_json::to_value(event).map_err(|e| e.to_string())?;
        let schema = event_errors(&json);
        if !schema.is_empty() {
            return Err(format!(
                "not valid AG-UI 1.0: {}: {json}",
                schema.join("; ")
            ));
        }
        let unattributed = |e: &Event| e.attributed_to().map(|s| s.as_str().to_owned());
        if !self.started {
            self.started = true;
            if !matches!(event, Event::RunStarted(_) | Event::RunError(_)) {
                return Err(format!(
                    "first event must be RUN_STARTED, got {}",
                    event.event_type().as_str()
                ));
            }
        }
        if self.ended && !matches!(event, Event::RunStarted(_)) {
            return Err(format!(
                "{} after the run ended",
                event.event_type().as_str()
            ));
        }
        if self.run.is_none() && !matches!(event, Event::RunStarted(_) | Event::RunError(_)) {
            return Err(format!("{} outside a run", event.event_type().as_str()));
        }
        if let Some(sub) = unattributed(event)
            && !self.subagents.contains(&sub)
        {
            return Err(format!(
                "{} is attributed to {sub}, which is not an open invocation",
                event.event_type().as_str()
            ));
        }
        match event {
            Event::RunStarted(e) => {
                if self.run.is_some() {
                    return Err("RUN_STARTED while a run is open".into());
                }
                if !self.run_ids.insert(e.run_id.to_string()) {
                    return Err(format!("run id {} reused", e.run_id));
                }
                self.run = Some(e.run_id.to_string());
                self.ended = false;
                self.closed_subagents.clear();
                self.suspended_ids.clear();
            }
            Event::TextMessageStart(e) => {
                if !self.message_ids.insert(e.message_id.to_string()) {
                    return Err(format!("message id {} reused", e.message_id));
                }
                self.texts.insert(e.message_id.to_string());
            }
            Event::TextMessageContent(e) => {
                if !self.texts.contains(e.message_id.as_str()) {
                    return Err(format!(
                        "CONTENT for message {} that is not open",
                        e.message_id
                    ));
                }
            }
            Event::TextMessageEnd(e) => {
                if !self.texts.remove(e.message_id.as_str()) {
                    return Err(format!("END for message {} that is not open", e.message_id));
                }
            }
            Event::ActivitySnapshot(e) => {
                let id = e.message_id.to_string();
                let known_activity = self.activity_ids.contains(&id);
                if self.message_ids.contains(&id) && !known_activity {
                    return Err(format!("activity id {id} is the id of another message"));
                }
                if known_activity && e.replace == Some(false) {
                    return Err(format!(
                        "activity {id} said again with replace: false is ignored"
                    ));
                }
                self.message_ids.insert(id.clone());
                self.activity_ids.insert(id);
            }
            Event::SubagentStarted(e) => {
                let id = e.subagent_run_id.to_string();
                if self.subagents.contains(&id) || self.closed_subagents.contains(&id) {
                    return Err(format!("invocation {id} started twice in one run"));
                }
                self.subagents.insert(id);
            }
            Event::SubagentFinished(e) => {
                let id = e.subagent_run_id.to_string();
                if !self.subagents.remove(&id) {
                    return Err(format!("SUBAGENT_FINISHED for {id}, which is not open"));
                }
                self.closed_subagents.insert(id);
                if let Some(SubagentFinishedOutcome::Suspended {
                    interrupt_ids: Some(ids),
                }) = &e.outcome
                {
                    self.suspended_ids
                        .extend(ids.iter().map(ToString::to_string));
                }
            }
            Event::SubagentError(e) => {
                let id = e.subagent_run_id.to_string();
                if !self.subagents.remove(&id) {
                    return Err(format!("SUBAGENT_ERROR for {id}, which is not open"));
                }
                self.closed_subagents.insert(id);
            }
            Event::RunFinished(e) => {
                if self.run.as_deref() != Some(e.run_id.as_str()) {
                    return Err(format!(
                        "RUN_FINISHED closes {}, but {:?} is open",
                        e.run_id, self.run
                    ));
                }
                self.nothing_open("RUN_FINISHED")?;
                if let Some(RunFinishedOutcome::Interrupt { interrupts }) = &e.outcome {
                    // An interrupt still unanswered when another run ends blocked is raised
                    // again under the same id: the id names the wait, not the run.
                    self.interrupt_ids
                        .extend(interrupts.iter().map(|i| i.id.to_string()));
                    for suspended in &self.suspended_ids {
                        if !interrupts.iter().any(|i| i.id.as_str() == suspended) {
                            return Err(format!(
                                "the invocation suspended on {suspended}, which the run's interrupts do not list"
                            ));
                        }
                    }
                }
                self.run = None;
                self.ended = true;
            }
            Event::RunError(_) => {
                self.nothing_open("RUN_ERROR")?;
                self.run = None;
                self.ended = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn nothing_open(&self, what: &str) -> Result<(), String> {
        if !self.texts.is_empty() {
            return Err(format!("{what} with text messages open: {:?}", self.texts));
        }
        if !self.subagents.is_empty() {
            return Err(format!(
                "{what} with invocations open: {:?}",
                self.subagents
            ));
        }
        Ok(())
    }

    /// Feeds frames, and checks the resume points: strictly increasing, and only where no
    /// text message is open.
    pub fn feed_frames(&mut self, frames: &[Frame]) -> Result<(), String> {
        for frame in frames {
            self.feed(&frame.event)?;
            if let Some(id) = frame.resume_id {
                if id <= self.last_resume_id {
                    return Err(format!(
                        "resume id {id} does not increase (after {})",
                        self.last_resume_id
                    ));
                }
                if self.text_open() {
                    return Err(format!(
                        "resume id {id} on a frame with a text message open"
                    ));
                }
                self.last_resume_id = id;
            }
        }
        Ok(())
    }
}

/// Checks a whole stream.
pub fn check(frames: &[Frame]) -> Result<Checker, String> {
    let mut checker = Checker::new();
    checker.feed_frames(frames)?;
    Ok(checker)
}

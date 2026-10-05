//! A Rust model of the reference consumer's rules (`verifyEvents` of `@ag-ui/client` 1.0.0, read
//! from its source) plus the rules of this projection: the stream begins with `RUN_STARTED`,
//! runs are balanced and never nested, nothing is open when a run ends (text messages, subagent
//! invocations), ids are unique, attribution names an open invocation, and every event is valid
//! against the vendored schema. `tools/agui-conformance` runs the real reference consumer on the
//! goldens; this checker is what lets the property tests apply the same rules to every random log.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{BTreeMap, BTreeSet};

use orch_agui_projection::Frame;
use orch_agui_proto::testkit::event_errors;
use orch_agui_proto::{Event, RunFinishedOutcome, SubagentFinishedOutcome};
use serde_json::Value;

#[derive(Default)]
pub struct Checker {
    started: bool,
    run: Option<String>,
    ended: bool,
    texts: BTreeSet<String>,
    /// The open text messages that are live (`metadata["vymalo.live"]` on their `START`, ADR
    /// 0027): not in the log, so they do not hold a resume point back.
    live_texts: BTreeSet<String>,
    /// The invocation each open text message is attributed to.
    text_owners: BTreeMap<String, String>,
    /// What each text message says so far, read the way the web reads it: a delta is appended,
    /// except that a live one says (`vymalo.live.offset`, UTF-16 code units) where it continues
    /// from, which must be what was said (or, on the log's final message, no more than that).
    read: BTreeMap<String, String>,
    /// The open reasoning spans (`REASONING_START` … `REASONING_END`) and, nested in them, the open reasoning
    /// messages (`REASONING_MESSAGE_START` … `REASONING_MESSAGE_END`), ADR 0044. What the reference client keeps
    /// apart by id, and this projection always nests.
    spans: BTreeSet<String>,
    reasoning_messages: BTreeSet<String>,
    /// The reasoning that is live (`metadata["vymalo.live"]` on its `REASONING_START`): not in the log.
    live_reasoning: BTreeSet<String>,
    /// The invocation each open reasoning span is attributed to.
    span_owners: BTreeMap<String, String>,
    subagents: BTreeSet<String>,
    closed_subagents: BTreeSet<String>,
    /// The enclosing subagent of every open one that has one.
    parents: BTreeMap<String, String>,
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

    /// Whether a text message of the log is open now: a live one is not (it is not in the log, so
    /// an `id:` written while it is open still names a position of the log).
    pub fn text_open(&self) -> bool {
        self.texts.iter().any(|t| !self.live_texts.contains(t))
    }

    /// Whether reasoning of the log is open now (a live one is not: it is not in the log, so an `id:` written while it
    /// is open still names a position of the log).
    pub fn reasoning_open(&self) -> bool {
        self.spans
            .iter()
            .chain(self.reasoning_messages.iter())
            .any(|r| !self.live_reasoning.contains(r))
    }

    /// Whether a live reasoning is open now.
    pub fn live_reasoning_open(&self) -> bool {
        !self.live_reasoning.is_empty()
    }

    /// What the message `id` says so far (every message that was started, open or not).
    pub fn reading(&self, id: &str) -> Option<&str> {
        self.read.get(id).map(String::as_str)
    }

    /// Every message that was started, with what it says.
    pub fn readings(&self) -> &BTreeMap<String, String> {
        &self.read
    }

    /// Whether a live text message is open now.
    pub fn live_open(&self) -> bool {
        !self.live_texts.is_empty()
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
                self.parents.clear();
                self.suspended_ids.clear();
            }
            Event::TextMessageStart(e) => {
                if !self.message_ids.insert(e.message_id.to_string()) {
                    return Err(format!("message id {} reused", e.message_id));
                }
                self.texts.insert(e.message_id.to_string());
                if let Some(owner) = &e.subagent_run_id {
                    self.text_owners
                        .insert(e.message_id.to_string(), owner.to_string());
                }
                self.read.insert(e.message_id.to_string(), String::new());
                if e.base
                    .metadata
                    .as_ref()
                    .is_some_and(|m| m.contains_key("vymalo.live"))
                {
                    self.live_texts.insert(e.message_id.to_string());
                }
            }
            Event::TextMessageContent(e) => {
                if !self.texts.contains(e.message_id.as_str()) {
                    return Err(format!(
                        "CONTENT for message {} that is not open",
                        e.message_id
                    ));
                }
                let live = e
                    .base
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("vymalo.live"))
                    .and_then(Value::as_object);
                let said = self.read.entry(e.message_id.to_string()).or_default();
                match live.and_then(|l| l.get("offset")).and_then(Value::as_u64) {
                    None => said.push_str(&e.delta),
                    Some(offset) => {
                        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
                        let units = said.encode_utf16().count();
                        let is_final = live
                            .and_then(|l| l.get("final"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if (!is_final && offset != units) || offset > units {
                            return Err(format!(
                                "CONTENT for {} continues from {offset}, but {units} UTF-16 units were said (final: {is_final})",
                                e.message_id
                            ));
                        }
                        *said = truncate_utf16(said, offset)?;
                        said.push_str(&e.delta);
                    }
                }
            }
            Event::TextMessageEnd(e) => {
                if !self.texts.remove(e.message_id.as_str()) {
                    return Err(format!("END for message {} that is not open", e.message_id));
                }
                self.live_texts.remove(e.message_id.as_str());
                self.text_owners.remove(e.message_id.as_str());
            }
            Event::ReasoningStart(e) => {
                let id = e.message_id.to_string();
                if !self.spans.insert(id.clone()) {
                    return Err(format!("REASONING_START for {id}, which is already open"));
                }
                if let Some(owner) = &e.subagent_run_id {
                    self.span_owners.insert(id.clone(), owner.to_string());
                }
                if e.base
                    .metadata
                    .as_ref()
                    .is_some_and(|m| m.contains_key("vymalo.live"))
                {
                    self.live_reasoning.insert(id);
                }
            }
            Event::ReasoningMessageStart(e) => {
                let id = e.message_id.to_string();
                if !self.spans.contains(&id) {
                    return Err(format!(
                        "REASONING_MESSAGE_START for {id}, whose REASONING_START was not sent"
                    ));
                }
                if !self.message_ids.insert(id.clone()) {
                    return Err(format!("message id {id} reused"));
                }
                self.reasoning_messages.insert(id.clone());
                self.read.insert(id, String::new());
            }
            Event::ReasoningMessageContent(e) => {
                let id = e.message_id.to_string();
                if !self.reasoning_messages.contains(&id) {
                    return Err(format!(
                        "REASONING_MESSAGE_CONTENT for {id}, which is not an open reasoning message"
                    ));
                }
                let live = e
                    .base
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("vymalo.live"))
                    .and_then(Value::as_object);
                let said = self.read.entry(id.clone()).or_default();
                match live.and_then(|l| l.get("offset")).and_then(Value::as_u64) {
                    None => said.push_str(&e.delta),
                    Some(offset) => {
                        let offset = usize::try_from(offset).unwrap_or(usize::MAX);
                        let units = said.encode_utf16().count();
                        let is_final = live
                            .and_then(|l| l.get("final"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                        if (!is_final && offset != units) || offset > units {
                            return Err(format!(
                                "REASONING_MESSAGE_CONTENT for {id} continues from {offset}, but {units} UTF-16 units were said (final: {is_final})"
                            ));
                        }
                        *said = truncate_utf16(said, offset)?;
                        said.push_str(&e.delta);
                    }
                }
            }
            Event::ReasoningMessageEnd(e) => {
                if !self.reasoning_messages.remove(e.message_id.as_str()) {
                    return Err(format!(
                        "REASONING_MESSAGE_END for {}, which is not open",
                        e.message_id
                    ));
                }
            }
            Event::ReasoningEnd(e) => {
                let id = e.message_id.as_str();
                if self.reasoning_messages.contains(id) {
                    return Err(format!(
                        "REASONING_END for {id} before its REASONING_MESSAGE_END"
                    ));
                }
                if !self.spans.remove(id) {
                    return Err(format!("REASONING_END for {id}, which is not open"));
                }
                self.live_reasoning.remove(id);
                self.span_owners.remove(id);
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
                // The reference client: the enclosing subagent must have started in this run.
                if let Some(parent) = &e.parent_subagent_run_id {
                    let parent = parent.to_string();
                    if !self.subagents.contains(&parent) && !self.closed_subagents.contains(&parent)
                    {
                        return Err(format!(
                            "invocation {id} names the parent {parent}, which has not started in this run"
                        ));
                    }
                    self.parents.insert(id.clone(), parent);
                }
                self.subagents.insert(id);
            }
            Event::SubagentFinished(e) => {
                let id = e.subagent_run_id.to_string();
                self.close_nested(&id, "SUBAGENT_FINISHED")?;
                self.close_texts(&id, "SUBAGENT_FINISHED")?;
                self.close_spans(&id, "SUBAGENT_FINISHED")?;
                if !self.subagents.remove(&id) {
                    return Err(format!("SUBAGENT_FINISHED for {id}, which is not open"));
                }
                self.parents.remove(&id);
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
                self.close_nested(&id, "SUBAGENT_ERROR")?;
                self.close_texts(&id, "SUBAGENT_ERROR")?;
                self.close_spans(&id, "SUBAGENT_ERROR")?;
                if !self.subagents.remove(&id) {
                    return Err(format!("SUBAGENT_ERROR for {id}, which is not open"));
                }
                self.parents.remove(&id);
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

    /// This projection's rule, beyond the reference client's: a subagent does not end while one
    /// that runs in it is still open.
    fn close_nested(&self, id: &str, what: &str) -> Result<(), String> {
        if let Some((child, _)) = self
            .parents
            .iter()
            .find(|(_, parent)| parent.as_str() == id)
        {
            return Err(format!(
                "{what} for {id} while {child}, which runs in it, is open"
            ));
        }
        Ok(())
    }

    /// This projection's rule too: a text message attributed to a subagent is over before the
    /// subagent is.
    fn close_texts(&self, id: &str, what: &str) -> Result<(), String> {
        if let Some((message, _)) = self
            .text_owners
            .iter()
            .find(|(_, owner)| owner.as_str() == id)
        {
            return Err(format!(
                "{what} for {id} while its message {message} is open"
            ));
        }
        Ok(())
    }

    /// And a reasoning span attributed to a subagent is over before the subagent is.
    fn close_spans(&self, id: &str, what: &str) -> Result<(), String> {
        if let Some((span, _)) = self
            .span_owners
            .iter()
            .find(|(_, owner)| owner.as_str() == id)
        {
            return Err(format!(
                "{what} for {id} while its reasoning {span} is open"
            ));
        }
        Ok(())
    }

    fn nothing_open(&self, what: &str) -> Result<(), String> {
        if !self.spans.is_empty() || !self.reasoning_messages.is_empty() {
            return Err(format!(
                "{what} with reasoning open: {:?} {:?}",
                self.spans, self.reasoning_messages
            ));
        }
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
                if self.reasoning_open() {
                    return Err(format!("resume id {id} on a frame with reasoning open"));
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

/// The first `units` UTF-16 code units of `text`; an error when that would split a character.
fn truncate_utf16(text: &str, units: usize) -> Result<String, String> {
    let mut count = 0;
    let mut out = String::new();
    for c in text.chars() {
        if count == units {
            return Ok(out);
        }
        count += c.len_utf16();
        if count > units {
            return Err(format!("{units} UTF-16 units split the character {c:?}"));
        }
        out.push(c);
    }
    Ok(out)
}

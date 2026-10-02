//! The MCP servers attached to a thread (ADR 0024, `docs/api/thread-tools-v1.md`).
//!
//! A person attaches servers from the deployment's list to a conversation and can detach them.
//! The thread remembers the set in its job ledger ([`Job::tools`](crate::Job::tools): the
//! servers' ids, sorted and unique, carried from job to job like the gate), and the log records
//! each change as a `tools_attached` or a `tools_detached` event ([`ToolsData`]). The core knows
//! ids and nothing else about a server: that an id is one the deployment lists, and one its
//! agent may use, is the application's to check ([`check_servers`] checks only the shape).

use serde::{Deserialize, Serialize};

use crate::event::{Event, EventBody};

/// Most servers attached to one thread.
pub const MAX_ATTACHED_SERVERS: usize = 16;

/// The longest server id: `^[a-z0-9][a-z0-9-]{0,30}$`.
pub const MAX_SERVER_ID_BYTES: usize = 31;

/// Whether `id` is a server id: lower-case letters, digits and `-`, starting with a letter or a
/// digit, at most [`MAX_SERVER_ID_BYTES`] long. It has no `_`, so the first `__` of a relayed tool's
/// name (`<server>__<tool>`) is always the split.
pub fn is_valid_server_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    id.len() <= MAX_SERVER_ID_BYTES
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `data` of a `tools_attached` and of a `tools_detached`: the ids of the servers that were
/// attached (or detached) by this event. Never empty, sorted, unique, and never anything but ids:
/// no URL, no header, no credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolsData {
    /// The server ids.
    pub servers: Vec<String>,
}

/// A server that cannot be attached to a thread, by its shape.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolsError {
    /// An id that is not `^[a-z0-9][a-z0-9-]{0,30}$`.
    #[error("`{0}` is not a server id")]
    BadId(String),
    /// More than [`MAX_ATTACHED_SERVERS`] distinct servers.
    #[error("at most {MAX_ATTACHED_SERVERS} servers can be attached to a thread")]
    TooMany,
}

/// The set of servers `servers` names, as a thread keeps it: sorted and without repeats. Each id
/// must have the shape of one and there may be at most [`MAX_ATTACHED_SERVERS`] of them.
///
/// # Errors
/// [`ToolsError::BadId`] for the first id that is not one, [`ToolsError::TooMany`] for a longer set.
pub fn check_servers(servers: &[String]) -> Result<Vec<String>, ToolsError> {
    if let Some(bad) = servers.iter().find(|s| !is_valid_server_id(s)) {
        return Err(ToolsError::BadId(bad.clone()));
    }
    let set = normalized(servers);
    if set.len() > MAX_ATTACHED_SERVERS {
        return Err(ToolsError::TooMany);
    }
    Ok(set)
}

/// `servers` sorted, without repeats.
pub(crate) fn normalized(servers: &[String]) -> Vec<String> {
    let mut set = servers.to_vec();
    set.sort();
    set.dedup();
    set
}

/// What changes when a thread whose set is `current` is given the set `wanted`: the ids to attach
/// and the ids to detach, each sorted. Both are empty when the sets are the same, and then nothing
/// is logged. Both arguments are sorted and unique ([`normalized`]).
pub(crate) fn changes(current: &[String], wanted: &[String]) -> (Vec<String>, Vec<String>) {
    let attached = wanted
        .iter()
        .filter(|s| current.binary_search(s).is_err())
        .cloned()
        .collect();
    let detached = current
        .iter()
        .filter(|s| wanted.binary_search(s).is_err())
        .cloned()
        .collect();
    (attached, detached)
}

/// The set of servers the events `events` leave attached, in the order they were logged: what a
/// thread copied from them (a fork) starts with.
pub fn attached_by(events: &[Event]) -> Vec<String> {
    let mut set: Vec<String> = Vec::new();
    for event in events {
        match &event.body {
            EventBody::ToolsAttached(d) => set.extend(d.servers.iter().cloned()),
            EventBody::ToolsDetached(d) => set.retain(|s| !d.servers.contains(s)),
            _ => {}
        }
    }
    normalized(&set)
}

/// A server attached to a thread, as the agent is told of it (`attached` of the `thread-tools/v1`
/// message metadata): its id, its name and what it is for. **Never a URL, a header or a
/// credential**: the agent reaches a server only through the thread's endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachedServer {
    /// The server's id: the prefix of its tools on the endpoint (`<id>__<tool>`).
    pub id: String,
    /// The display name.
    pub name: String,
    /// What it is for, when the deployment says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|i| (*i).to_owned()).collect()
    }

    #[test]
    fn a_server_id_is_lower_case_words_with_dashes() {
        for good in ["websearch", "a", "0day", "web-search-2", &"a".repeat(31)] {
            assert!(is_valid_server_id(good), "{good}");
        }
        for bad in [
            "",
            "-x",
            "Web",
            "web_search",
            "web search",
            "web/search",
            "é",
            &"a".repeat(32),
        ] {
            assert!(!is_valid_server_id(bad), "{bad}");
        }
    }

    #[test]
    fn a_set_is_sorted_unique_and_at_most_sixteen() {
        assert_eq!(
            check_servers(&s(&["b", "a", "b"])),
            Ok(s(&["a", "b"])),
            "repeats collapse"
        );
        assert_eq!(check_servers(&[]), Ok(vec![]));
        assert_eq!(
            check_servers(&s(&["a", "No"])),
            Err(ToolsError::BadId("No".to_owned()))
        );
        let sixteen: Vec<String> = (0..16).map(|n| format!("s{n}")).collect();
        assert_eq!(check_servers(&sixteen).map(|set| set.len()), Ok(16));
        let seventeen: Vec<String> = (0..17).map(|n| format!("s{n}")).collect();
        assert_eq!(check_servers(&seventeen), Err(ToolsError::TooMany));
        let repeated: Vec<String> = (0..40).map(|_| "same".to_owned()).collect();
        assert_eq!(check_servers(&repeated), Ok(s(&["same"])));
    }

    #[test]
    fn changes_are_the_difference_both_ways() {
        assert_eq!(
            changes(&s(&["a", "b"]), &s(&["b", "c"])),
            (s(&["c"]), s(&["a"]))
        );
        assert_eq!(changes(&s(&["a"]), &s(&["a"])), (vec![], vec![]));
        assert_eq!(changes(&[], &s(&["a", "b"])), (s(&["a", "b"]), vec![]));
        assert_eq!(changes(&s(&["a", "b"]), &[]), (vec![], s(&["a", "b"])));
    }
}

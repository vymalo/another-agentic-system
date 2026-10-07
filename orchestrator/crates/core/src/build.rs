//! Which build of an agent a thread's jobs ran on (ADR 0053): the identity its live card gave
//! when the orchestrator was about to give it work, recorded in the job ledger so that a thread
//! export can say it long after the agent was upgraded.
//!
//! Pure data and bounds. The card is the agent's own word, so everything in it is untrusted text:
//! it is cut to the limits here, in the core, whatever adapter read it ("the door is checked
//! again"), and it is never read as an instruction.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::gate::truncate_to;
use crate::ids::AgentId;

/// The most bytes of a card's `name` or `version` that are kept.
pub const MAX_BUILD_TEXT_BYTES: usize = 128;
/// The most parameters of a card's build extension that are kept.
pub const MAX_BUILD_PARAMS: usize = 8;
/// The most bytes of a parameter's name that are kept.
pub const MAX_BUILD_KEY_BYTES: usize = 64;
/// The most bytes of a parameter's value that are kept.
pub const MAX_BUILD_VALUE_BYTES: usize = 256;
/// The most builds a job keeps: when there are more, the oldest go.
pub const MAX_BUILDS: usize = 32;

/// What an agent's live card said it was, when a job was about to work with it.
///
/// `name` and `version` are the card's own, free-form (adam-rs puts its revision in the version as
/// semver build metadata, `0.3.0+<sha>`); `build` is whatever a build extension of the card listed
/// among its parameters, as text. Nothing here is required: a card that says nothing is a build
/// with only an `agent` and the job it was first seen in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBuild {
    /// The agent, by the id the orchestrator knows it under.
    pub agent: AgentId,
    /// The card's `name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The card's `version`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The parameters of the card's build extension, when it has one.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub build: BTreeMap<String, String>,
    /// The first job of the thread this build was seen working in (set by the core, from 1).
    #[serde(default = "first_job")]
    pub job: u32,
}

fn first_job() -> u32 {
    1
}

impl AgentBuild {
    /// A build of `agent` as its card said it, bounded; the job is set when the core records it.
    #[must_use]
    pub fn new(
        agent: AgentId,
        name: Option<&str>,
        version: Option<&str>,
        build: impl IntoIterator<Item = (String, String)>,
    ) -> Self {
        let text = |t: Option<&str>| {
            t.map(str::trim)
                .filter(|t| !t.is_empty())
                .map(|t| truncate_to(t, MAX_BUILD_TEXT_BYTES).to_owned())
        };
        AgentBuild {
            agent,
            name: text(name),
            version: text(version),
            build: build
                .into_iter()
                .filter(|(k, v)| !k.trim().is_empty() && !v.trim().is_empty())
                .take(MAX_BUILD_PARAMS)
                .map(|(k, v)| {
                    (
                        truncate_to(k.trim(), MAX_BUILD_KEY_BYTES).to_owned(),
                        truncate_to(v.trim(), MAX_BUILD_VALUE_BYTES).to_owned(),
                    )
                })
                .collect(),
            job: 1,
        }
    }

    /// The same build, bounded again and put in `job`: what the ledger keeps of an input.
    pub(crate) fn recorded_in(&self, job: u32) -> Self {
        let mut again = AgentBuild::new(
            self.agent.clone(),
            self.name.as_deref(),
            self.version.as_deref(),
            self.build.clone(),
        );
        again.job = job.max(1);
        again
    }

    /// Whether `other` says the same of the same agent (the job it was seen in does not count).
    fn same_as(&self, other: &AgentBuild) -> bool {
        self.agent == other.agent
            && self.name == other.name
            && self.version == other.version
            && self.build == other.build
    }
}

/// Adds `build` to `builds` unless the agent's latest entry says the same; keeps the newest
/// [`MAX_BUILDS`]. Returns whether the ledger changed.
pub(crate) fn note_build(builds: &mut Vec<AgentBuild>, build: AgentBuild) -> bool {
    let latest = builds.iter().rev().find(|b| b.agent == build.agent);
    if latest.is_some_and(|l| l.same_as(&build)) {
        return false;
    }
    builds.push(build);
    if builds.len() > MAX_BUILDS {
        let over = builds.len() - MAX_BUILDS;
        builds.drain(..over);
    }
    true
}

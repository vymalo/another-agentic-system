//! The configuration of the verification gate (ADR 0018): three layers, one rule set.
//!
//! A [`GateLayer`] is what one layer says: the deployment (`ORCH_GATE` and its siblings), a
//! target (the `gate` key of an `AGENTS_FILE` entry) or a thread (`forwardedProps["vymalo.gate"]`
//! of a run). [`GateRules::apply`] puts a layer on the policy above it and enforces, in one
//! place, what may not be done:
//!
//! * **Sources: a layer may add, never remove.** Its `require` is the whole list and must contain
//!   the one the layer above requires; `ci.required` is unioned with the layer above's. **Attempts:
//!   anywhere within `1..=cap`**, lower or higher than the layer above's. The one removal allowed
//!   is for the verifier itself: the entry of the agent that is the verifier may leave the
//!   `verifier` source out for itself, because an agent cannot verify its own work.
//! * **This build honours only some sources** ([`GateRules::honours`]). Until the inbox and the
//!   timers (slice 5), the CI results (slice 6) and the verifier dispatch (slice 10) exist, the
//!   application drops the commands a `ci` or `verifier` source needs (`Watch`, `Schedule`,
//!   `RequestVerification`), so a gate that required either would wait for a verdict that can
//!   never come. Configuration therefore refuses them in every layer, and fails closed: the
//!   binary exits 78, a request is a 400. [`pending_reason`] says which sources those are and
//!   why; [`GateRules::honouring`] is how a build (or a test) that has them says so. A slice that
//!   makes a source real changes that arm, and then also owns what the source needs on top of the
//!   rules here (its own settings, its own checks in [`GateRules::check_verifier`]).
//! * **The verifier is another agent**, and only the deployment and a target choose it.

use std::collections::{BTreeMap, BTreeSet};

use orch_core::{AgentId, CheckSource, CiPolicy, GatePolicy};
use serde::Deserialize;
use serde_json::Value;

use crate::AgentDirectory;

/// The key of `forwardedProps` (AG-UI) and of `start_job` (MCP) that carries a thread's
/// [`GateLayer`].
pub const THREAD_GATE_KEY: &str = "vymalo.gate";

/// Attempts the deployment allows a target or a thread to raise the limit to, when
/// `ORCH_MAX_ATTEMPTS_CAP` says nothing.
pub const DEFAULT_MAX_ATTEMPTS_CAP: u32 = 10;

/// The most `ORCH_MAX_ATTEMPTS_CAP` may be: a job is bounded by construction (ADR 0018), and this
/// is the bound of the bound.
pub const MAX_ATTEMPTS_CAP_CEILING: u32 = 100;

/// Why this build cannot honour `source` yet, or `None` when it can.
///
/// This is the single place that decides which sources are real. A slice that makes one real
/// changes its arm to `None`; nothing else in configuration knows the difference.
pub fn pending_reason(source: CheckSource) -> Option<&'static str> {
    match source {
        CheckSource::AgentChecks => None,
        CheckSource::Ci => Some(
            "CI results need the inbox and timers (MVP slice 5) and the CI webhook (MVP slice 6), \
             which are not built yet",
        ),
        CheckSource::Verifier => {
            Some("the verifier needs its dispatch (MVP slice 10), which is not built yet")
        }
    }
}

/// A source as configuration spells it: `ci`, `agent-checks` or `verifier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceName(pub CheckSource);

impl<'de> Deserialize<'de> for SourceName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        CheckSource::from_config_name(&name)
            .map(SourceName)
            .ok_or_else(|| {
                serde::de::Error::custom(format!(
                    "unknown gate source {name:?} (one of: {})",
                    known_sources()
                ))
            })
    }
}

/// The spellings of every source, for messages.
pub fn known_sources() -> String {
    CheckSource::ALL.map(CheckSource::config_name).join(", ")
}

/// The `ci` settings of a layer.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CiLayer {
    /// Names of the checks that must pass; empty or absent: the first completed report decides.
    pub required: Option<BTreeSet<String>>,
    /// Seconds CI may take before the thread blocks.
    pub timeout_secs: Option<i64>,
}

/// What one layer says about the gate. Every member is optional: an absent member leaves the
/// layer above as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GateLayer {
    /// The sources that must pass. At least the ones the layer above requires.
    pub require: Option<Vec<SourceName>>,
    /// Attempts, the first included: `1..=cap`.
    pub max_attempts: Option<u32>,
    /// The agent that verifies (a deployment or a target chooses it, never a thread).
    pub verifier: Option<String>,
    /// The `ci` settings (a deployment or a target, never a thread).
    pub ci: Option<CiLayer>,
}

impl GateLayer {
    /// Reads a layer from JSON (the value of `forwardedProps["vymalo.gate"]`). `null` is no
    /// layer.
    pub fn from_json(value: &Value) -> Result<Option<GateLayer>, GateError> {
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|e| GateError::Malformed(e.to_string()))
    }
}

/// Which layer a [`GateLayer`] is, for messages and for what it may set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layer {
    /// `ORCH_GATE`, `ORCH_MAX_ATTEMPTS`, `ORCH_VERIFIER`.
    Deployment,
    /// The `gate` key of this agent's `AGENTS_FILE` entry.
    Target(AgentId),
    /// The request that creates a thread.
    Thread,
}

impl std::fmt::Display for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Layer::Deployment => f.write_str("the deployment gate"),
            Layer::Target(agent) => {
                write!(f, "the gate of agent {:?} in AGENTS_FILE", agent.as_str())
            }
            Layer::Thread => write!(f, "the thread's gate ({THREAD_GATE_KEY:?})"),
        }
    }
}

/// A gate configuration the rules refuse. The message is for the operator (startup) or the
/// caller (a 400); it carries no secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GateError {
    /// The layer is not the expected JSON.
    #[error("the gate is malformed: {0}")]
    Malformed(String),
    /// The layer asks for something this build cannot honour yet.
    #[error(
        "{layer}: {what} is not available yet: {needs}. Until then only {} can be required",
        honoured_names()
    )]
    Unavailable {
        /// The layer.
        layer: Layer,
        /// What was asked for (`the ci source`, `the verifier setting`, …).
        what: String,
        /// Why it cannot be honoured, and which slice enables it.
        needs: &'static str,
    },
    /// `maxAttempts` outside `1..=cap`.
    #[error("{layer}: maxAttempts {value} is outside 1..={cap}")]
    Attempts {
        /// The layer.
        layer: Layer,
        /// What it asked for.
        value: u32,
        /// The most the deployment allows.
        cap: u32,
    },
    /// The layer leaves out a source the layer above requires.
    #[error(
        "{layer}: require leaves out {}, which the gate above requires; a layer may add sources \
         but not remove them",
        dropped.config_name()
    )]
    Removes {
        /// The layer.
        layer: Layer,
        /// The source it dropped.
        dropped: CheckSource,
    },
    /// A thread tried to choose what only the deployment or a target chooses.
    #[error(
        "{layer}: {setting} cannot be set per thread; the deployment or the agent's entry in AGENTS_FILE configures it"
    )]
    NotPerThread {
        /// The layer (always the thread).
        layer: Layer,
        /// The setting.
        setting: &'static str,
    },
    /// A timeout below one second.
    #[error("{layer}: ci.timeoutSecs must be at least 1")]
    Timeout {
        /// The layer.
        layer: Layer,
    },
    /// The verifier is required but no agent verifies.
    #[error(
        "{layer}: the verifier source is required but no verifier agent is configured (set `verifier`)"
    )]
    NoVerifier {
        /// The agent whose gate it is, when known.
        layer: Layer,
    },
    /// The verifier names an agent that is not configured.
    #[error("{layer}: verifier {verifier:?} is not a configured agent")]
    UnknownVerifier {
        /// The layer.
        layer: Layer,
        /// The id as written.
        verifier: String,
    },
    /// An agent cannot verify its own work.
    #[error(
        "{layer}: agent {agent:?} would verify its own work; the verifier must be another configured \
         agent. Give the entry of {agent:?} in AGENTS_FILE a `gate` whose `require` leaves out `verifier`"
    )]
    SelfVerifier {
        /// The layer.
        layer: Layer,
        /// The agent.
        agent: String,
    },
}

fn honoured_names() -> String {
    let names: Vec<&str> = CheckSource::ALL
        .into_iter()
        .filter(|s| pending_reason(*s).is_none())
        .map(CheckSource::config_name)
        .collect();
    names.join(", ")
}

/// The rules layers are checked against: which sources this build honours and how far attempts
/// may be raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRules {
    honoured: BTreeSet<CheckSource>,
    cap: u32,
}

impl Default for GateRules {
    fn default() -> Self {
        GateRules::new(DEFAULT_MAX_ATTEMPTS_CAP)
    }
}

impl GateRules {
    /// The rules of this build (the sources [`pending_reason`] lets through) with `cap` as the
    /// most attempts a target or a thread may ask for.
    pub fn new(cap: u32) -> Self {
        GateRules {
            honoured: CheckSource::ALL
                .into_iter()
                .filter(|s| pending_reason(*s).is_none())
                .collect(),
            cap: cap.clamp(1, MAX_ATTEMPTS_CAP_CEILING),
        }
    }

    /// The same rules for a build that honours `sources` (for the tests of what comes with the
    /// later slices).
    #[must_use]
    pub fn honouring(mut self, sources: impl IntoIterator<Item = CheckSource>) -> Self {
        self.honoured = sources.into_iter().collect();
        self
    }

    /// The most attempts a target or a thread may ask for.
    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// Whether this build honours `source`.
    pub fn honours(&self, source: CheckSource) -> bool {
        self.honoured.contains(&source)
    }

    /// The refusal for requiring `source` in `layer`, when this build cannot honour it.
    pub fn refuse_source(&self, layer: &Layer, source: CheckSource) -> Option<GateError> {
        if self.honours(source) {
            return None;
        }
        Some(GateError::Unavailable {
            layer: layer.clone(),
            what: format!("the {} source", source.config_name()),
            needs: pending_reason(source).unwrap_or("this build does not honour it"),
        })
    }

    /// The refusal for any `verifier` setting in `layer`, when this build cannot honour the
    /// verifier.
    pub fn refuse_verifier_setting(&self, layer: &Layer) -> Option<GateError> {
        self.refuse_setting(layer, CheckSource::Verifier, "the verifier setting")
    }

    /// The refusal for any `ci` setting in `layer`, when this build cannot honour CI.
    pub fn refuse_ci_settings(&self, layer: &Layer) -> Option<GateError> {
        self.refuse_setting(layer, CheckSource::Ci, "the ci settings")
    }

    fn refuse_setting(&self, layer: &Layer, source: CheckSource, what: &str) -> Option<GateError> {
        if self.honours(source) {
            return None;
        }
        Some(GateError::Unavailable {
            layer: layer.clone(),
            what: what.to_owned(),
            needs: pending_reason(source).unwrap_or("this build does not honour it"),
        })
    }

    /// Checks that `attempts` is in `1..=cap`.
    pub fn check_attempts(&self, layer: &Layer, attempts: u32) -> Result<(), GateError> {
        if (1..=self.cap).contains(&attempts) {
            Ok(())
        } else {
            Err(GateError::Attempts {
                layer: layer.clone(),
                value: attempts,
                cap: self.cap,
            })
        }
    }

    /// Puts `layer` on `above`: the policy the layers above produced. Nothing changes when an
    /// error is returned.
    pub fn apply(
        &self,
        above: &GatePolicy,
        layer: &GateLayer,
        at: &Layer,
    ) -> Result<GatePolicy, GateError> {
        // What this build cannot honour goes first, so the message names the slice that will.
        if let Some(sources) = &layer.require {
            for SourceName(source) in sources {
                if let Some(refusal) = self.refuse_source(at, *source) {
                    return Err(refusal);
                }
            }
        }
        if layer.verifier.is_some()
            && let Some(refusal) = self.refuse_verifier_setting(at)
        {
            return Err(refusal);
        }
        if layer.ci.is_some()
            && let Some(refusal) = self.refuse_ci_settings(at)
        {
            return Err(refusal);
        }
        if *at == Layer::Thread {
            for (set, setting) in [
                (layer.verifier.is_some(), "verifier"),
                (layer.ci.is_some(), "ci"),
            ] {
                if set {
                    return Err(GateError::NotPerThread {
                        layer: at.clone(),
                        setting,
                    });
                }
            }
        }

        let mut policy = above.clone();
        if let Some(sources) = &layer.require {
            let wanted: BTreeSet<CheckSource> = sources.iter().map(|s| s.0).collect();
            // The verifier's own entry may leave the verifier out for itself, and nothing else out.
            let is_the_verifier = |agent: &AgentId| above.verifier.as_ref() == Some(agent);
            let may_drop = |s: &CheckSource| {
                *s == CheckSource::Verifier && matches!(at, Layer::Target(a) if is_the_verifier(a))
            };
            if let Some(dropped) = above
                .require
                .iter()
                .find(|s| !wanted.contains(s) && !may_drop(s))
            {
                return Err(GateError::Removes {
                    layer: at.clone(),
                    dropped: *dropped,
                });
            }
            policy.require = wanted;
        }
        if let Some(attempts) = layer.max_attempts {
            self.check_attempts(at, attempts)?;
            policy.max_attempts = attempts;
        }
        if let Some(verifier) = &layer.verifier {
            policy.verifier = Some(AgentId::new(verifier.clone()));
        }
        if let Some(ci) = &layer.ci {
            // Names add up: a layer cannot weaken the checks the layer above waits for.
            if let Some(required) = &ci.required {
                policy.ci.required.extend(required.iter().cloned());
            }
            if let Some(secs) = ci.timeout_secs {
                if secs < 1 {
                    return Err(GateError::Timeout { layer: at.clone() });
                }
                policy.ci.timeout = jiff::SignedDuration::from_secs(secs);
            }
        }
        Ok(policy)
    }

    /// Checks the verifier of a resolved gate against the configured agents: when the gate
    /// requires the verifier there must be one; any verifier must be a configured agent; and an
    /// agent whose gate requires the verifier cannot be that verifier itself (an agent whose gate
    /// does not can: the reviewer is not reviewed by itself).
    pub fn check_verifier(
        &self,
        policy: &GatePolicy,
        target: &AgentId,
        agents: &AgentDirectory,
        at: &Layer,
    ) -> Result<(), GateError> {
        match &policy.verifier {
            None => {
                if policy.requires(CheckSource::Verifier) {
                    return Err(GateError::NoVerifier { layer: at.clone() });
                }
            }
            Some(verifier) => {
                if agents.get(verifier).is_none() {
                    return Err(GateError::UnknownVerifier {
                        layer: at.clone(),
                        verifier: verifier.to_string(),
                    });
                }
                if policy.requires(CheckSource::Verifier) && verifier == target {
                    return Err(GateError::SelfVerifier {
                        layer: at.clone(),
                        agent: verifier.to_string(),
                    });
                }
            }
        }
        Ok(())
    }

    /// The policy new threads of `target` start under before any request: the deployment's,
    /// then the target's.
    pub fn for_target(
        &self,
        deployment: &GatePolicy,
        target: &AgentId,
        layer: Option<&GateLayer>,
    ) -> Result<GatePolicy, GateError> {
        match layer {
            Some(layer) => self.apply(deployment, layer, &Layer::Target(target.clone())),
            None => Ok(deployment.clone()),
        }
    }

    /// Checks a policy that was not built by [`apply`](Self::apply) (the deployment's, as a
    /// composition root hands it over): it requires and configures only what this build honours,
    /// and its attempts are in range.
    pub fn check_policy(&self, policy: &GatePolicy, at: &Layer) -> Result<(), GateError> {
        for source in &policy.require {
            if let Some(refusal) = self.refuse_source(at, *source) {
                return Err(refusal);
            }
        }
        if policy.verifier.is_some()
            && let Some(refusal) = self.refuse_verifier_setting(at)
        {
            return Err(refusal);
        }
        if policy.ci != CiPolicy::default()
            && let Some(refusal) = self.refuse_ci_settings(at)
        {
            return Err(refusal);
        }
        self.check_attempts(at, policy.max_attempts)
    }

    /// Startup validation: the deployment's policy is valid ([`check_policy`](Self::check_policy)),
    /// and so is every agent's resolved gate, whose verifier, if any, is a configured agent.
    /// `targets` are the `gate` keys of `AGENTS_FILE` by agent.
    pub fn validate(
        &self,
        deployment: &GatePolicy,
        targets: &BTreeMap<AgentId, GateLayer>,
        agents: &AgentDirectory,
    ) -> Result<(), GateError> {
        self.check_policy(deployment, &Layer::Deployment)?;
        for entry in agents.iter() {
            let id = &entry.endpoint.id;
            let layer = targets.get(id);
            let policy = self.for_target(deployment, id, layer)?;
            let at = match layer {
                Some(_) => Layer::Target(id.clone()),
                None => Layer::Deployment,
            };
            self.check_verifier(&policy, id, agents, &at)?;
        }
        Ok(())
    }
}

//! The team ledger: what a team thread means to the core, purely ([ADR 0074], decision 15).
//!
//! A team is several primary agents in one thread ([ADR 0074]). This module holds the part the
//! core decides without I/O: who the members are, whose turn is running, the hand-offs counted
//! against a person's message, the fan-out and the fan-in, and the closed state the thread has
//! because of them. It never parses an answer (the `hand_off` tool and the trailing `@member`
//! line are `T1b`), never sends a push (`T1c`) and never reads a store (`T1d`).
//!
//! # What a fold reads
//!
//! [`TeamLedger::apply`] takes one [`TeamEvent`] and never fails: a log the core would refuse was
//! refused before it was written ([`Member::new`], [`TeamLedger::formed`]), so a replay of a
//! thread's log is total. [`TeamLedger::replay`] is that fold from the first event, which must be
//! [`TeamEvent::Formed`].
//!
//! # The rules this file is
//!
//! * **Routing** (decision 4): a person's message that names no member goes to the lead; one that
//!   names members goes to each of them, and two or more is a fan-out whose results go back to the
//!   person. A person's message ends a pending hand-off or checkpoint.
//! * **The cap** (decision 8): at most `maxHandoffs` hand-offs per person's message, 6 by default
//!   and at most 20; a fan-out to N counts as N, the person's own fan-out included. The one over
//!   the cap is not routed: the ledger is [`TeamState::Capped`] and the floor goes to the person.
//! * **Fan-out and fan-in** (decision 6): a hand-off to several members starts their turns in
//!   parallel; when, and only when, every one has ended, the member that fanned out gets a turn
//!   with all their results, each labelled by member — [`TeamCommand::FanIn`].
//! * **One agent** (decision 16): a thread with a single agent has no team state at all, which is
//!   why [`TeamLedger::formed`] refuses fewer than [`MIN_MEMBERS`] members: the caller keeps no
//!   ledger for such a thread.
//!
//! A member that is removed takes its pending hand-off with it, and a turn of it that was running
//! counts as stopped, so a fan-in it was part of can still complete.
//!
//! # And the preset a team is saved as
//!
//! [`presets`] holds the other half of ADR 0074 decision 13: the team a person **saves** — a name, a
//! title, the lead, the members with their optional release and how their history reaches them, a
//! charter, the cap of hand-offs, when a hand-off waits for the person, the checkpoint rules and the
//! budget of a push. It reuses this module's vocabulary rather than defining a second one:
//! [`MemberName`], [`MIN_MEMBERS`], [`MAX_MEMBERS`], [`MAX_NAME_CHARS`], [`MAX_ROLE_CHARS`],
//! [`MIN_MAX_HANDOFFS`], [`MAX_HANDOFFS_CEILING`] and [`TeamRefusal`] are the ledger's, and a preset
//! that breaks one of their rules is refused **with that refusal**. The preset's own bounds are the
//! ones the ledger does not own: the approval's seconds, the charter's bytes, the push budget and
//! how many presets a person holds.
//!
//! [ADR 0074]: https://github.com/vymalo/another-agentic-system/blob/main/docs/decisions/0074-agent-team.md

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ids::AgentId;

mod presets;

pub use presets::{
    BadTeamPresetId, CheckpointRules, DEFAULT_PUSH_TOKENS, HandoffApproval, MAX_APPROVAL_SECONDS,
    MAX_CHARTER_BYTES, MAX_PUSH_TOKENS, MAX_TEAM_PRESETS, MAX_TEAM_TITLE_CHARS,
    MIN_APPROVAL_SECONDS, MIN_PUSH_TOKENS, MemberHistory, PAGE, TeamMember, TeamMemberInput,
    TeamPreset, TeamPresetError, TeamPresetId, TeamPresetInput, validate,
};

/// The hand-offs one person's message allows when a preset says nothing (ADR 0074 decision 8).
pub const DEFAULT_MAX_HANDOFFS: u32 = 6;

/// The lowest `maxHandoffs` a team may set, the default (ADR 0074 decision 8).
pub const MIN_MAX_HANDOFFS: u32 = DEFAULT_MAX_HANDOFFS;

/// The highest `maxHandoffs` a preset may set (ADR 0074 decision 8).
pub const MAX_HANDOFFS_CEILING: u32 = 20;

/// The fewest members a team may have (ADR 0074 decision 13).
pub const MIN_MEMBERS: usize = 2;

/// The most members a team may have (ADR 0074 decision 1).
pub const MAX_MEMBERS: usize = 8;

/// The most characters a member's role may have (ADR 0074 decision 13).
pub const MAX_ROLE_CHARS: usize = 280;

/// The most characters a member's name may have, the `@` not counted.
pub const MAX_NAME_CHARS: usize = 32;

/// A member's name: the label a person and an agent write as `@name`, unique in a thread.
///
/// Built by [`MemberName::new`], which is the only road in: an `@` may be written and is dropped,
/// and the rest is a letter, a digit, `-` or `_`. A value read back from a stored event is taken
/// as it was stored, which is why the field is not exposed.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MemberName(String);

impl MemberName {
    /// Reads a name, with or without the leading `@`.
    ///
    /// # Errors
    /// [`TeamRefusal::EmptyName`], [`TeamRefusal::NameTooLong`] or [`TeamRefusal::NameNotPlain`].
    pub fn new(raw: &str) -> Result<Self, TeamRefusal> {
        let name = raw.strip_prefix('@').unwrap_or(raw);
        if name.is_empty() {
            return Err(TeamRefusal::EmptyName);
        }
        if name.chars().count() > MAX_NAME_CHARS {
            return Err(TeamRefusal::NameTooLong);
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        {
            return Err(TeamRefusal::NameNotPlain);
        }
        Ok(MemberName(name.to_owned()))
    }

    /// The name without its `@`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for MemberName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One member of a team: a name, the agent behind it and its role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    /// The label, `@name` in the thread.
    pub name: MemberName,
    /// The agent this member runs.
    pub agent: AgentId,
    /// The role the lead and the other members are told about, at most [`MAX_ROLE_CHARS`].
    pub role: String,
}

impl Member {
    /// A member, its role checked against the bound.
    ///
    /// # Errors
    /// What [`MemberName::new`] refuses, and [`TeamRefusal::RoleTooLong`].
    pub fn new(
        name: impl AsRef<str>,
        agent: AgentId,
        role: impl Into<String>,
    ) -> Result<Self, TeamRefusal> {
        let name = MemberName::new(name.as_ref())?;
        let role = role.into();
        if role.chars().count() > MAX_ROLE_CHARS {
            return Err(TeamRefusal::RoleTooLong);
        }
        Ok(Member { name, agent, role })
    }
}

/// Why a team, a member or a cap was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TeamRefusal {
    /// The name was empty, or held only an `@`.
    #[error("a member name is empty")]
    EmptyName,
    /// The name was longer than [`MAX_NAME_CHARS`].
    #[error("a member name is longer than 32 characters")]
    NameTooLong,
    /// The name held a character other than a letter, a digit, `-` or `_`.
    #[error("a member name has a character other than a letter, a digit, `-` or `_`")]
    NameNotPlain,
    /// The role was longer than [`MAX_ROLE_CHARS`].
    #[error("a role is longer than 280 characters")]
    RoleTooLong,
    /// Fewer than [`MIN_MEMBERS`] members: a one-agent thread keeps no team state.
    #[error("a team has at least 2 members, and a thread of one agent keeps no team state")]
    TooFewMembers,
    /// More than [`MAX_MEMBERS`] members.
    #[error("a team has at most 8 members")]
    TooManyMembers,
    /// Two members had one name.
    #[error("two members have the same name")]
    DuplicateName,
    /// The lead was not among the members.
    #[error("the lead is not a member")]
    LeadNotMember,
    /// `maxHandoffs` was outside the range a preset may set.
    #[error("maxHandoffs is {0}, outside the 6 to 20 a preset may set")]
    HandoffCapOutOfRange(u32),
    /// The first event of a replay was not [`TeamEvent::Formed`].
    #[error("a team's log starts with team_formed")]
    NotFormed,
}

/// How a member's turn ended (ADR 0074 decision 6: completed, failed or stopped).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    /// The member finished its answer.
    Completed,
    /// The member failed.
    Failed,
    /// The member was stopped, or removed while its turn ran.
    Stopped,
}

/// The closed state of a team thread (ADR 0074 decision 15).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamState {
    /// Nothing is running and nothing waits for the person.
    Idle,
    /// At least one turn is running.
    Working,
    /// A hand-off waits for the person's answer.
    AwaitingApproval,
    /// A fan-out is running and the member that fanned out waits for its results.
    FanningIn,
    /// A checkpoint waits for the person to continue.
    AtCheckpoint,
    /// A hand-off went over `maxHandoffs`: nothing was routed, the floor is the person's.
    Capped,
}

/// What waits for the person, if anything (ADR 0074 decisions 10 and 11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Pending {
    /// A hand-off proposed by `from` to these members, waiting for the person's answer.
    Handoff {
        /// The member that proposed it.
        from: MemberName,
        /// The members it named, in the order it named them.
        to: Vec<MemberName>,
    },
    /// A checkpoint a member asked for, waiting for the person to continue.
    Checkpoint {
        /// The member that asked.
        member: MemberName,
    },
}

/// One event of a team's log, as the core folds it (ADR 0074 decisions 1, 3, 5, 6, 8 and 11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TeamEvent {
    /// The team was formed: the lead and its members, a copy of the preset.
    Formed {
        /// The member an unaddressed message goes to.
        lead: MemberName,
        /// The members, in the order the preset named them.
        members: Vec<Member>,
    },
    /// A member was added mid-chat.
    MemberAdded {
        /// The member.
        member: Member,
    },
    /// A member was removed mid-chat.
    MemberRemoved {
        /// The member's name.
        name: MemberName,
    },
    /// A member's turn started.
    TurnStarted {
        /// The member.
        member: MemberName,
    },
    /// A member's turn ended.
    TurnFinished {
        /// The member.
        member: MemberName,
        /// How it ended.
        outcome: TurnOutcome,
    },
    /// A member proposed a hand-off (the `hand_off` tool or the trailing line, `T1b`).
    HandoffProposed {
        /// The member that proposed it.
        from: MemberName,
        /// The members it named, in the order it named them.
        to: Vec<MemberName>,
    },
    /// The person answered a pending hand-off.
    HandoffDecided {
        /// Whether it goes; a hand-off that does not go returns the floor to the person.
        approved: bool,
    },
    /// The member that fanned out got its turn with the results.
    FanIn {
        /// That member.
        member: MemberName,
    },
    /// A hand-off went over `maxHandoffs` and was not routed.
    Capped {
        /// The members it named.
        refused: Vec<MemberName>,
    },
    /// A member asked for a checkpoint.
    Checkpoint {
        /// The member.
        member: MemberName,
    },
}

/// What the fold asks the caller to do, when an event changes the routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TeamCommand {
    /// Every fanned-out turn has ended: give the member that fanned out a turn with the results,
    /// each labelled by member (ADR 0074 decision 6).
    FanIn {
        /// The member that fanned out, now given the floor.
        from: MemberName,
        /// The members' results, by member name.
        results: Vec<(MemberName, TurnOutcome)>,
    },
    /// A hand-off went over the cap: it was not routed, and the floor is the person's.
    Capped {
        /// The members it named.
        refused: Vec<MemberName>,
    },
}

/// Where the floor goes after a person's message (ADR 0074 decision 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Floor {
    /// One member answers.
    Member(MemberName),
    /// These members answer in parallel, and their results go back to the person.
    FanOut(Vec<MemberName>),
    /// The message named more members than the cap allows: nothing was routed.
    Capped(Vec<MemberName>),
    /// The person must act: there is no lead to route to, or nothing was routable.
    Person,
}

/// A fan-out whose member is waiting for its results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanIn {
    /// The member that fanned out.
    pub from: MemberName,
    /// The members whose turns have not ended yet.
    pub waiting: BTreeSet<MemberName>,
    /// The members whose turns have ended, by name.
    pub ended: BTreeMap<MemberName, TurnOutcome>,
}

/// The team part of a job ledger: members, running turns, the count, the pending question and the
/// state (ADR 0074 decision 15).
///
/// Built by [`TeamLedger::formed`], changed by [`TeamLedger::apply`] and read by whoever renders
/// the brief or the projection. It holds no I/O and no clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamLedger {
    lead: MemberName,
    members: Vec<Member>,
    running: BTreeSet<MemberName>,
    fan_in: Option<FanIn>,
    count: u32,
    cap: u32,
    state: TeamState,
    pending: Option<Pending>,
}

impl TeamLedger {
    /// A team as it is formed.
    ///
    /// # Errors
    /// [`TeamRefusal::TooFewMembers`] under [`MIN_MEMBERS`] members (a thread of one agent keeps no
    /// team state), [`TeamRefusal::TooManyMembers`] above [`MAX_MEMBERS`],
    /// [`TeamRefusal::DuplicateName`], [`TeamRefusal::LeadNotMember`], and what
    /// [`TeamLedger::cap_for`] refuses.
    pub fn formed(
        lead: MemberName,
        members: Vec<Member>,
        max_handoffs: Option<u32>,
    ) -> Result<Self, TeamRefusal> {
        if members.len() < MIN_MEMBERS {
            return Err(TeamRefusal::TooFewMembers);
        }
        if members.len() > MAX_MEMBERS {
            return Err(TeamRefusal::TooManyMembers);
        }
        let names: BTreeSet<&MemberName> = members.iter().map(|m| &m.name).collect();
        if names.len() != members.len() {
            return Err(TeamRefusal::DuplicateName);
        }
        if !members.iter().any(|m| m.name == lead) {
            return Err(TeamRefusal::LeadNotMember);
        }
        Ok(TeamLedger {
            lead,
            members,
            running: BTreeSet::new(),
            fan_in: None,
            count: 0,
            cap: Self::cap_for(max_handoffs)?,
            state: TeamState::Idle,
            pending: None,
        })
    }

    /// The cap a preset's `maxHandoffs` means, or the default when it says nothing.
    ///
    /// # Errors
    /// [`TeamRefusal::HandoffCapOutOfRange`] outside [`MIN_MAX_HANDOFFS`]..=[`MAX_HANDOFFS_CEILING`].
    pub fn cap_for(max_handoffs: Option<u32>) -> Result<u32, TeamRefusal> {
        match max_handoffs {
            None => Ok(DEFAULT_MAX_HANDOFFS),
            Some(v) if (MIN_MAX_HANDOFFS..=MAX_HANDOFFS_CEILING).contains(&v) => Ok(v),
            Some(v) => Err(TeamRefusal::HandoffCapOutOfRange(v)),
        }
    }

    /// Folds a whole log from its first event, which must be [`TeamEvent::Formed`].
    ///
    /// The commands the fold would issue are not returned: a replay rebuilds the state, and only
    /// the live path dispatches.
    ///
    /// # Errors
    /// [`TeamRefusal::NotFormed`], or what [`TeamLedger::formed`] refuses.
    pub fn replay(events: &[TeamEvent]) -> Result<Self, TeamRefusal> {
        let mut it = events.iter();
        let Some(TeamEvent::Formed { lead, members }) = it.next() else {
            return Err(TeamRefusal::NotFormed);
        };
        let mut ledger = TeamLedger::formed(lead.clone(), members.clone(), None)?;
        for event in it {
            ledger.apply(event.clone());
        }
        Ok(ledger)
    }

    /// The member an unaddressed message goes to.
    pub fn lead(&self) -> &MemberName {
        &self.lead
    }

    /// The members, in the order they were formed.
    pub fn members(&self) -> &[Member] {
        &self.members
    }

    /// The members whose turn is running.
    pub fn running(&self) -> &BTreeSet<MemberName> {
        &self.running
    }

    /// The fan-out waiting for its results, when one is.
    pub fn fan_in(&self) -> Option<&FanIn> {
        self.fan_in.as_ref()
    }

    /// The hand-offs counted against the person's last message.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// The cap in force.
    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// The state the thread is in because of the team.
    pub fn state(&self) -> TeamState {
        self.state
    }

    /// What waits for the person, when something does.
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// The member called `name`, when one is.
    pub fn member(&self, name: &MemberName) -> Option<&Member> {
        self.members.iter().find(|m| &m.name == name)
    }

    /// Where a person's message goes, and how it is counted.
    ///
    /// A message that names nobody goes to the lead. One that names members goes to each of them
    /// (a fan-out when there are two or more, whose results go back to the person), and a fan-out
    /// to N counts N against the cap. The message ends a pending hand-off or checkpoint and starts
    /// the count again; turns already running are left alone.
    pub fn route_person_message(&mut self, mentions: &[MemberName], max_handoffs: u32) -> Floor {
        self.cap = max_handoffs.clamp(MIN_MAX_HANDOFFS, MAX_HANDOFFS_CEILING);
        self.count = 0;
        self.pending = None;
        self.state = if self.running.is_empty() {
            TeamState::Idle
        } else {
            TeamState::Working
        };

        let named: Vec<MemberName> = {
            let mut seen = BTreeSet::new();
            mentions
                .iter()
                .filter(|name| self.member(name).is_some() && seen.insert((*name).clone()))
                .cloned()
                .collect()
        };

        match named.len() {
            0 => {
                if self.member(&self.lead).is_some() {
                    self.state = TeamState::Working;
                    Floor::Member(self.lead.clone())
                } else {
                    Floor::Person
                }
            }
            1 => {
                self.count = 1;
                self.state = TeamState::Working;
                // `named` had exactly one member, so this cannot fall back; the lead is only a
                // placeholder that keeps the fold total.
                Floor::Member(
                    named
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| self.lead.clone()),
                )
            }
            n => {
                let n32 = u32::try_from(n).unwrap_or(u32::MAX);
                self.count = n32;
                if n32 > self.cap {
                    self.state = TeamState::Capped;
                    Floor::Capped(named)
                } else {
                    self.state = TeamState::Working;
                    Floor::FanOut(named)
                }
            }
        }
    }

    /// Folds one event, answering what it asks the caller to do.
    ///
    /// Total: an event a ledger would not accept was refused before it was written.
    pub fn apply(&mut self, event: TeamEvent) -> Option<TeamCommand> {
        match event {
            // The ledger is formed already: `Formed` is what `formed` read.
            TeamEvent::Formed { .. } => None,

            TeamEvent::MemberAdded { member } => {
                if self.member(&member.name).is_none() && self.members.len() < MAX_MEMBERS {
                    self.members.push(member);
                }
                None
            }

            TeamEvent::MemberRemoved { name } => self.remove(&name),

            TeamEvent::TurnStarted { member } => {
                if self.member(&member).is_some() {
                    self.running.insert(member);
                    if self.state != TeamState::FanningIn {
                        self.state = TeamState::Working;
                    }
                }
                None
            }

            TeamEvent::TurnFinished { member, outcome } => self.finished(&member, outcome),

            TeamEvent::HandoffProposed { from, to } => self.proposed(&from, to),

            TeamEvent::HandoffDecided { approved } => self.decided(approved),

            TeamEvent::FanIn { member } => {
                if self.member(&member).is_some() {
                    self.running.insert(member);
                    self.state = TeamState::Working;
                }
                None
            }

            TeamEvent::Capped { refused } => {
                self.pending = None;
                self.state = TeamState::Capped;
                Some(TeamCommand::Capped { refused })
            }

            TeamEvent::Checkpoint { member } => {
                self.pending = Some(Pending::Checkpoint { member });
                self.state = TeamState::AtCheckpoint;
                None
            }
        }
    }

    fn remove(&mut self, name: &MemberName) -> Option<TeamCommand> {
        self.members.retain(|m| &m.name != name);
        self.running.remove(name);
        // Its pending hand-off goes with it, and the floor is the person's.
        let drops_pending = match &self.pending {
            Some(Pending::Handoff { to, .. }) => to.iter().any(|n| n == name),
            _ => false,
        };
        if drops_pending {
            self.pending = None;
            self.state = TeamState::Idle;
            return None;
        }
        // A turn of it that was running counts as stopped, so a fan-in it was part of can
        // complete (ADR 0074 decision 14).
        let done = self.fan_in.as_mut().map(|fan| {
            let was_waiting = fan.waiting.remove(name);
            if was_waiting {
                fan.ended.insert(name.clone(), TurnOutcome::Stopped);
            }
            fan.waiting.is_empty()
        });
        if done == Some(true) {
            return self.take_fan_in();
        }
        None
    }

    fn finished(&mut self, member: &MemberName, outcome: TurnOutcome) -> Option<TeamCommand> {
        self.running.remove(member);
        let done = self.fan_in.as_mut().map(|fan| {
            let was_waiting = fan.waiting.remove(member);
            if was_waiting {
                fan.ended.insert(member.clone(), outcome);
            }
            fan.waiting.is_empty()
        });
        if done == Some(true) {
            return self.take_fan_in();
        }
        if self.state != TeamState::FanningIn
            && self.state != TeamState::AtCheckpoint
            && self.running.is_empty()
            && self.pending.is_none()
        {
            self.state = TeamState::Idle;
        }
        None
    }

    fn take_fan_in(&mut self) -> Option<TeamCommand> {
        let fan = self.fan_in.take()?;
        let results = fan.ended.into_iter().collect();
        // The fan-in turn is what runs next: the thread is working again (ADR 0074 decision 6).
        self.state = TeamState::Working;
        Some(TeamCommand::FanIn {
            from: fan.from,
            results,
        })
    }

    fn proposed(&mut self, from: &MemberName, to: Vec<MemberName>) -> Option<TeamCommand> {
        // A proposal by somebody who is not a member is not a hand-off.
        self.member(from)?;
        // A name that is not a member, or the member itself, is no hand-off: the floor goes back
        // to the person (ADR 0074 decision 5).
        let valid = !to.is_empty()
            && to
                .iter()
                .all(|name| name != from && self.member(name).is_some());
        if !valid {
            self.pending = None;
            self.state = TeamState::Idle;
            return None;
        }
        let n = u32::try_from(to.len()).unwrap_or(u32::MAX);
        let next = self.count.saturating_add(n);
        if next > self.cap {
            // The count is what was routed, so a refused hand-off does not move it (the property
            // `never_more_than_max_handoffs_per_person_message` found this).
            self.pending = None;
            self.state = TeamState::Capped;
            return Some(TeamCommand::Capped { refused: to });
        }
        self.count = next;
        self.pending = Some(Pending::Handoff {
            from: from.clone(),
            to,
        });
        self.state = TeamState::AwaitingApproval;
        None
    }

    fn decided(&mut self, approved: bool) -> Option<TeamCommand> {
        let Some(Pending::Handoff { from, to }) = self.pending.take() else {
            return None;
        };
        if !approved {
            self.state = TeamState::Idle;
            return None;
        }
        if to.len() > 1 {
            self.fan_in = Some(FanIn {
                from,
                waiting: to.iter().cloned().collect(),
                ended: BTreeMap::new(),
            });
            self.state = TeamState::FanningIn;
        } else {
            // A hand-off to one member passes the floor; nothing comes back.
            self.state = TeamState::Working;
        }
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn member(name: &str) -> Member {
        Member::new(name, AgentId::new(format!("agent-{name}")), "does its part").unwrap()
    }

    fn team(names: &[&str], cap: Option<u32>) -> TeamLedger {
        let members: Vec<Member> = names.iter().map(|n| member(n)).collect();
        let lead = members[0].name.clone();
        TeamLedger::formed(lead, members, cap).unwrap()
    }

    fn name(raw: &str) -> MemberName {
        MemberName::new(raw).unwrap()
    }

    #[test]
    fn a_name_takes_an_at_and_keeps_letters_digits_dash_and_underscore() {
        assert_eq!(name("@coder-a").as_str(), "coder-a");
        assert_eq!(name("coder_b2").as_str(), "coder_b2");
        assert_eq!(MemberName::new("@").unwrap_err(), TeamRefusal::EmptyName);
        assert_eq!(
            MemberName::new("a b").unwrap_err(),
            TeamRefusal::NameNotPlain
        );
        assert_eq!(
            MemberName::new(&"x".repeat(MAX_NAME_CHARS + 1)).unwrap_err(),
            TeamRefusal::NameTooLong
        );
    }

    #[test]
    fn a_team_of_one_agent_keeps_no_state_and_the_bounds_are_refused() {
        let one = vec![member("lead")];
        assert_eq!(
            TeamLedger::formed(one[0].name.clone(), one, None).unwrap_err(),
            TeamRefusal::TooFewMembers
        );
        let nine: Vec<Member> = (0..9).map(|i| member(&format!("m{i}"))).collect();
        assert_eq!(
            TeamLedger::formed(nine[0].name.clone(), nine, None).unwrap_err(),
            TeamRefusal::TooManyMembers
        );
        assert_eq!(
            TeamLedger::cap_for(Some(MAX_HANDOFFS_CEILING + 1)).unwrap_err(),
            TeamRefusal::HandoffCapOutOfRange(MAX_HANDOFFS_CEILING + 1)
        );
        assert_eq!(
            TeamLedger::cap_for(Some(MIN_MAX_HANDOFFS - 1)).unwrap_err(),
            TeamRefusal::HandoffCapOutOfRange(MIN_MAX_HANDOFFS - 1)
        );
        assert_eq!(TeamLedger::cap_for(None).unwrap(), DEFAULT_MAX_HANDOFFS);
        let two = vec![member("a"), member("a")];
        assert_eq!(
            TeamLedger::formed(two[0].name.clone(), two, None).unwrap_err(),
            TeamRefusal::DuplicateName
        );
        let members = vec![member("a"), member("b")];
        assert_eq!(
            TeamLedger::formed(name("elsewhere"), members, None).unwrap_err(),
            TeamRefusal::LeadNotMember
        );
    }

    #[test]
    fn an_unaddressed_message_goes_to_the_lead_and_a_named_one_to_each_member() {
        let mut ledger = team(&["lead", "a", "b"], Some(6));
        assert_eq!(
            ledger.route_person_message(&[], 6),
            Floor::Member(name("lead"))
        );
        assert_eq!(
            ledger.route_person_message(&[name("a"), name("b")], 6),
            Floor::FanOut(vec![name("a"), name("b")])
        );
        assert_eq!(ledger.count(), 2);
        assert_eq!(
            ledger.route_person_message(&[name("a")], 6),
            Floor::Member(name("a"))
        );
        assert_eq!(ledger.count(), 1);
        // A name that is nobody is not routed to anybody but the lead.
        assert_eq!(
            ledger.route_person_message(&[name("ghost")], 6),
            Floor::Member(name("lead"))
        );
    }

    #[test]
    fn a_person_message_ends_a_pending_handoff_and_starts_the_count_again() {
        let mut ledger = team(&["lead", "a", "b"], Some(6));
        let _ = ledger.route_person_message(&[], 6);
        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("a")],
        });
        assert_eq!(ledger.state(), TeamState::AwaitingApproval);
        assert!(ledger.pending().is_some());
        let _ = ledger.route_person_message(&[], 6);
        assert!(ledger.pending().is_none());
        assert_eq!(ledger.count(), 0);
    }

    #[test]
    fn a_handoff_to_one_passes_the_floor_and_to_several_fans_out() {
        let mut ledger = team(&["lead", "a", "b"], Some(6));
        let _ = ledger.route_person_message(&[], 6);
        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("a")],
        });
        ledger.apply(TeamEvent::HandoffDecided { approved: true });
        assert_eq!(ledger.state(), TeamState::Working);
        assert!(ledger.fan_in().is_none());

        let mut ledger = team(&["lead", "a", "b"], Some(6));
        let _ = ledger.route_person_message(&[], 6);
        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("a"), name("b")],
        });
        ledger.apply(TeamEvent::HandoffDecided { approved: true });
        assert_eq!(ledger.state(), TeamState::FanningIn);
        assert_eq!(ledger.fan_in().unwrap().waiting.len(), 2);
    }

    #[test]
    fn a_handoff_to_a_stranger_or_to_itself_is_no_handoff() {
        let mut ledger = team(&["lead", "a"], Some(6));
        let _ = ledger.route_person_message(&[], 6);
        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("ghost")],
        });
        assert_eq!(ledger.state(), TeamState::Idle);
        assert!(ledger.pending().is_none());

        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("lead")],
        });
        assert!(ledger.pending().is_none());
    }

    #[test]
    fn a_removed_member_takes_its_pending_handoff_and_its_turn_ends_stopped() {
        let mut ledger = team(&["lead", "a", "b"], Some(6));
        let _ = ledger.route_person_message(&[], 6);
        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("a")],
        });
        ledger.apply(TeamEvent::MemberRemoved { name: name("a") });
        assert_eq!(ledger.state(), TeamState::Idle);
        assert!(ledger.pending().is_none());
        assert_eq!(ledger.members().len(), 2);

        // A fan-out whose member is removed still completes, with that member stopped.
        let mut ledger = team(&["lead", "a", "b"], Some(6));
        let _ = ledger.route_person_message(&[], 6);
        ledger.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("a"), name("b")],
        });
        ledger.apply(TeamEvent::HandoffDecided { approved: true });
        ledger.apply(TeamEvent::TurnFinished {
            member: name("a"),
            outcome: TurnOutcome::Completed,
        });
        let command = ledger.apply(TeamEvent::MemberRemoved { name: name("b") });
        match command {
            Some(TeamCommand::FanIn { from, results }) => {
                assert_eq!(from, name("lead"));
                assert_eq!(
                    results,
                    vec![
                        (name("a"), TurnOutcome::Completed),
                        (name("b"), TurnOutcome::Stopped)
                    ]
                );
            }
            other => panic!("the fan-in should complete once b is gone, got {other:?}"),
        }
    }

    #[test]
    fn a_replay_rebuilds_the_same_ledger() {
        let mut live = team(&["lead", "a", "b"], Some(6));
        let _ = live.route_person_message(&[], 6);
        live.apply(TeamEvent::HandoffProposed {
            from: name("lead"),
            to: vec![name("a")],
        });
        live.apply(TeamEvent::HandoffDecided { approved: true });
        live.apply(TeamEvent::TurnStarted { member: name("a") });

        let log = vec![
            TeamEvent::Formed {
                lead: name("lead"),
                members: vec![member("lead"), member("a"), member("b")],
            },
            TeamEvent::HandoffProposed {
                from: name("lead"),
                to: vec![name("a")],
            },
            TeamEvent::HandoffDecided { approved: true },
            TeamEvent::TurnStarted { member: name("a") },
        ];
        let replayed = TeamLedger::replay(&log).unwrap();
        assert_eq!(replayed.members(), live.members());
        assert_eq!(replayed.state(), live.state());
        assert_eq!(replayed.running(), live.running());
        assert_eq!(replayed.count(), live.count());
        assert_eq!(
            TeamLedger::replay(&log[1..]).unwrap_err(),
            TeamRefusal::NotFormed
        );
    }

    proptest! {
        /// The decisive case of ADR 0074 decision 8: no person's message ever starts more turns
        /// than `maxHandoffs`, a fan-out to N counts N, and nothing over the cap is routed.
        ///
        /// Seen red by deleting the cap check in `TeamLedger::proposed`: `count` then passes the
        /// cap and the assertion below fails.
        #[test]
        fn never_more_than_max_handoffs_per_person_message(
            cap in MIN_MAX_HANDOFFS..=MAX_HANDOFFS_CEILING,
            targets in proptest::collection::vec(1usize..=3, 1..12),
            people in proptest::collection::vec(0usize..3, 0..8),
        ) {
            let mut ledger = team(&["lead", "a", "b"], Some(cap));
            let names = [name("a"), name("b"), name("lead")];
            let mut routed: u32 = 0;

            for n in &targets {
                let started_before = ledger.running().len();
                let to: Vec<MemberName> = names
                    .iter()
                    .filter(|candidate| **candidate != name("lead"))
                    .cycle()
                    .take(*n)
                    .cloned()
                    .collect();

                let command = ledger.apply(TeamEvent::HandoffProposed {
                    from: name("lead"),
                    to: to.clone(),
                });

                let n32 = u32::try_from(to.len()).unwrap_or(u32::MAX);
                if routed + n32 > cap {
                    // Over the cap: nothing routed, and the ledger says so.
                    prop_assert!(command.is_some());
                    prop_assert_eq!(ledger.state(), TeamState::Capped);
                    prop_assert!(ledger.pending().is_none());
                    prop_assert_eq!(ledger.running().len(), started_before);
                } else {
                    routed += n32;
                    prop_assert_eq!(ledger.count(), routed);
                    prop_assert!(command.is_none());
                }
                prop_assert!(ledger.count() <= cap);

                // A person's message starts the count again.
                if people.contains(&0) {
                    let floor = ledger.route_person_message(&[], cap);
                    prop_assert_eq!(floor, Floor::Member(name("lead")));
                    prop_assert_eq!(ledger.count(), 0);
                    routed = 0;
                }
            }
        }

        /// The decisive case of ADR 0074 decision 6: the fan-in turn starts only once every
        /// fanned-out turn has ended, and exactly once.
        ///
        /// Seen red by deleting the `fan.waiting.is_empty()` guard in `TeamLedger::finished`:
        /// the command then appears on the first finish, which the assertion refuses.
        #[test]
        fn fan_in_starts_only_after_every_fanned_out_turn_ended(
            fan_out in 2usize..=3,
            order in proptest::collection::vec(0usize..6, 1..14),
        ) {
            let names = ["a", "b", "c"];
            let mut ledger = team(&["lead", "a", "b", "c"], Some(MAX_HANDOFFS_CEILING));
            let _ = ledger.route_person_message(&[], MAX_HANDOFFS_CEILING);
            let to: Vec<MemberName> = names[..fan_out].iter().map(|n| name(n)).collect();
            ledger.apply(TeamEvent::HandoffProposed {
                from: name("lead"),
                to: to.clone(),
            });
            ledger.apply(TeamEvent::HandoffDecided { approved: true });

            let mut ended: BTreeSet<usize> = BTreeSet::new();
            let mut emitted = 0;
            for index in order {
                let slot = index % fan_out;
                ended.insert(slot);
                let command = ledger.apply(TeamEvent::TurnFinished {
                    member: to[slot].clone(),
                    outcome: TurnOutcome::Completed,
                });
                let all_ended = ended.len() == fan_out;

                match command {
                    Some(TeamCommand::FanIn { from, results }) => {
                        prop_assert!(all_ended);
                        prop_assert_eq!(emitted, 0);
                        prop_assert_eq!(from, name("lead"));
                        prop_assert_eq!(results.len(), fan_out);
                        emitted += 1;
                    }
                    Some(TeamCommand::Capped { .. }) => prop_assert!(false, "no cap here"),
                    None => prop_assert!(!all_ended || emitted > 0),
                }
                // However many finishes arrive, the fan-in is issued at most once.
                prop_assert!(emitted <= 1);
            }
        }
    }
}

//! The mission core: its events, its states and the transitions between them (ADR 0095,
//! `docs/api/missions-v1.md`, slice 3a).
//!
//! A [`Mission`] is a pure fold of [`MissionEvent`]s: [`Mission::replay`] folds a log and
//! [`Mission::apply`] takes one more event or refuses it and changes nothing. It does no I/O,
//! reads no clock and is not wired into `transition` yet: no route, no store, no variant of an
//! existing enum. The limit and the team are data ([`MissionLimit`], [`MissionTeam`]); judging
//! them against a person's allowance is ADR 0081's.
//!
//! Transitions (anything else is [`MissionRefusal::InvalidTransition`]; `Done` and `Stopped`
//! accept nothing):
//!
//! | From | Event | To | Also needs |
//! |---|---|---|---|
//! | `Drafted` | `plan_proposed` | `Drafted` | nothing |
//! | `Drafted` | `plan_approved` | `Planned` | by the person; the proposed version and digest |
//! | `Planned` | `started` | `Running` | by the person |
//! | `Running` | `waiting` | `Waiting` | nothing |
//! | `Waiting` | `resumed` | `Running` | by the person or the core |
//! | `Paused` | `resumed` | `Running` | by the person |
//! | `Running`, `Waiting` | `paused` | `Paused` | by the person |
//! | `Drafted`, `Planned`, `Running`, `Waiting`, `Paused` | `stopped` (person) | `Stopped` | by the person |
//! | `Running` | `stopped` (limit spent) | `Stopped` | by the core; the observed spend has reached the limit |
//! | `Running` | `done` | `Done` | every done check passed on this very tip |
//! | not `Done`, not `Stopped` | `spend_observed`, | same | nothing |
//! | `Running` | `check_reported` | `Running` | a check of the definition of done |
//!
//! Open (the ADR does not say, so the narrowest reading is implemented):
//!
//! * The limit stops a mission only from `Running` (the diagram), not from `Waiting` or `Paused`.
//! * A pause drops what the mission waited for; the ADR keeps `Paused` to `Running` only.
//! * A new plan version after approval (decision 4) is not modelled: a plan is proposed and
//!   approved once, while `Drafted`. A plan is only its version and digest here, not its tasks.
//! * The ADR names no event for the result of a done check or for the integration tip, so
//!   `check_reported` (check index, tip, passed) is added; the newest result of a check wins and
//!   `done` needs all of them to pass on the one tip it names. That the tip is pushed is the
//!   reporter's to attest, not modelled.
//! * Tasks, reservations and the verifier (`mission_task_*`, `mission_budget_*`) are slices 4 and 5.
//! * `repositories` are not modelled: no transition depends on them.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The most bytes of a mission's goal.
pub const MAX_MISSION_GOAL_BYTES: usize = 8_192;
/// The fewest members of a mission's team.
pub const MIN_MISSION_MEMBERS: usize = 2;
/// The most members of a mission's team.
pub const MAX_MISSION_MEMBERS: usize = 8;
/// The most characters of a member's role.
pub const MAX_MISSION_ROLE_CHARS: usize = 280;
/// The most checks of a definition of done.
pub const MAX_DONE_CHECKS: usize = 8;

/// Where a mission stands, closed (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionState {
    /// The object exists; no plan is approved.
    Drafted,
    /// A plan is approved; nothing is dispatched.
    Planned,
    /// Work is dispatched or ready to dispatch.
    Running,
    /// It needs something it cannot get by itself.
    Waiting,
    /// The person said stop for now.
    Paused,
    /// The definition of done passed. Terminal.
    Done,
    /// The person or the limit ended it. Terminal.
    Stopped,
}

impl MissionState {
    /// Whether no event leaves the state.
    pub fn is_terminal(self) -> bool {
        matches!(self, MissionState::Done | MissionState::Stopped)
    }
}

/// Who an event is the act of, closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionActor {
    /// The mission's owner.
    Person,
    /// The lead agent.
    Lead,
    /// The orchestrator's own core.
    Core,
}

/// Why a mission waits, closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitReason {
    /// A question to the person.
    Question,
    /// A task failed twice.
    NeedsPerson,
    /// A limit parked a task.
    LimitParked,
    /// A verifier is down.
    VerifierDown,
}

/// Why a mission stopped, closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The person stopped it.
    Person,
    /// The mission's limit is spent.
    LimitSpent,
}

/// The mission's limit, as data: all three numbers are required and above zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionLimit {
    /// Tokens over the whole mission.
    pub tokens: u64,
    /// Seconds of wall clock.
    pub wall_clock_secs: u64,
    /// Tasks that may run together.
    pub tasks_at_once: u32,
}

/// One member of a mission's team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionMember {
    /// The member's name in the team.
    pub name: String,
    /// The agent that plays it.
    pub agent: String,
    /// What it does, at most [`MAX_MISSION_ROLE_CHARS`] characters.
    pub role: String,
}

/// A mission's team, as data: two to eight members and a lead among them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MissionTeam {
    /// The members.
    pub members: Vec<MissionMember>,
    /// The name of the lead.
    pub lead: String,
}

/// One check of the definition of done (ADR 0095 decision 5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DoneCheck {
    /// Named required check runs of a repository.
    Ci {
        /// The repository.
        repo: String,
        /// The check runs that must pass.
        checks: Vec<String>,
    },
    /// An argv the verifier runs.
    Command {
        /// The program and its arguments; never a shell string.
        argv: Vec<String>,
    },
    /// A verifier's verdict for the whole mission.
    Verdict {
        /// The verifier agent.
        agent: String,
    },
}

/// Why a mission cannot be drafted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DraftError {
    /// The goal is empty or too long.
    #[error("goal: {0}")]
    Goal(&'static str),
    /// The limit lacks a number.
    #[error("limit.{0} must be above zero")]
    Limit(&'static str),
    /// The team breaks a rule.
    #[error("team: {0}")]
    Team(&'static str),
    /// The definition of done breaks a rule.
    #[error("done: {0}")]
    Done(&'static str),
}

/// Why an event is not taken, closed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MissionRefusal {
    /// The log does not begin with `mission_drafted`, or begins it twice.
    #[error("a mission starts with mission_drafted, once")]
    NotDrafted,
    /// The mission cannot be drafted.
    #[error(transparent)]
    Draft(#[from] DraftError),
    /// The event does not apply in this state.
    #[error("{event} does not apply to a {from:?} mission")]
    InvalidTransition {
        /// The state the mission is in.
        from: MissionState,
        /// The event's name.
        event: &'static str,
    },
    /// The event is the person's, or the core's, alone.
    #[error("{event} is not this actor's to make")]
    WrongActor {
        /// The event's name.
        event: &'static str,
    },
    /// The approval is not of the proposed plan.
    #[error("the plan approved is not the plan proposed")]
    PlanMismatch,
    /// The check is not in the definition of done.
    #[error("no check {0} in the definition of done")]
    UnknownCheck(usize),
    /// Not every check passed on this tip.
    #[error("every check of the definition of done must pass on the tip")]
    DoneNotProven,
    /// The observed spend has not reached the limit.
    #[error("the limit is not spent")]
    LimitNotSpent,
}

/// A fact of a mission's log, closed (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MissionEvent {
    /// `mission_drafted`: the object, with its limit, team and definition of done.
    Drafted {
        /// The goal, as data.
        goal: String,
        /// The limit.
        limit: MissionLimit,
        /// The team, copied.
        team: MissionTeam,
        /// The definition of done.
        done: Vec<DoneCheck>,
    },
    /// `mission_plan_proposed`: the lead's plan, by its version and digest.
    PlanProposed {
        /// The plan's version.
        version: u32,
        /// The plan's digest.
        digest: String,
    },
    /// `mission_plan_approved`: the person freezes the proposed plan.
    PlanApproved {
        /// The version approved.
        version: u32,
        /// The digest approved.
        digest: String,
        /// Who approved.
        by: MissionActor,
    },
    /// `mission_started`: the person starts it.
    Started {
        /// Who started.
        by: MissionActor,
    },
    /// `mission_waiting`.
    Waiting {
        /// What it waits for.
        reason: WaitReason,
    },
    /// `mission_resumed`.
    Resumed {
        /// Who resumed.
        by: MissionActor,
    },
    /// `mission_paused`.
    Paused {
        /// Who paused.
        by: MissionActor,
    },
    /// `mission_stopped`.
    Stopped {
        /// Why.
        reason: StopReason,
        /// Who stopped.
        by: MissionActor,
    },
    /// The totals the usage ledger and the log show now.
    SpendObserved {
        /// Tokens spent.
        tokens: u64,
        /// Seconds of wall clock spent.
        wall_clock_secs: u64,
    },
    /// The result of one check of the definition of done on an integration tip.
    CheckReported {
        /// The index in the definition of done.
        check: usize,
        /// The integration tip it ran on.
        tip: String,
        /// Whether it passed.
        passed: bool,
    },
    /// `mission_done`: accepted only if every check passed on `tip`.
    Done {
        /// The integration tip.
        tip: String,
    },
}

impl MissionEvent {
    /// The event's name in the log.
    pub fn name(&self) -> &'static str {
        match self {
            MissionEvent::Drafted { .. } => "mission_drafted",
            MissionEvent::PlanProposed { .. } => "mission_plan_proposed",
            MissionEvent::PlanApproved { .. } => "mission_plan_approved",
            MissionEvent::Started { .. } => "mission_started",
            MissionEvent::Waiting { .. } => "mission_waiting",
            MissionEvent::Resumed { .. } => "mission_resumed",
            MissionEvent::Paused { .. } => "mission_paused",
            MissionEvent::Stopped { .. } => "mission_stopped",
            MissionEvent::SpendObserved { .. } => "mission_spend_observed",
            MissionEvent::CheckReported { .. } => "mission_check_reported",
            MissionEvent::Done { .. } => "mission_done",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlanSlot {
    None,
    Proposed { version: u32, digest: String },
    Approved { version: u32, digest: String },
}

/// A mission: the fold of its events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission {
    state: MissionState,
    goal: String,
    limit: MissionLimit,
    team: MissionTeam,
    done: Vec<DoneCheck>,
    plan: PlanSlot,
    checks: BTreeMap<usize, (String, bool)>,
    spent_tokens: u64,
    spent_wall_clock_secs: u64,
    waiting_for: Option<WaitReason>,
    stopped: Option<StopReason>,
    done_tip: Option<String>,
}

fn invalid(from: MissionState, event: &MissionEvent) -> MissionRefusal {
    MissionRefusal::InvalidTransition {
        from,
        event: event.name(),
    }
}

fn by(
    actor: MissionActor,
    allowed: &[MissionActor],
    event: &MissionEvent,
) -> Result<(), MissionRefusal> {
    if allowed.contains(&actor) {
        Ok(())
    } else {
        Err(MissionRefusal::WrongActor {
            event: event.name(),
        })
    }
}

fn check_draft(
    goal: &str,
    limit: &MissionLimit,
    team: &MissionTeam,
    done: &[DoneCheck],
) -> Result<(), DraftError> {
    if goal.trim().is_empty() {
        return Err(DraftError::Goal("must not be empty"));
    }
    if goal.len() > MAX_MISSION_GOAL_BYTES {
        return Err(DraftError::Goal("is too long"));
    }
    if limit.tokens == 0 {
        return Err(DraftError::Limit("tokens"));
    }
    if limit.wall_clock_secs == 0 {
        return Err(DraftError::Limit("wall_clock_secs"));
    }
    if limit.tasks_at_once == 0 {
        return Err(DraftError::Limit("tasks_at_once"));
    }
    let n = team.members.len();
    if !(MIN_MISSION_MEMBERS..=MAX_MISSION_MEMBERS).contains(&n) {
        return Err(DraftError::Team("needs two to eight members"));
    }
    for (i, m) in team.members.iter().enumerate() {
        if m.name.trim().is_empty() || m.agent.trim().is_empty() {
            return Err(DraftError::Team("a member needs a name and an agent"));
        }
        if m.role.chars().count() > MAX_MISSION_ROLE_CHARS {
            return Err(DraftError::Team("a role is too long"));
        }
        if team.members[..i].iter().any(|o| o.name == m.name) {
            return Err(DraftError::Team("a name is repeated"));
        }
    }
    if !team.members.iter().any(|m| m.name == team.lead) {
        return Err(DraftError::Team("the lead is not a member"));
    }
    if done.is_empty() {
        return Err(DraftError::Done("must name at least one check"));
    }
    if done.len() > MAX_DONE_CHECKS {
        return Err(DraftError::Done("has too many checks"));
    }
    for c in done {
        let ok = match c {
            DoneCheck::Ci { repo, checks } => {
                !repo.trim().is_empty()
                    && !checks.is_empty()
                    && checks.iter().all(|s| !s.trim().is_empty())
            }
            DoneCheck::Command { argv } => argv.first().is_some_and(|p| !p.trim().is_empty()),
            DoneCheck::Verdict { agent } => !agent.trim().is_empty(),
        };
        if !ok {
            return Err(DraftError::Done("a check is empty"));
        }
    }
    Ok(())
}

impl Mission {
    /// Starts a mission from its first event, which must be `mission_drafted` and valid.
    pub fn from_event(event: &MissionEvent) -> Result<Mission, MissionRefusal> {
        let MissionEvent::Drafted {
            goal,
            limit,
            team,
            done,
        } = event
        else {
            return Err(MissionRefusal::NotDrafted);
        };
        check_draft(goal, limit, team, done)?;
        Ok(Mission {
            state: MissionState::Drafted,
            goal: goal.clone(),
            limit: *limit,
            team: team.clone(),
            done: done.clone(),
            plan: PlanSlot::None,
            checks: BTreeMap::new(),
            spent_tokens: 0,
            spent_wall_clock_secs: 0,
            waiting_for: None,
            stopped: None,
            done_tip: None,
        })
    }

    /// Folds a whole log; the first refusal ends it.
    pub fn replay(events: &[MissionEvent]) -> Result<Mission, MissionRefusal> {
        let Some((first, rest)) = events.split_first() else {
            return Err(MissionRefusal::NotDrafted);
        };
        let mut mission = Mission::from_event(first)?;
        for event in rest {
            mission.apply(event)?;
        }
        Ok(mission)
    }

    /// Takes one event, or refuses it and leaves the mission as it was.
    pub fn apply(&mut self, event: &MissionEvent) -> Result<(), MissionRefusal> {
        use MissionActor::{Core, Person};
        let from = self.state;
        if from.is_terminal() {
            return Err(invalid(from, event));
        }
        match event {
            MissionEvent::Drafted { .. } => return Err(invalid(from, event)),
            MissionEvent::PlanProposed { version, digest } => {
                if from != MissionState::Drafted {
                    return Err(invalid(from, event));
                }
                self.plan = PlanSlot::Proposed {
                    version: *version,
                    digest: digest.clone(),
                };
            }
            MissionEvent::PlanApproved {
                version,
                digest,
                by: actor,
            } => {
                if from != MissionState::Drafted {
                    return Err(invalid(from, event));
                }
                by(*actor, &[Person], event)?;
                let proposed = matches!(
                    &self.plan,
                    PlanSlot::Proposed { version: v, digest: d } if v == version && d == digest
                );
                if !proposed {
                    return Err(MissionRefusal::PlanMismatch);
                }
                self.plan = PlanSlot::Approved {
                    version: *version,
                    digest: digest.clone(),
                };
                self.state = MissionState::Planned;
            }
            MissionEvent::Started { by: actor } => {
                if from != MissionState::Planned {
                    return Err(invalid(from, event));
                }
                by(*actor, &[Person], event)?;
                self.state = MissionState::Running;
            }
            MissionEvent::Waiting { reason } => {
                if from != MissionState::Running {
                    return Err(invalid(from, event));
                }
                self.waiting_for = Some(*reason);
                self.state = MissionState::Waiting;
            }
            MissionEvent::Resumed { by: actor } => match from {
                MissionState::Waiting => {
                    by(*actor, &[Person, Core], event)?;
                    self.waiting_for = None;
                    self.state = MissionState::Running;
                }
                MissionState::Paused => {
                    by(*actor, &[Person], event)?;
                    self.state = MissionState::Running;
                }
                _ => return Err(invalid(from, event)),
            },
            MissionEvent::Paused { by: actor } => {
                if !matches!(from, MissionState::Running | MissionState::Waiting) {
                    return Err(invalid(from, event));
                }
                by(*actor, &[Person], event)?;
                self.waiting_for = None;
                self.state = MissionState::Paused;
            }
            MissionEvent::Stopped { reason, by: actor } => {
                match reason {
                    StopReason::Person => by(*actor, &[Person], event)?,
                    StopReason::LimitSpent => {
                        if from != MissionState::Running {
                            return Err(invalid(from, event));
                        }
                        by(*actor, &[Core], event)?;
                        if !self.limit_spent() {
                            return Err(MissionRefusal::LimitNotSpent);
                        }
                    }
                }
                self.waiting_for = None;
                self.stopped = Some(*reason);
                self.state = MissionState::Stopped;
            }
            MissionEvent::SpendObserved {
                tokens,
                wall_clock_secs,
            } => {
                self.spent_tokens = *tokens;
                self.spent_wall_clock_secs = *wall_clock_secs;
            }
            MissionEvent::CheckReported { check, tip, passed } => {
                if from != MissionState::Running {
                    return Err(invalid(from, event));
                }
                if *check >= self.done.len() {
                    return Err(MissionRefusal::UnknownCheck(*check));
                }
                self.checks.insert(*check, (tip.clone(), *passed));
            }
            MissionEvent::Done { tip } => {
                if from != MissionState::Running {
                    return Err(invalid(from, event));
                }
                if !self.proven_on(tip) {
                    return Err(MissionRefusal::DoneNotProven);
                }
                self.done_tip = Some(tip.clone());
                self.state = MissionState::Done;
            }
        }
        Ok(())
    }

    fn limit_spent(&self) -> bool {
        self.spent_tokens >= self.limit.tokens
            || self.spent_wall_clock_secs >= self.limit.wall_clock_secs
    }

    fn proven_on(&self, tip: &str) -> bool {
        !self.done.is_empty()
            && (0..self.done.len())
                .all(|i| matches!(self.checks.get(&i), Some((t, true)) if t == tip))
    }

    /// Where the mission stands.
    pub fn state(&self) -> MissionState {
        self.state
    }

    /// The goal.
    pub fn goal(&self) -> &str {
        &self.goal
    }

    /// The limit.
    pub fn limit(&self) -> &MissionLimit {
        &self.limit
    }

    /// The team.
    pub fn team(&self) -> &MissionTeam {
        &self.team
    }

    /// The definition of done.
    pub fn done_checks(&self) -> &[DoneCheck] {
        &self.done
    }

    /// The approved plan's version and digest, once approved.
    pub fn approved_plan(&self) -> Option<(u32, &str)> {
        match &self.plan {
            PlanSlot::Approved { version, digest } => Some((*version, digest.as_str())),
            PlanSlot::None | PlanSlot::Proposed { .. } => None,
        }
    }

    /// What it waits for, while `Waiting`.
    pub fn waiting_for(&self) -> Option<WaitReason> {
        self.waiting_for
    }

    /// Why it stopped, once `Stopped`.
    pub fn stopped_reason(&self) -> Option<StopReason> {
        self.stopped
    }

    /// The tip it was accepted on, once `Done`.
    pub fn done_tip(&self) -> Option<&str> {
        self.done_tip.as_deref()
    }

    /// The tokens and wall-clock seconds last observed.
    pub fn spent(&self) -> (u64, u64) {
        (self.spent_tokens, self.spent_wall_clock_secs)
    }
}

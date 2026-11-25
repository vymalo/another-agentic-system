//! A person's team presets, the pure rules (ADR 0074, decision 13; `docs/api/teams-v1.md`).
//!
//! A preset is a name, a title, the lead, two to eight members (a name, the agent with its optional
//! release, a role and how the member's history is kept), a charter, the cap of hand-offs, the mode
//! of their approval, the checkpoint rules, the budget of a push and whether the older entries are
//! summarised. Everything a client and a server must agree on is here and nowhere else: which saves
//! are refused and where ([`validate`]), and the bounds.
//!
//! Nothing of a preset is an event of the log: a thread that a preset started keeps a **copy** in
//! `team_formed`, never a reference (ADR 0074 decision 13, which amends invariant 3).
//!
//! # One vocabulary for one team
//!
//! The team ledger ([`super`]) came first, and this module **reuses it rather than defining a second
//! one**: [`MemberName`], [`MIN_MEMBERS`], [`MAX_MEMBERS`], [`MAX_NAME_CHARS`], [`MAX_ROLE_CHARS`],
//! [`MIN_MAX_HANDOFFS`], [`MAX_HANDOFFS_CEILING`] and [`TeamRefusal`] are the ledger's. A member's
//! name, a role that is too long, a repeated name, too few or too many members, a lead that is no
//! member and a cap out of range are refused **by the ledger's own code**, and the preset only adds
//! where the field stands (`at`, for the 400 of the API). What is the preset's alone is the preset's:
//! its name (a slug, as a prompt's, not a member's), the title, the charter's bytes, the approval's
//! seconds, the push budget, how many presets a person holds, a member's release and its history.
//!
//! # The lead is a member's name
//!
//! ADR 0074 decision 4 sends an unaddressed message to the preset's lead, and decision 1 lets two
//! members be the same agent (two coders). An agent id would therefore not say which member leads,
//! so [`TeamPreset::lead`] is a [`MemberName`], and [`validate`] refuses a lead that is no member.
//!
//! # Units
//!
//! A length is counted in Unicode scalar values (`chars().count()`), except the charter, which is
//! counted in UTF-8 bytes. **Nothing is trimmed here.**

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{
    MAX_MEMBERS, MAX_NAME_CHARS, MAX_ROLE_CHARS, MIN_MEMBERS, Member, MemberName, TeamLedger,
    TeamRefusal,
};
use crate::error::{Classify, ErrorClass};
use crate::ids::is_valid_agent_id;
use crate::thread::AgentTarget;

/// The most team presets one person holds.
pub const MAX_TEAM_PRESETS: usize = 50;
/// The most presets one listing holds.
pub const PAGE: usize = 50;
/// The longest title of a preset, in Unicode scalar values. (`MAX_TITLE_CHARS` is a thread title's:
/// the two bounds are equal today and are not one rule.)
pub const MAX_TEAM_TITLE_CHARS: usize = 120;
/// The longest charter, in UTF-8 bytes.
pub const MAX_CHARTER_BYTES: usize = 4096;
/// The longest release selector of a member's agent, in characters.
pub const MAX_RELEASE_CHARS: usize = 64;
/// The least seconds a hand-off may wait for the person before it goes (ADR 0074 decision 10).
pub const MIN_APPROVAL_SECONDS: u16 = 5;
/// The most seconds a hand-off may wait for the person.
pub const MAX_APPROVAL_SECONDS: u16 = 600;
/// The least budget of a push, in estimated tokens (ADR 0074 A1).
pub const MIN_PUSH_TOKENS: u32 = 4_000;
/// The most budget of a push, in estimated tokens.
pub const MAX_PUSH_TOKENS: u32 = 100_000;
/// The budget of a push when a preset says nothing.
pub const DEFAULT_PUSH_TOKENS: u32 = 24_000;

// ------------------------------------------------------------------------------------------ ids

/// Not `t-` and 12 lowercase letters or digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("not a team preset id: `t-` and 12 lowercase letters or digits")]
pub struct BadTeamPresetId;

impl Classify for BadTeamPresetId {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// The id of a team preset: `t-` and 12 lowercase base-36 characters (ADR 0074 decision 13, as a
/// prompt's id, ADR 0070). Global: the id alone names a preset, and it is no capability, since only
/// its owner reads it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TeamPresetId(String);

impl TeamPresetId {
    const PREFIX: &'static str = "t-";
    const SUFFIX_LEN: usize = 12;

    /// Reads an id.
    ///
    /// # Errors
    /// [`BadTeamPresetId`] for anything but `t-` and 12 characters of `[a-z0-9]`.
    pub fn parse(text: &str) -> Result<Self, BadTeamPresetId> {
        let suffix = text.strip_prefix(Self::PREFIX).ok_or(BadTeamPresetId)?;
        let ok = suffix.len() == Self::SUFFIX_LEN
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
        if ok {
            Ok(TeamPresetId(text.to_owned()))
        } else {
            Err(BadTeamPresetId)
        }
    }

    /// An id made of 16 bytes of entropy: the same bytes make the same id. The 128 bits are read as
    /// a number and cut to 36^12 values: the bias is 2^-76.
    pub fn from_entropy(entropy: [u8; 16]) -> Self {
        const DIGITS: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut n = u128::from_be_bytes(entropy) % 36u128.pow(12);
        let mut suffix = [b'0'; TeamPresetId::SUFFIX_LEN];
        for slot in suffix.iter_mut().rev() {
            *slot = DIGITS[usize::try_from(n % 36).unwrap_or(0)];
            n /= 36;
        }
        let suffix: String = suffix.iter().map(|b| char::from(*b)).collect();
        TeamPresetId(format!("{}{suffix}", Self::PREFIX))
    }

    /// The text of the id.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TeamPresetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for TeamPresetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TeamPresetId({})", self.0)
    }
}

impl FromStr for TeamPresetId {
    type Err = BadTeamPresetId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

// ----------------------------------------------------------------------------------- the errors

/// A save the rules refuse: which rule (the `code` of the 400), where (`at`, the name of the field as
/// the contract spells it), and — when the team ledger already refuses it — **that refusal**, whose
/// words are its own (`refusal`). It never holds a value of the preset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamPresetError {
    /// `bad_name`, `bad_title`, `bad_charter`, `bad_members`, `bad_member_name`, `bad_member_role`,
    /// `bad_member_agent`, `bad_member_release`, `bad_lead`, `bad_max_handoffs`,
    /// `bad_handoff_approval`, `bad_checkpoint` or `bad_push_tokens`.
    pub code: &'static str,
    /// `name`, `title`, `charter`, `members`, `lead`, `maxHandoffs`, `handoffApproval`,
    /// `checkpoints` or `pushTokens`; for a member `members.<i>` or `members.<i>.<field>`.
    pub at: String,
    /// The ledger's own refusal, when this one is the ledger's rule (a member's name or role, how
    /// many members, a repeated name, the lead, the cap of hand-offs).
    pub refusal: Option<TeamRefusal>,
}

impl fmt::Display for TeamPresetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // A rule the ledger owns is worded by the ledger, so that one team never reads two
        // sentences for one rule.
        if let Some(refusal) = &self.refusal {
            return refusal.fmt(f);
        }
        // Every other sentence names the field and, where there is one, the bound: a person who is
        // refused a team reads which number was wrong.
        let words = match self.code {
            "bad_name" => {
                "the name is not 1 to 32 of a-z, 0-9 and -, starting with a letter or a digit"
            }
            "bad_title" => "the title is not 1 to 120 characters without controls or edge spaces",
            "bad_charter" => "the charter is not at most 4096 bytes of text without controls",
            "bad_member_role" => "a role is not printable text of at most 280 characters",
            "bad_member_agent" => {
                "a member's agent id is not 1 to 63 of a-z, 0-9 and -, starting with a letter or a digit"
            }
            "bad_member_release" => "a member's release is not 1 to 64 printable characters",
            "bad_handoff_approval" => "handoffApproval is not ask, auto, or after 5 to 600 seconds",
            "bad_checkpoint" => "a checkpoint rule does not name a member",
            _ => "pushTokens is not 4000 to 100000",
        };
        f.write_str(words)
    }
}

impl std::error::Error for TeamPresetError {}

impl Classify for TeamPresetError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Invalid
    }
}

/// A refusal of the preset's own rules.
fn fail(code: &'static str, at: impl Into<String>) -> TeamPresetError {
    TeamPresetError {
        code,
        at: at.into(),
        refusal: None,
    }
}

/// The same refusal, worded by the team ledger.
fn refused(code: &'static str, at: impl Into<String>, refusal: TeamRefusal) -> TeamPresetError {
    TeamPresetError {
        code,
        at: at.into(),
        refusal: Some(refusal),
    }
}

/// Whether `s` has a control character (U+0000 to U+001F, U+007F to U+009F) other than `allowed`.
fn has_control(s: &str, allowed: &[char]) -> bool {
    s.chars().any(|c| {
        let n = u32::from(c);
        (n <= 0x1f || (0x7f..=0x9f).contains(&n)) && !allowed.contains(&c)
    })
}

/// Whether `c` is a bidirectional embedding, override or isolate control, or a line or paragraph
/// separator: U+202A to U+202E, U+2066 to U+2069, U+2028 and U+2029.
fn is_bidi(c: char) -> bool {
    matches!(u32::from(c), 0x202a..=0x202e | 0x2066..=0x2069 | 0x2028 | 0x2029)
}

fn scalars(s: &str) -> usize {
    s.chars().count()
}

/// A **preset's** name, which is not a member's: `^[a-z0-9][a-z0-9-]{0,31}$`, as a prompt's name is,
/// because it keys a menu. A member's name goes through [`MemberName::new`], the ledger's.
fn is_preset_name(text: &str) -> bool {
    let mut bytes = text.bytes();
    let first = bytes
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    first
        && text.len() <= MAX_NAME_CHARS
        && text
            .bytes()
            .skip(1)
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

// ---------------------------------------------------------------------------------- the members

/// How a member's history reaches it in a push (ADR 0074 A2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MemberHistory {
    /// The agent keeps the conversation, as A2A's context semantics do. The default.
    #[default]
    AgentKeeps,
    /// The orchestrator sends the thread's history on every turn, as a fork's history.
    OrchestratorSends,
}

/// A member as a save holds it, before the rules read it.
///
/// It is not [`Member`]: the ledger's member is what a thread holds once the team is formed (its
/// name, its agent and its role), while a save also carries the agent's optional **release** and how
/// that member's **history** is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberInput {
    /// The name, without the `@`.
    pub name: String,
    /// The agent, with its optional release.
    pub target: AgentTarget,
    /// What the member is for, a line the brief gives it.
    pub role: String,
    /// How its history reaches it.
    #[serde(default)]
    pub history: MemberHistory,
}

/// A member the rules accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamMember {
    name: MemberName,
    target: AgentTarget,
    role: String,
    history: MemberHistory,
}

impl TeamMember {
    /// The name.
    pub fn name(&self) -> &MemberName {
        &self.name
    }

    /// The agent and its optional release.
    pub fn target(&self) -> &AgentTarget {
        &self.target
    }

    /// The role.
    pub fn role(&self) -> &str {
        &self.role
    }

    /// How its history reaches it.
    pub fn history(&self) -> MemberHistory {
        self.history
    }

    /// The member as the team ledger holds it once the team is formed (the release is the preset's:
    /// a thread is formed on the agent its card names).
    pub fn to_member(&self) -> Member {
        Member {
            name: self.name.clone(),
            agent: self.target.agent_id.clone(),
            role: self.role.clone(),
        }
    }

    /// The save this member would be made from.
    pub fn to_input(&self) -> TeamMemberInput {
        TeamMemberInput {
            name: self.name.as_str().to_owned(),
            target: self.target.clone(),
            role: self.role.clone(),
            history: self.history,
        }
    }
}

/// The first rule a member breaks, or the name it was accepted under. `seen` holds the names of the
/// members before it.
fn check_member(
    i: usize,
    input: &TeamMemberInput,
    seen: &mut BTreeSet<MemberName>,
) -> Result<MemberName, TeamPresetError> {
    let here = format!("members.{i}");
    let name = match MemberName::new(&input.name) {
        Ok(name) => name,
        Err(refusal) => return Err(refused("bad_member_name", format!("{here}.name"), refusal)),
    };
    if !seen.insert(name.clone()) {
        return Err(refused(
            "bad_member_name",
            format!("{here}.name"),
            TeamRefusal::DuplicateName,
        ));
    }
    let agent = input.target.agent_id.as_str();
    if !is_valid_agent_id(agent) {
        return Err(fail("bad_member_agent", format!("{here}.agentId")));
    }
    if let Some(release) = input.target.release.as_deref() {
        let printable = (1..=MAX_RELEASE_CHARS).contains(&scalars(release))
            && release.bytes().all(|b| (0x21..=0x7e).contains(&b));
        if !printable {
            return Err(fail("bad_member_release", format!("{here}.release")));
        }
    }
    if scalars(&input.role) > MAX_ROLE_CHARS {
        return Err(refused(
            "bad_member_role",
            format!("{here}.role"),
            TeamRefusal::RoleTooLong,
        ));
    }
    if has_control(&input.role, &[]) || input.role.chars().any(is_bidi) {
        return Err(fail("bad_member_role", format!("{here}.role")));
    }
    Ok(name)
}

// ------------------------------------------------------------------------------- the approval

/// When a hand-off runs (ADR 0074 decision 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HandoffApproval {
    /// Every hand-off waits for the person. The default.
    #[default]
    Ask,
    /// It goes unless the person acted within this many seconds (5 to 600).
    After {
        /// The seconds it waits.
        seconds: u16,
    },
    /// It goes at once: a team that runs unattended.
    Auto,
}

impl HandoffApproval {
    /// The seconds it waits, if it waits on a timer.
    pub fn seconds(self) -> Option<u16> {
        match self {
            HandoffApproval::After { seconds } => Some(seconds),
            HandoffApproval::Ask | HandoffApproval::Auto => None,
        }
    }
}

// ------------------------------------------------------------------------------ the checkpoints

/// When the team stops and waits for the person (ADR 0074 decision 11).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRules {
    /// The members whose turn pauses the team when it ends.
    #[serde(default)]
    pub after: BTreeSet<String>,
    /// Whether a turn that is given branches to merge, or a pushed branch to open a pull request
    /// from, pauses before it starts.
    #[serde(default)]
    pub before_pull_request: bool,
}

// ------------------------------------------------------------------------------- the definition

/// What a save holds before the rules have read it (the body of `POST` and `PUT /api/teams`).
///
/// `maxHandoffs` and `pushTokens` are the caller's to fill: a client that wants the defaults sends
/// [`DEFAULT_MAX_HANDOFFS`](super::DEFAULT_MAX_HANDOFFS) and [`DEFAULT_PUSH_TOKENS`], and zero is
/// refused like any other number out of bounds, never read as "the default".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamPresetInput {
    /// The name, `^[a-z0-9][a-z0-9-]{0,31}$`.
    pub name: String,
    /// The title.
    pub title: String,
    /// The name of the member an unaddressed message goes to.
    pub lead: String,
    /// Two to eight members.
    pub members: Vec<TeamMemberInput>,
    /// The charter, the brief every member is given.
    pub charter: String,
    /// At most this many hand-offs per person message (6 to 20, [`MIN_MAX_HANDOFFS`](super::MIN_MAX_HANDOFFS) to [`MAX_HANDOFFS_CEILING`](super::MAX_HANDOFFS_CEILING)).
    pub max_handoffs: u32,
    /// When a hand-off runs.
    #[serde(default)]
    pub handoff_approval: HandoffApproval,
    /// When the team stops and waits.
    #[serde(default)]
    pub checkpoints: CheckpointRules,
    /// The budget of a push, in estimated tokens (4 000 to 100 000).
    pub push_tokens: u32,
    /// Whether the orchestrator's model summarises the older entries for the members (ADR 0074 A2).
    /// The summary itself is the thread's, never the preset's.
    #[serde(default)]
    pub summary: bool,
}

/// A team preset the rules accepted. It holds no owner: ownership is the store's, and no type of
/// the port carries one.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamPreset {
    id: TeamPresetId,
    name: String,
    title: String,
    lead: MemberName,
    members: Vec<TeamMember>,
    charter: String,
    max_handoffs: u32,
    handoff_approval: HandoffApproval,
    checkpoints: CheckpointRules,
    push_tokens: u32,
    summary: bool,
}

impl TeamPreset {
    /// A preset of `input` under `id`, if the rules accept it.
    ///
    /// # Errors
    /// The first rule the save breaks, as [`validate`] says.
    pub fn new(id: TeamPresetId, input: TeamPresetInput) -> Result<Self, TeamPresetError> {
        let checked = check(&input)?;
        let members = input
            .members
            .iter()
            .zip(checked.names)
            .map(|(m, name)| TeamMember {
                name,
                target: m.target.clone(),
                role: m.role.clone(),
                history: m.history,
            })
            .collect();
        Ok(TeamPreset {
            id,
            name: input.name,
            title: input.title,
            lead: checked.lead,
            members,
            charter: input.charter,
            max_handoffs: input.max_handoffs,
            handoff_approval: input.handoff_approval,
            checkpoints: input.checkpoints,
            push_tokens: input.push_tokens,
            summary: input.summary,
        })
    }

    /// The id.
    pub fn id(&self) -> &TeamPresetId {
        &self.id
    }

    /// The name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The member an unaddressed message goes to.
    pub fn lead(&self) -> &MemberName {
        &self.lead
    }

    /// The members, in the order the save gave them.
    pub fn members(&self) -> &[TeamMember] {
        &self.members
    }

    /// The charter.
    pub fn charter(&self) -> &str {
        &self.charter
    }

    /// At most this many hand-offs per person message.
    pub fn max_handoffs(&self) -> u32 {
        self.max_handoffs
    }

    /// The cap [`TeamLedger`] runs this preset at (ADR 0074 decision 8).
    pub fn cap(&self) -> u32 {
        // validated into `MIN_MAX_HANDOFFS..=MAX_HANDOFFS_CEILING`, so this cannot fail
        TeamLedger::cap_for(Some(self.max_handoffs)).unwrap_or(self.max_handoffs)
    }

    /// The members as the ledger holds them once the team is formed.
    pub fn ledger_members(&self) -> Vec<Member> {
        self.members.iter().map(TeamMember::to_member).collect()
    }

    /// When a hand-off runs.
    pub fn handoff_approval(&self) -> HandoffApproval {
        self.handoff_approval
    }

    /// When the team stops and waits.
    pub fn checkpoints(&self) -> &CheckpointRules {
        &self.checkpoints
    }

    /// The budget of a push, in estimated tokens.
    pub fn push_tokens(&self) -> u32 {
        self.push_tokens
    }

    /// Whether the older entries are summarised for the members.
    pub fn summary(&self) -> bool {
        self.summary
    }

    /// The member the name `name` is, if it is one.
    pub fn member(&self, name: &str) -> Option<&TeamMember> {
        self.members.iter().find(|m| m.name.as_str() == name)
    }

    /// The save this preset would be made from (for an edit that changes some fields).
    pub fn to_input(&self) -> TeamPresetInput {
        TeamPresetInput {
            name: self.name.clone(),
            title: self.title.clone(),
            lead: self.lead.as_str().to_owned(),
            members: self.members.iter().map(TeamMember::to_input).collect(),
            charter: self.charter.clone(),
            max_handoffs: self.max_handoffs,
            handoff_approval: self.handoff_approval,
            checkpoints: self.checkpoints.clone(),
            push_tokens: self.push_tokens,
            summary: self.summary,
        }
    }
}

// ---------------------------------------------------------------------------------- the save

/// What a save holds once the rules accepted it: the lead and each member's name, in the save's
/// order. [`validate`] keeps nothing and [`TeamPreset::new`] builds from it, so a name is parsed
/// once.
struct Checked {
    lead: MemberName,
    names: Vec<MemberName>,
}

/// The first rule a save breaks, in this order: the name, the title, the charter, the members (how
/// many, then each in order: its name and that no member repeats one, the agent, the release, the
/// role), the lead (it must be a member), `maxHandoffs`, `handoffApproval`, the checkpoint rules,
/// `pushTokens`.
///
/// # Errors
/// [`TeamPresetError`], with the rule's `code` and the field — and, for a rule the team ledger owns
/// (a member's name or role, how many members, a repeated name, the lead, the cap of hand-offs), the
/// ledger's own [`TeamRefusal`].
fn check(input: &TeamPresetInput) -> Result<Checked, TeamPresetError> {
    if !is_preset_name(&input.name) {
        return Err(fail("bad_name", "name"));
    }
    let title = &input.title;
    let edge = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r');
    if !(1..=MAX_TEAM_TITLE_CHARS).contains(&scalars(title))
        || has_control(title, &[])
        || title.chars().any(is_bidi)
        || title.starts_with(edge)
        || title.ends_with(edge)
    {
        return Err(fail("bad_title", "title"));
    }
    if input.charter.len() > MAX_CHARTER_BYTES
        || has_control(&input.charter, &['\t', '\n', '\r'])
        || input.charter.chars().any(is_bidi)
    {
        return Err(fail("bad_charter", "charter"));
    }
    if !(MIN_MEMBERS..=MAX_MEMBERS).contains(&input.members.len()) {
        let refusal = if input.members.len() < MIN_MEMBERS {
            TeamRefusal::TooFewMembers
        } else {
            TeamRefusal::TooManyMembers
        };
        return Err(refused("bad_members", "members", refusal));
    }
    let mut seen = BTreeSet::new();
    let mut names = Vec::with_capacity(input.members.len());
    for (i, member) in input.members.iter().enumerate() {
        names.push(check_member(i, member, &mut seen)?);
    }
    let lead =
        MemberName::new(&input.lead).map_err(|refusal| refused("bad_lead", "lead", refusal))?;
    if !seen.contains(&lead) {
        return Err(refused("bad_lead", "lead", TeamRefusal::LeadNotMember));
    }
    if let Err(refusal) = TeamLedger::cap_for(Some(input.max_handoffs)) {
        return Err(refused("bad_max_handoffs", "maxHandoffs", refusal));
    }
    if input
        .handoff_approval
        .seconds()
        .is_some_and(|s| !(MIN_APPROVAL_SECONDS..=MAX_APPROVAL_SECONDS).contains(&s))
    {
        return Err(fail("bad_handoff_approval", "handoffApproval"));
    }
    if let Some(rule) = input
        .checkpoints
        .after
        .iter()
        .find(|n| MemberName::new(n).map_or(true, |name| !seen.contains(&name)))
    {
        return Err(fail("bad_checkpoint", format!("checkpoints.after.{rule}")));
    }
    if !(MIN_PUSH_TOKENS..=MAX_PUSH_TOKENS).contains(&input.push_tokens) {
        return Err(fail("bad_push_tokens", "pushTokens"));
    }
    Ok(Checked { lead, names })
}

/// The first rule a save breaks, or `()` when the rules accept it.
///
/// # Errors
/// As [`TeamPreset::new`].
pub fn validate(input: &TeamPresetInput) -> Result<(), TeamPresetError> {
    check(input).map(|_| ())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;

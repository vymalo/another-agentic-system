//! A person's team presets: what the application does with them (ADR 0074 decision 13,
//! `docs/api/teams-v1.md`). The HTTP routes call these methods and nothing else; no handler is given
//! the port.
//!
//! # Who may
//!
//! - **Every operation is the person's own**: a preset that is not theirs is
//!   [`TeamRefusal::NotFound`], the answer for an id that never existed. The owner predicate is the
//!   store's.
//! - **Every member's agent must be one the person may invoke**: the roles' `agents` name it
//!   (`agent.invoke`, a 403 as a message to it would be) **and** its audience lets the person in
//!   (ADR 0045), in which case an agent the audience hides is, to them, not there:
//!   [`TeamRefusal::UnknownAgent`], with the member it is about. The live card is not read: a
//!   release is checked when a thread is started on the preset.
//!
//! # The order of the checks
//!
//! `create` and `replace`: the values ([`orch_core::TeamPreset::new`]), the members' agents, then
//! the store (the revision, the name, the limit). Nothing here logs or quotes a word of a preset.

use orch_core::{TeamPreset, TeamPresetId, TeamPresetInput};
use orch_ports::{
    Clock, IdGen, StoredTeamPreset, TeamPresetPage, TeamPresetRevision, TeamPresetStore,
    TeamPresetStoreError,
};

use crate::app::App;
use crate::authz::{Permission, Requester};
use crate::error::AppError;

/// How many times a save is tried with a new id when the id turns out to be taken.
const ID_ATTEMPTS: usize = 4;

/// Why the presets refused a request. Each is the caller's to act on; none holds a word of a preset.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum TeamRefusal {
    /// The save breaks a rule of the page: the code says which, `at` where.
    #[error("{0}")]
    Invalid(orch_core::TeamPresetError),
    /// A member's agent is not one the person may use: it is not listed, or its audience leaves
    /// them out (the same answer for both).
    #[error("a member's agent is not available to you")]
    UnknownAgent {
        /// Where: `members.<i>.agent`.
        at: String,
    },
    /// The person already has a preset of that name.
    #[error("you already have a team preset of that name")]
    NameTaken,
    /// The person holds the most presets there are.
    #[error("you hold the most team presets there are ({limit})")]
    TooMany {
        /// The limit.
        limit: usize,
    },
    /// The preset changed since the revision the request was based on.
    #[error("the team preset changed since the revision sent")]
    StaleRevision,
    /// No such preset for this person: it never existed, is gone, or is another person's.
    #[error("no such team preset")]
    NotFound,
}

impl TeamRefusal {
    /// The `code` of the problem, where a client acts on one; `None` for a plain 404.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            TeamRefusal::Invalid(e) => Some(e.code),
            TeamRefusal::UnknownAgent { .. } => Some("unknown_agent"),
            TeamRefusal::NameTaken => Some("name_taken"),
            TeamRefusal::TooMany { .. } => Some("too_many_teams"),
            TeamRefusal::StaleRevision => Some("stale_revision"),
            TeamRefusal::NotFound => None,
        }
    }
}

impl From<TeamRefusal> for AppError {
    fn from(refusal: TeamRefusal) -> Self {
        AppError::Team(refusal)
    }
}

fn store_error(error: TeamPresetStoreError) -> AppError {
    match error {
        TeamPresetStoreError::TooMany { limit } => TeamRefusal::TooMany { limit }.into(),
        TeamPresetStoreError::NameTaken => TeamRefusal::NameTaken.into(),
        TeamPresetStoreError::NotFound => TeamRefusal::NotFound.into(),
        TeamPresetStoreError::Conflict => TeamRefusal::StaleRevision.into(),
        TeamPresetStoreError::Unavailable { .. } => {
            AppError::Store(orch_ports::StoreError::unavailable(error))
        }
        TeamPresetStoreError::Corrupt(_) => AppError::Store(orch_ports::StoreError::corrupt_with(
            "a person's team preset does not read back",
            error,
        )),
        TeamPresetStoreError::Internal { .. } => {
            AppError::Store(orch_ports::StoreError::internal(error))
        }
        TeamPresetStoreError::Exists => {
            AppError::internal("a team preset id the store reported as taken was not retried")
        }
        // a variant added later is a bug until it is mapped
        other => AppError::Store(orch_ports::StoreError::internal(other)),
    }
}

impl<P: orch_ports::Ports> App<P> {
    /// Every member's agent is one the person may invoke.
    async fn check_team_members(
        &self,
        who: &impl Requester,
        preset: &TeamPreset,
    ) -> Result<(), AppError> {
        let access = self.access(who);
        for (i, member) in preset.members().iter().enumerate() {
            let id = self.agents.canonical(&member.target().agent_id);
            // before the registry is asked: what the roles let a person invoke does not depend on it
            access
                .check_scope(Permission::AgentInvoke, &id)
                .map_err(|denied| crate::app::denied_agent(denied, &id))?;
            if self
                .visible_agent(&access, Permission::AgentInvoke, &id)
                .await?
                .is_none()
            {
                return Err(TeamRefusal::UnknownAgent {
                    at: format!("members.{i}.agent"),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Saves a preset of the person's own.
    ///
    /// # Errors
    /// [`TeamRefusal::Invalid`] for a rule of the page; [`AppError::Forbidden`] for a member's agent
    /// the roles do not name; [`TeamRefusal::UnknownAgent`]; [`TeamRefusal::NameTaken`];
    /// [`TeamRefusal::TooMany`]; [`AppError::RegistryUnavailable`]; [`AppError::Store`].
    pub async fn create_team_preset(
        &self,
        who: &impl Requester,
        input: TeamPresetInput,
    ) -> Result<StoredTeamPreset, AppError> {
        let store = self.ports().store();
        for _ in 0..ID_ATTEMPTS {
            let id = TeamPresetId::from_entropy(self.ports().ids().new_token_bytes());
            let preset = TeamPreset::new(id, input.clone()).map_err(TeamRefusal::Invalid)?;
            self.check_team_members(who, &preset).await?;
            let now = self.ports().clock().now();
            match store.insert_team_preset(who.user(), &preset, now).await {
                Ok(stored) => return Ok(stored),
                Err(TeamPresetStoreError::Exists) => {}
                Err(e) => return Err(store_error(e)),
            }
        }
        Err(AppError::internal(
            "a new team preset id was taken four times in a row",
        ))
    }

    /// A page of the person's presets, by id.
    ///
    /// # Errors
    /// [`AppError::Store`].
    pub async fn list_team_presets(
        &self,
        who: &impl Requester,
        after: Option<&TeamPresetId>,
    ) -> Result<TeamPresetPage, AppError> {
        let limit = u32::try_from(orch_core::TEAM_PRESET_PAGE).unwrap_or(50);
        self.ports()
            .store()
            .list_team_presets(who.user(), after, limit)
            .await
            .map_err(store_error)
    }

    /// One of the person's presets.
    ///
    /// # Errors
    /// [`TeamRefusal::NotFound`] for any other; [`AppError::Store`].
    pub async fn get_team_preset(
        &self,
        who: &impl Requester,
        id: &TeamPresetId,
    ) -> Result<StoredTeamPreset, AppError> {
        self.ports()
            .store()
            .get_team_preset(who.user(), id)
            .await
            .map_err(store_error)?
            .ok_or_else(|| TeamRefusal::NotFound.into())
    }

    /// Replaces the person's preset `id` with `input` if it is still at `expected`.
    ///
    /// # Errors
    /// [`TeamRefusal::NotFound`]; [`TeamRefusal::StaleRevision`]; and what
    /// [`create_team_preset`](Self::create_team_preset) refuses.
    pub async fn replace_team_preset(
        &self,
        who: &impl Requester,
        id: &TeamPresetId,
        input: TeamPresetInput,
        expected: TeamPresetRevision,
    ) -> Result<StoredTeamPreset, AppError> {
        // the preset first: a preset that is not theirs is a 404 whatever the body says
        self.get_team_preset(who, id).await?;
        let preset = TeamPreset::new(id.clone(), input).map_err(TeamRefusal::Invalid)?;
        self.check_team_members(who, &preset).await?;
        let now = self.ports().clock().now();
        self.ports()
            .store()
            .replace_team_preset(who.user(), &preset, expected, now)
            .await
            .map_err(store_error)
    }

    /// Deletes the person's preset; with `expected`, only if it is still at that revision. A thread
    /// the preset started keeps its copy.
    ///
    /// # Errors
    /// [`TeamRefusal::NotFound`] (not theirs, or gone); [`TeamRefusal::StaleRevision`];
    /// [`AppError::Store`].
    pub async fn delete_team_preset(
        &self,
        who: &impl Requester,
        id: &TeamPresetId,
        expected: Option<TeamPresetRevision>,
    ) -> Result<(), AppError> {
        self.ports()
            .store()
            .delete_team_preset(who.user(), id, expected)
            .await
            .map_err(store_error)
    }
}

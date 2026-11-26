//! A person's team presets: what the application does with them (ADR 0074 decision 13,
//! `docs/api/teams-v1.md`). The HTTP routes call these methods and nothing else.
//!
//! # Who may
//!
//! - **Presets are each person's own.** Every method asks the store with the caller's id, so a
//!   preset that is somebody else's is [`TeamPresetRefusal::NotFound`], the answer for an id that
//!   never existed.
//! - **A member's agent must be one the person may use** (`agent.invoke` over the agent, and its
//!   audience, ADR 0045). An agent the person may not use, or that no registry lists for them, is
//!   [`TeamPresetRefusal::AgentRefused`] naming the member (`members.<i>.agentId`) and never the
//!   agent: what a person may not read does not look different from what is not there.
//!
//! # The order of the checks
//!
//! `create`: the values ([`orch_core::TeamPreset::new`]), the members' agents, then the store (the
//! name, the limit). `replace`: the preset (404), the revision (409), the values, the members'
//! agents, then the store (the name). Nothing here logs or quotes a word of a preset.

use orch_core::{AgentId, TeamPreset, TeamPresetError, TeamPresetId, TeamPresetInput};
use orch_ports::{
    Clock, IdGen, Ports, StoredTeamPreset, TeamPresetPage, TeamPresetRevision, TeamPresetStore,
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
pub enum TeamPresetRefusal {
    /// The save breaks a rule of the page: the code says which, `at` where.
    #[error("{0}")]
    Invalid(TeamPresetError),
    /// A member's agent is not one the person may use, or none is listed for them.
    #[error("a member's agent is unknown to you or you may not use it")]
    AgentRefused {
        /// `members.<i>.agentId`.
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
    /// No such preset for this person (never existed, gone, or somebody else's: the same answer).
    #[error("no such team preset")]
    NotFound,
}

impl TeamPresetRefusal {
    /// The `code` of the problem, where a client acts on one; `None` for a plain 404.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            TeamPresetRefusal::Invalid(e) => Some(e.code),
            TeamPresetRefusal::AgentRefused { .. } => Some("member_agent_refused"),
            TeamPresetRefusal::NameTaken => Some("name_taken"),
            TeamPresetRefusal::TooMany { .. } => Some("too_many_teams"),
            TeamPresetRefusal::StaleRevision => Some("stale_revision"),
            TeamPresetRefusal::NotFound => None,
        }
    }
}

impl From<TeamPresetRefusal> for AppError {
    fn from(refusal: TeamPresetRefusal) -> Self {
        AppError::TeamPreset(refusal)
    }
}

fn store_error(error: TeamPresetStoreError) -> AppError {
    match error {
        TeamPresetStoreError::TooMany { limit } => TeamPresetRefusal::TooMany { limit }.into(),
        TeamPresetStoreError::NameTaken => TeamPresetRefusal::NameTaken.into(),
        TeamPresetStoreError::NotFound => TeamPresetRefusal::NotFound.into(),
        TeamPresetStoreError::Conflict => TeamPresetRefusal::StaleRevision.into(),
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

impl<P: Ports> App<P> {
    /// Whether every member's agent is one `who` may use; the first that is not is refused by
    /// position.
    async fn check_member_agents(
        &self,
        who: &impl Requester,
        preset: &TeamPreset,
    ) -> Result<(), AppError> {
        let access = self.access(who);
        for (i, member) in preset.members().iter().enumerate() {
            let id: AgentId = self.canonical_agent(&member.target().agent_id);
            let refused = || {
                AppError::from(TeamPresetRefusal::AgentRefused {
                    at: format!("members.{i}.agentId"),
                })
            };
            // what the roles let a person invoke is asked first and does not depend on the registry
            if access.check_scope(Permission::AgentInvoke, &id).is_err() {
                return Err(refused());
            }
            // an agent whose audience leaves the person out is unknown to them (ADR 0045)
            if self
                .visible_agent(&access, Permission::AgentInvoke, &id)
                .await?
                .is_none()
            {
                return Err(refused());
            }
        }
        Ok(())
    }

    /// Saves a preset of the person's own.
    ///
    /// # Errors
    /// [`TeamPresetRefusal::Invalid`] for a rule of the page; [`TeamPresetRefusal::AgentRefused`];
    /// [`TeamPresetRefusal::NameTaken`]; [`TeamPresetRefusal::TooMany`]; [`AppError::Store`];
    /// [`AppError::RegistryUnavailable`].
    pub async fn create_team_preset(
        &self,
        who: &impl Requester,
        input: TeamPresetInput,
    ) -> Result<StoredTeamPreset, AppError> {
        let store = self.ports().store();
        for _ in 0..ID_ATTEMPTS {
            let id = TeamPresetId::from_entropy(self.ports().ids().new_token_bytes());
            let preset = TeamPreset::new(id, input.clone()).map_err(TeamPresetRefusal::Invalid)?;
            self.check_member_agents(who, &preset).await?;
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

    /// A page of the person's presets.
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

    /// One preset of the person's own.
    ///
    /// # Errors
    /// [`TeamPresetRefusal::NotFound`] for any other; [`AppError::Store`].
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
            .ok_or_else(|| TeamPresetRefusal::NotFound.into())
    }

    /// Replaces the whole preset at `expected`.
    ///
    /// # Errors
    /// [`TeamPresetRefusal::NotFound`]; [`TeamPresetRefusal::StaleRevision`];
    /// [`TeamPresetRefusal::Invalid`]; [`TeamPresetRefusal::AgentRefused`];
    /// [`TeamPresetRefusal::NameTaken`]; [`AppError::Store`].
    pub async fn replace_team_preset(
        &self,
        who: &impl Requester,
        id: &TeamPresetId,
        expected: TeamPresetRevision,
        input: TeamPresetInput,
    ) -> Result<StoredTeamPreset, AppError> {
        let current = self.get_team_preset(who, id).await?;
        if current.revision() != expected {
            return Err(TeamPresetRefusal::StaleRevision.into());
        }
        let next = TeamPreset::new(id.clone(), input).map_err(TeamPresetRefusal::Invalid)?;
        self.check_member_agents(who, &next).await?;
        if &next == current.preset() {
            return Ok(current);
        }
        let now = self.ports().clock().now();
        self.ports()
            .store()
            .replace_team_preset(who.user(), &next, expected, now)
            .await
            .map_err(store_error)
    }

    /// Deletes the person's own preset; with `expected`, only if it is still at that revision.
    ///
    /// # Errors
    /// [`TeamPresetRefusal::NotFound`]; [`TeamPresetRefusal::StaleRevision`]; [`AppError::Store`].
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

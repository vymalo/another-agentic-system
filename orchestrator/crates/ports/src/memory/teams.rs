//! The in-memory team presets: the reference implementation of the conformance suite
//! (`team_preset_store_conformance!`), held by [`MemoryStore`] so that it is a
//! [`TeamPresetStore`] like every [`ThreadStore`](crate::ThreadStore).

use std::collections::BTreeMap;

use orch_core::{TeamPreset, TeamPresetId};
use orch_core::{Timestamp, UserId};

use super::MemoryStore;
use crate::{
    MAX_TEAM_PRESETS, StoredTeamPreset, TEAM_PRESET_PAGE, TeamPresetPage, TeamPresetRevision,
    TeamPresetStore, TeamPresetStoreError, TeamPresetSummary,
};

#[derive(Debug, Clone)]
struct Row {
    owner: UserId,
    preset: TeamPreset,
    revision: TeamPresetRevision,
    updated_at: Timestamp,
}

/// Every person's presets, ordered by id so that a listing is.
#[derive(Debug, Default)]
pub(super) struct TeamShelf {
    rows: BTreeMap<TeamPresetId, Row>,
    /// The last revision handed out: one counter for the whole shelf, so a revision is never reused,
    /// not even by an id that was deleted and saved again.
    last: u64,
}

impl TeamShelf {
    fn next_revision(&mut self) -> TeamPresetRevision {
        self.last += 1;
        TeamPresetRevision::new(self.last)
    }

    fn held(&self, owner: &UserId) -> usize {
        self.rows.values().filter(|r| &r.owner == owner).count()
    }

    fn name_taken(&self, owner: &UserId, except: Option<&TeamPresetId>, name: &str) -> bool {
        self.rows
            .iter()
            .any(|(id, r)| &r.owner == owner && Some(id) != except && r.preset.name() == name)
    }

    /// The owner's preset `id`, if it is theirs.
    fn own(&self, owner: &UserId, id: &TeamPresetId) -> Option<&Row> {
        self.rows.get(id).filter(|row| &row.owner == owner)
    }
}

fn stored(row: &Row) -> StoredTeamPreset {
    StoredTeamPreset::new(row.preset.clone(), row.revision, row.updated_at)
}

impl MemoryStore {
    fn team_shelf(&self) -> std::sync::MutexGuard<'_, TeamShelf> {
        self.team_presets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The page `limit` asks for: at least one, at most [`TEAM_PRESET_PAGE`].
fn page_size(limit: u32) -> usize {
    // a page is at least one and at most `PAGE`, whatever the caller asked for (the port's promise)
    usize::try_from(limit.max(1))
        .unwrap_or(usize::MAX)
        .min(TEAM_PRESET_PAGE)
}

impl TeamPresetStore for MemoryStore {
    async fn insert_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        at: Timestamp,
    ) -> Result<StoredTeamPreset, TeamPresetStoreError> {
        let mut shelf = self.team_shelf();
        if shelf.rows.contains_key(preset.id()) {
            return Err(TeamPresetStoreError::Exists);
        }
        if shelf.name_taken(owner, None, preset.name()) {
            return Err(TeamPresetStoreError::NameTaken);
        }
        if shelf.held(owner) >= MAX_TEAM_PRESETS {
            return Err(TeamPresetStoreError::TooMany {
                limit: MAX_TEAM_PRESETS,
            });
        }
        let revision = shelf.next_revision();
        let row = Row {
            owner: owner.clone(),
            preset: preset.clone(),
            revision,
            updated_at: at,
        };
        let out = stored(&row);
        shelf.rows.insert(preset.id().clone(), row);
        Ok(out)
    }

    async fn get_team_preset(
        &self,
        owner: &UserId,
        id: &TeamPresetId,
    ) -> Result<Option<StoredTeamPreset>, TeamPresetStoreError> {
        let shelf = self.team_shelf();
        Ok(shelf.own(owner, id).map(stored))
    }

    async fn list_team_presets(
        &self,
        owner: &UserId,
        after: Option<&TeamPresetId>,
        limit: u32,
    ) -> Result<TeamPresetPage, TeamPresetStoreError> {
        let shelf = self.team_shelf();
        let held = u32::try_from(shelf.held(owner)).unwrap_or(u32::MAX);
        let mut visible = shelf
            .rows
            .iter()
            .filter(|(_, row)| &row.owner == owner)
            .filter(|(id, _)| after.is_none_or(|a| *id > a));
        let take = page_size(limit);
        let items: Vec<TeamPresetSummary> = visible
            .by_ref()
            .take(take)
            .map(|(_, row)| TeamPresetSummary {
                id: row.preset.id().clone(),
                name: row.preset.name().to_owned(),
                title: row.preset.title().to_owned(),
                lead: row.preset.lead().clone(),
                member_count: u8::try_from(row.preset.members().len()).unwrap_or(u8::MAX),
                revision: row.revision,
                updated_at: row.updated_at,
            })
            .collect();
        let more = visible.next().is_some();
        Ok(TeamPresetPage { items, more, held })
    }

    async fn replace_team_preset(
        &self,
        owner: &UserId,
        preset: &TeamPreset,
        expected: TeamPresetRevision,
        at: Timestamp,
    ) -> Result<StoredTeamPreset, TeamPresetStoreError> {
        let mut shelf = self.team_shelf();
        match shelf.own(owner, preset.id()) {
            Some(row) if row.revision != expected => return Err(TeamPresetStoreError::Conflict),
            Some(_) => {}
            None => return Err(TeamPresetStoreError::NotFound),
        }
        if shelf.name_taken(owner, Some(preset.id()), preset.name()) {
            return Err(TeamPresetStoreError::NameTaken);
        }
        let revision = shelf.next_revision();
        let row = Row {
            owner: owner.clone(),
            preset: preset.clone(),
            revision,
            updated_at: at,
        };
        let out = stored(&row);
        shelf.rows.insert(preset.id().clone(), row);
        Ok(out)
    }

    async fn delete_team_preset(
        &self,
        owner: &UserId,
        id: &TeamPresetId,
        expected: Option<TeamPresetRevision>,
    ) -> Result<(), TeamPresetStoreError> {
        let mut shelf = self.team_shelf();
        match shelf.own(owner, id) {
            Some(row) if expected.is_some_and(|e| e != row.revision) => {
                return Err(TeamPresetStoreError::Conflict);
            }
            Some(_) => {}
            None => return Err(TeamPresetStoreError::NotFound),
        }
        shelf.rows.remove(id);
        Ok(())
    }
}

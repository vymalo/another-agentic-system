//! The in-memory prompt library: the reference implementation of the conformance suite
//! (`prompt_store_conformance!`), held by [`MemoryStore`] so that it is a [`PromptStore`] like every
//! [`ThreadStore`](crate::ThreadStore).

use std::collections::BTreeMap;

use orch_core::prompt::{PromptDefinition, PromptId};
use orch_core::{Timestamp, UserId};

use super::MemoryStore;
use crate::{
    MAX_PROMPTS, PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError,
    PromptSummary, PromptViewer, StoredPrompt,
};

#[derive(Debug, Clone)]
struct Row {
    owner: UserId,
    prompt: PromptDefinition,
    revision: PromptRevision,
    updated_at: Timestamp,
}

/// Every person's prompts, ordered by id so that a listing is.
#[derive(Debug, Default)]
pub(super) struct PromptShelf {
    rows: BTreeMap<PromptId, Row>,
    /// The last revision handed out: one counter for the whole shelf, so a revision is never reused,
    /// not even by an id that was deleted and saved again.
    last: u64,
}

impl PromptShelf {
    fn next_revision(&mut self) -> PromptRevision {
        self.last += 1;
        PromptRevision::new(self.last)
    }

    fn held(&self, owner: &UserId) -> usize {
        self.rows.values().filter(|r| &r.owner == owner).count()
    }

    fn name_taken(&self, owner: &UserId, except: Option<&PromptId>, name: &str) -> bool {
        self.rows.iter().any(|(id, r)| {
            &r.owner == owner && Some(id) != except && r.prompt.name().as_str() == name
        })
    }
}

fn seen_by(row: &Row, viewer: &PromptViewer) -> Option<bool> {
    if row.owner == viewer.user {
        Some(true)
    } else if row.prompt.audience().admits(&viewer.roles) {
        Some(false)
    } else {
        None
    }
}

fn stored(row: &Row, own: bool) -> StoredPrompt {
    StoredPrompt::new(row.prompt.clone(), row.revision, row.updated_at, own)
}

impl MemoryStore {
    fn shelf(&self) -> std::sync::MutexGuard<'_, PromptShelf> {
        self.prompts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl PromptStore for MemoryStore {
    async fn insert_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        at: Timestamp,
    ) -> Result<StoredPrompt, PromptStoreError> {
        let mut shelf = self.shelf();
        if shelf.rows.contains_key(prompt.id()) {
            return Err(PromptStoreError::Exists);
        }
        if shelf.name_taken(owner, None, prompt.name().as_str()) {
            return Err(PromptStoreError::NameTaken);
        }
        if shelf.held(owner) >= MAX_PROMPTS {
            return Err(PromptStoreError::TooMany { limit: MAX_PROMPTS });
        }
        let revision = shelf.next_revision();
        let row = Row {
            owner: owner.clone(),
            prompt: prompt.clone(),
            revision,
            updated_at: at,
        };
        let out = stored(&row, true);
        shelf.rows.insert(prompt.id().clone(), row);
        Ok(out)
    }

    async fn get_prompt(
        &self,
        viewer: &PromptViewer,
        id: &PromptId,
    ) -> Result<Option<StoredPrompt>, PromptStoreError> {
        let shelf = self.shelf();
        Ok(shelf
            .rows
            .get(id)
            .and_then(|row| seen_by(row, viewer).map(|own| stored(row, own))))
    }

    async fn list_prompts(
        &self,
        viewer: &PromptViewer,
        scope: PromptScope,
        after: Option<&PromptId>,
        limit: u32,
    ) -> Result<PromptPage, PromptStoreError> {
        let shelf = self.shelf();
        let held = u32::try_from(shelf.held(&viewer.user)).unwrap_or(u32::MAX);
        let mut visible = shelf
            .rows
            .iter()
            .filter(|(id, _)| after.is_none_or(|a| *id > a))
            .filter_map(|(_, row)| seen_by(row, viewer).map(|own| (row, own)))
            .filter(|(_, own)| match scope {
                PromptScope::Mine => *own,
                PromptScope::Shared => !own,
                PromptScope::All => true,
            });
        // a page is at most `PAGE`, whatever the caller asked for (the port's promise)
        let take = usize::try_from(limit)
            .unwrap_or(usize::MAX)
            .min(orch_core::prompt::PAGE);
        let items: Vec<PromptSummary> = visible
            .by_ref()
            .take(take)
            .map(|(row, own)| PromptSummary {
                id: row.prompt.id().clone(),
                name: row.prompt.name().as_str().to_owned(),
                title: row.prompt.title().to_owned(),
                description: row.prompt.description().to_owned(),
                variable_count: u8::try_from(row.prompt.variables().len()).unwrap_or(u8::MAX),
                audience: own.then(|| row.prompt.audience().clone()),
                own,
                revision: row.revision,
                updated_at: row.updated_at,
            })
            .collect();
        let more = visible.next().is_some();
        Ok(PromptPage { items, more, held })
    }

    async fn replace_prompt(
        &self,
        owner: &UserId,
        prompt: &PromptDefinition,
        expected: PromptRevision,
        at: Timestamp,
    ) -> Result<StoredPrompt, PromptStoreError> {
        let mut shelf = self.shelf();
        match shelf.rows.get(prompt.id()) {
            Some(row) if &row.owner == owner => {
                if row.revision != expected {
                    return Err(PromptStoreError::Conflict);
                }
            }
            _ => return Err(PromptStoreError::NotFound),
        }
        if shelf.name_taken(owner, Some(prompt.id()), prompt.name().as_str()) {
            return Err(PromptStoreError::NameTaken);
        }
        let revision = shelf.next_revision();
        let row = Row {
            owner: owner.clone(),
            prompt: prompt.clone(),
            revision,
            updated_at: at,
        };
        let out = stored(&row, true);
        shelf.rows.insert(prompt.id().clone(), row);
        Ok(out)
    }

    async fn delete_prompt(
        &self,
        owner: &UserId,
        id: &PromptId,
        expected: Option<PromptRevision>,
    ) -> Result<(), PromptStoreError> {
        let mut shelf = self.shelf();
        match shelf.rows.get(id) {
            Some(row) if &row.owner == owner => {
                if expected.is_some_and(|e| e != row.revision) {
                    return Err(PromptStoreError::Conflict);
                }
            }
            _ => return Err(PromptStoreError::NotFound),
        }
        shelf.rows.remove(id);
        Ok(())
    }
}

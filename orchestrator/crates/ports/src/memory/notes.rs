//! The in-memory memory notes: the reference implementation of the conformance suite
//! (`memory_note_store_conformance!`), held by [`MemoryStore`] so that it is a [`MemoryNoteStore`]
//! beside being a [`ThreadStore`](crate::ThreadStore).
//!
//! What a read returns is **`orch_core::Memory::read` itself**, fed the person's notes in the order
//! they were saved: the filter, the order and the cut are the core's rules, not a second copy that
//! could drift from them.

use std::collections::BTreeMap;

use orch_core::{
    ForgetInput, Grant, MAX_NOTES, Memory, MemorySettings, Note, ReadInput, ReadResult, SaveInput,
    Timestamp, UserId,
};

use super::MemoryStore;
use crate::{MemoryNoteStore, MemoryNoteStoreError};

/// One saved note and whose it is.
#[derive(Debug, Clone)]
struct Row {
    owner: String,
    note: Note,
}

/// Every person's notes in the order saved, and every person's grants.
#[derive(Debug, Default)]
pub(super) struct NoteShelf {
    rows: Vec<Row>,
    /// `(owner, agent)` to what the owner lets that agent do.
    grants: BTreeMap<(String, String), Grant>,
}

impl NoteShelf {
    fn own<'a>(&'a self, owner: &'a UserId) -> impl Iterator<Item = &'a Note> {
        self.rows
            .iter()
            .filter(move |row| row.owner == owner.as_str())
            .map(|row| &row.note)
    }
}

impl MemoryStore {
    fn note_shelf(&self) -> std::sync::MutexGuard<'_, NoteShelf> {
        self.memory_notes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl MemoryNoteStore for MemoryStore {
    async fn save_note(
        &self,
        owner: &UserId,
        text: &SaveInput,
        written_by: &str,
        id: &str,
        at: Timestamp,
    ) -> Result<Note, MemoryNoteStoreError> {
        let mut shelf = self.note_shelf();
        if shelf.own(owner).any(|note| note.id == id) {
            return Err(MemoryNoteStoreError::Exists);
        }
        if shelf.own(owner).count() >= MAX_NOTES {
            return Err(MemoryNoteStoreError::TooMany { limit: MAX_NOTES });
        }
        let note = Note {
            id: id.to_owned(),
            text: text.0.clone(),
            saved_at: at.to_string(),
            written_by: written_by.to_owned(),
        };
        shelf.rows.push(Row {
            owner: owner.as_str().to_owned(),
            note: note.clone(),
        });
        Ok(note)
    }

    async fn read_notes(
        &self,
        owner: &UserId,
        input: &ReadInput,
    ) -> Result<ReadResult, MemoryNoteStoreError> {
        let shelf = self.note_shelf();
        let mut memory = Memory::default();
        for note in shelf.own(owner) {
            memory
                .save(
                    SaveInput(note.text.clone()),
                    &note.written_by,
                    note.id.clone(),
                    note.saved_at.clone(),
                )
                .map_err(|_| {
                    MemoryNoteStoreError::Corrupt("a person holds too many notes".into())
                })?;
        }
        Ok(memory.read(input))
    }

    async fn list_notes(&self, owner: &UserId) -> Result<Vec<Note>, MemoryNoteStoreError> {
        let shelf = self.note_shelf();
        let mut notes: Vec<Note> = shelf.own(owner).cloned().collect();
        notes.reverse();
        Ok(notes)
    }

    async fn forget_note(
        &self,
        owner: &UserId,
        id: &ForgetInput,
    ) -> Result<bool, MemoryNoteStoreError> {
        let mut shelf = self.note_shelf();
        let before = shelf.rows.len();
        shelf
            .rows
            .retain(|row| !(row.owner == owner.as_str() && row.note.id == id.0));
        Ok(shelf.rows.len() != before)
    }

    async fn memory_settings(
        &self,
        owner: &UserId,
    ) -> Result<MemorySettings, MemoryNoteStoreError> {
        let shelf = self.note_shelf();
        let agents = shelf
            .grants
            .iter()
            .filter(|((who, _), _)| who == owner.as_str())
            .map(|((_, agent), grant)| (agent.clone(), *grant))
            .collect();
        Ok(MemorySettings {
            default: Grant::default(),
            agents,
        })
    }

    async fn set_memory_grant(
        &self,
        owner: &UserId,
        agent: &str,
        grant: Grant,
    ) -> Result<(), MemoryNoteStoreError> {
        self.note_shelf()
            .grants
            .insert((owner.as_str().to_owned(), agent.to_owned()), grant);
        Ok(())
    }
}

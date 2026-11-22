//! The prompt library, declared and not built.
//!
//! [`ThreadStore`](orch_ports::ThreadStore) requires [`PromptStore`](orch_ports::PromptStore): an
//! adapter that did not implement it would not compile. The other two supertraits are built —
//! [`UsageLedger`](orch_ports::UsageLedger) in `crate::usage` and
//! [`ThreadSearch`](orch_ports::ThreadSearch) in `crate::search` (ADR 0076 E2, milestone 2a) — and
//! this one is **milestone 2b**: a person's saved prompts and the roles they share each with
//! (ADR 0070), with its own tables, its own migration, its own conformance suite and the hooks that
//! suite needs.
//!
//! Until then every method here answers an error that names the milestone. It is deliberately
//! **not** an empty answer: an empty prompt page is something a caller cannot tell from "nothing
//! was saved", and a store that answered it would be wrong silently, in production, for as long as
//! nobody looked. An error that says which milestone it is waiting for cannot be mistaken for an
//! answer.

use orch_core::UserId;
use orch_core::prompt::PromptDefinition;
use orch_ports::{
    PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError, PromptViewer,
};

use crate::SqliteStore;

/// The detail of a [`PromptStoreError::Internal`] this milestone answers.
const PROMPTS: &str =
    "the SQLite store does not hold the prompt library yet (ADR 0076 E2, milestone 2b)";

impl PromptStore for SqliteStore {
    async fn insert_prompt(
        &self,
        _owner: &UserId,
        _prompt: &PromptDefinition,
        _at: orch_core::Timestamp,
    ) -> Result<orch_ports::StoredPrompt, PromptStoreError> {
        Err(PromptStoreError::internal(PROMPTS))
    }

    async fn get_prompt(
        &self,
        _viewer: &PromptViewer,
        _id: &orch_core::prompt::PromptId,
    ) -> Result<Option<orch_ports::StoredPrompt>, PromptStoreError> {
        Err(PromptStoreError::internal(PROMPTS))
    }

    async fn list_prompts(
        &self,
        _viewer: &PromptViewer,
        _scope: PromptScope,
        _after: Option<&orch_core::prompt::PromptId>,
        _limit: u32,
    ) -> Result<PromptPage, PromptStoreError> {
        Err(PromptStoreError::internal(PROMPTS))
    }

    async fn replace_prompt(
        &self,
        _owner: &UserId,
        _prompt: &PromptDefinition,
        _expected: PromptRevision,
        _at: orch_core::Timestamp,
    ) -> Result<orch_ports::StoredPrompt, PromptStoreError> {
        Err(PromptStoreError::internal(PROMPTS))
    }

    async fn delete_prompt(
        &self,
        _owner: &UserId,
        _id: &orch_core::prompt::PromptId,
        _expected: Option<PromptRevision>,
    ) -> Result<(), PromptStoreError> {
        Err(PromptStoreError::internal(PROMPTS))
    }
}

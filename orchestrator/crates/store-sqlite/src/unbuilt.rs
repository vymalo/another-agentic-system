//! The port's three supertraits, declared and not built.
//!
//! [`ThreadStore`](orch_ports::ThreadStore) requires [`UsageLedger`](orch_ports::UsageLedger),
//! [`ThreadSearch`](orch_ports::ThreadSearch) and [`PromptStore`](orch_ports::PromptStore): an
//! adapter that did not implement them would not compile. This milestone builds the thread store
//! only (ADR 0076 E2, milestone 1); the other three are their own commits, with their own tables,
//! their own conformance suites and the `forget` and `age` hooks those suites need.
//!
//! Until then every method here answers an error that names the milestone. It is deliberately
//! **not** an empty answer: an empty ledger month, an empty search and an empty prompt page are
//! things a caller cannot tell from "nothing was used", "nothing matched" and "nothing was saved",
//! and a store that answered them would be wrong silently, in production, for as long as nobody
//! looked. An error that says which milestone it is waiting for cannot be mistaken for an answer.

use orch_core::prompt::PromptDefinition;
use orch_core::{ThreadId, UsageMonth, UsageMonthTotals, UsageRow, UserId};
use orch_ports::{
    PromptPage, PromptRevision, PromptScope, PromptStore, PromptStoreError, PromptViewer,
    SearchBackfill, SearchRequest, SearchRows, StoreError, ThreadSearch, UsageBackfill,
    UsageLedger,
};

use crate::SqliteStore;

/// The detail of a [`StoreError::Internal`] this milestone answers.
const LEDGER: &str =
    "the SQLite store does not hold the usage ledger yet (ADR 0076 E2, milestone 2)";

/// The detail of a [`StoreError::Internal`] this milestone answers.
const SEARCH: &str =
    "the SQLite store does not hold the search index yet (ADR 0076 E2, milestone 2)";

/// The detail of a [`PromptStoreError::Internal`] this milestone answers.
const PROMPTS: &str =
    "the SQLite store does not hold the prompt library yet (ADR 0076 E2, milestone 2)";

impl UsageLedger for SqliteStore {
    async fn month(
        &self,
        _owner: &UserId,
        _month: UsageMonth,
    ) -> Result<UsageMonthTotals, StoreError> {
        Err(StoreError::internal(LEDGER))
    }

    async fn thread_rows(
        &self,
        _owner: &UserId,
        _thread: ThreadId,
    ) -> Result<Vec<UsageRow>, StoreError> {
        Err(StoreError::internal(LEDGER))
    }

    async fn backfill(
        &self,
        _after: Option<ThreadId>,
        _limit: u32,
    ) -> Result<UsageBackfill, StoreError> {
        Err(StoreError::internal(LEDGER))
    }

    async fn sweep(&self, _before: UsageMonth, _limit: u32) -> Result<u64, StoreError> {
        Err(StoreError::internal(LEDGER))
    }
}

impl ThreadSearch for SqliteStore {
    async fn search_threads(
        &self,
        _owner: &UserId,
        _request: &SearchRequest,
    ) -> Result<SearchRows, StoreError> {
        Err(StoreError::internal(SEARCH))
    }

    async fn backfill_search(
        &self,
        _after: Option<ThreadId>,
        _limit: u32,
        _rebuild: bool,
    ) -> Result<SearchBackfill, StoreError> {
        Err(StoreError::internal(SEARCH))
    }

    async fn search_backlog(&self, _cap: u32) -> Result<u32, StoreError> {
        Err(StoreError::internal(SEARCH))
    }
}

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

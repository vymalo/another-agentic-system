# ADR 0068 — A person searches their own threads: whole words by their start, the rules in the contract, a derived index in the commit of the event

- **Status:** **proposed (2026-11-15)**, the design of slice 7 of [ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md)
  (wave 1), built in this change as **7a: the API, the store, the contract and the mock server; no web UI** (7b, the box, is later).
  **Proposed**, for the owner to confirm: every name, default and shape below, and the [decisions the build settled](#what-the-build-settled-for-the-owner-to-confirm).
  Builds on [ADR 0039](0039-nobody-reads-another-persons-thread.md) (a person reads their own threads),
  [ADR 0043](0043-deleting-a-thread-erases-it.md) (what an erase leaves) and [ADR 0065](0065-a-usage-ledger-per-person.md) (durable state written in the commit
  of the event, with a watermark and a backfill). **It amends invariant 3** of `CLAUDE.md` and [ADR 0001](0001-rust-state-machine-on-postgres.md) with a
  dated note (decision 2), as ADR 0042, 0046 and 0065 did before it.

## Context

A person with a few hundred threads cannot find one. The list is the only way in, and it holds titles and descriptions. ADR 0067 puts search in
wave 1 and fixes the rule that governs it: **every feature is implementable from `docs/api/` alone** (the server enforces, a client only offers), so a
second client (the Flutter app) must be able to search from the contract, and two clients must show the same hits.

*Verified 2026-11-15 by reading the code at `d0a732f`:*

| What | Where |
|---|---|
| `listThreads` answers a bare array of `Thread`, with `before` as a cursor that has rail meaning: there is no room for hits or a cursor object | `docs/api/chat-api.yaml`, `list_threads` in `orchestrator/crates/api/src/routes.rs` |
| the request log writes the query string as it is (except the OAuth callback), so a `GET ?q=` would put a person's words in every access log | `orchestrator/crates/api/src/trace.rs` |
| a thread's words are in `events`: `user_message.text` and `agent_message.text`; a partial message is logged (`final: false`) and a later final one with the same `messageId` replaces it; `purpose: working` marks words said while the agent works | `orch_core::AgentMessageData`, `docs/api/thread-markdown-v1.md` ("The answer") |
| a fork copies its parent's events up to the cut as they are, and the parent may be deleted afterwards | `PgStore::create`, `ThreadStore::fork_thread` (ADR 0029) |
| there is no text-search extension in use (`unaccent`, `pg_trgm`) and none is needed below | `git grep` in `orchestrator/crates/store-postgres/migrations` |

## Decision

1. **A new route, `POST /api/search/threads` (`searchThreads`).** Not a parameter of `listThreads`, which answers an array and has no room for hits; not a
   `GET`, because the query must not be in a URL or an access log (the route is a safe read that writes nothing; `POST` only keeps the words out of URLs).
   The request has `q`, `limit` (1 to 50, default 20), `cursor`, `archived` (`exclude`, `only`, `include`; default **`include`**) and `branches`. The answer is
   `{query: {terms}, results: [{thread, rank, hits, messageHits}], next?}`; a `hit` has a `field` (`title`, `description`, `message`), for a message its `seq`,
   `role` and `at`, and a `snippet` with `text`, `cutStart`, `cutEnd` and `matches` as **UTF-16 offsets** (as `Mention` has them), never markup.
   `GET /api/config` gains `search: {maxQueryLength, maxTerms}`: its presence is the capability (ADR 0067, rule 3).
2. **The index is durable state beside the log and amends invariant 3.** *"The search index of a person's threads (the folded words of their titles,
   descriptions and messages) is durable state beside the log, a function of it, written in the commit of the event, erased with the thread and rebuilt
   from the log by `orchestrator search backfill`."* Unlike the usage ledger, it holds **content** (folded words), which is why the amendment is stated and
   not implied. It is derived (the log is the truth; the table can be dropped and rebuilt), it is the owner's alone (every row names its owner, every read
   filters on it) and it goes in the transaction that deletes the thread, so an erase leaves no word (decision 8).
3. **What is searched:** the title, the description, a person's `user_message`s and an agent's **final** `agent_message`s that are not working text,
   the last of each `messageId`. **Not** searched: partial messages, working text, reasoning (not the agent's words, ADR 0044), steps' input and output
   and tool results (shown on demand and redacted, ADR 0030; large and noisy), asked agents' text, surfaces and plans, a form's answers (they need the
   surface to be read), file names (a follow-up) and usage, checks and shares (not words). Forks are searched: each is the person's own conversation, but
   **while the parent exists the events a fork copied give no message hit** (the parent shows them); once it is deleted they are the fork's own.
4. **Whole words, matched by their start, defined by the contract and computed in Rust.** A word is a run of letters, marks and numbers; its key is NFKD,
   no marks, lower case, `œ æ ß` spelled out, the final sigma made a sigma, only letters and numbers. A query's terms are the keys of its words (at most 8, of
   2 to 64 code points, distinct); a document matches when **every term is a prefix of a key of one of its words**. No stemming, no language, no
   segmentation: it serves English and French, and a run of Chinese letters is one word. **Postgres is only the index:** the store keeps the Rust keys as
   a `tsvector` built by `array_to_tsvector` (lexemes as given: no parser, no dictionary, no locale) and asks it a `tsquery` made of those keys with `:*`.
   So the candidates a database finds are the contract's whatever its collation, and a second client or a second store (the in-memory one) answers the same.
5. **Everything is deterministic and contractual:** the hit set, the snippets, their offsets, the **order** (`rank` descending, then the thread's `id`
   descending: 2 for a title hit, 1 for a description hit, 0 for messages only) and `messageHits`. Nothing depends on `ts_rank`, `ts_headline`, an
   extension or the database's version. Only the cursor's encoding is each server's own. The page is `docs/api/thread-search-v1.md`, with vectors that the Rust core and the
   web's mock server both read.
6. **Written in the commit of the event.** A new table `message_terms (thread_id, seq, owner, message_id, terms tsvector)` with a GIN index, and
   `threads.title_terms`, `threads.description_terms` and `threads.search_through` (the seq the index has been through, as ADR 0065's `usage_through`):
   `insert_events` writes the rows in the transaction of the events, a fork copies its parent's rows next to the events, and each commit first indexes
   `(search_through, last_seq]`, so a thread is complete whichever builds wrote and in whatever order. **Migration `0025`** adds columns and an empty table: nothing is
   rewritten and nothing locks a large table. `orchestrator search backfill` indexes the logs that predate it, per thread under its lock, in pages,
   idempotently (the same command shape as `usage backfill`).

   *Amended 2026-11-15 (the review of pull request 322).* **Three rules were missing.** (a) **A fold version.** The keys are made by three Unicode tables
   (Rust's `to_lowercase`, `regex-syntax`'s classes, `unicode-normalization`), so a dependency bump could split the stored keys from the query's with no error.
   `orch_core::search::FOLD_VERSION` (1) is stored per thread (`threads.search_fold`, in `0025`: it was not released) and **must be raised whenever the folding
   rules or the tables behind them change**; a unit test pins the fold on inputs chosen to move with those tables, and its failure message says "raise the
   version". A thread stored under another version is **unindexed: it is not found** (the choice: never found, rather than found by old keys), its next commit
   or the backfill indexes it again from the start replacing its rows, and `search backfill --rebuild` does that for every thread. (b) **The operator is told.**
   At startup the orchestrator logs one WARN with the count of unindexed threads (capped at 100,000) and the command; the count reads `threads` once per process
   start (it stops at the cap, but reads the whole table when nearly every thread is indexed: there is no index on columns that change with every commit),
   and `search backfill` reports how many `remaining`. (c) **A commit's catch-up is bounded**: at most 500 older events beside its own, the watermark moving only
   as far as it got (never past a gap), the commit's own events always indexed (above the watermark when the old log is not done: the upsert makes the later
   catch-up over them harmless), a message that a later final message replaces not indexed. A thread not caught up is found only for what is indexed.
7. **Authorization:** `thread.read`, which every role holds for its own threads, and **no new permission**; both `message_terms.owner` and
   `threads.owner` are the caller; no administrator reads more (ADR 0039); another person's thread is simply never a hit, by title or by content. **What it finds is always
   a subset of what `listThreads` lists** for the same `archived` and `branches` (a test holds it). No search under `/api/shared` or `/api/public`.
8. **Deletion (ADR 0043):** `message_terms` hangs on `events` with `ON DELETE CASCADE` and the title columns are on the thread row, so the delete's own
   transaction removes every word. Backups keep them, as ADR 0043 already says of everything else.
9. **Cost and abuse.** A query has at most 8 prefix terms of at least 2 characters; the store runs it in a read transaction under **one 2-second budget for all
   its statements** (each statement's `SET LOCAL statement_timeout` is what the ones before it left; a timeout is `503 search_unavailable` with `Retry-After`), and a
   process runs **at most 8 searches at once, for everybody**: the ninth is a 503 at once (no queue: a queue under a slow database holds connections longer);
   each person has a token bucket (a burst of 20, then 2 a second,
   in this process, `429 rate_limited` with `Retry-After`); the query is never in a URL, a trace or a log line (the log says the number of terms), and a test reads
   the captured log. **What is not bounded:** the cost of a prefix term over a very large shared index. The GIN index on `terms` spans every owner, so a
   prefix such as `ab` scans the postings of every person's messages that start with it before the owner filter applies, and the limits above bound the
   number and the time of searches, not that scan. If it is too slow at scale the follow-ups are a per-owner index (a partitioned or an owner-prefixed key)
   and a rule on the shortest prefix; neither is built here.

## Consequences

- **Easier:** a second client searches from the contract; the same query shows the same hits on every client; no extension, no collation to configure, no
  ranking function to tune; the thread list keeps its shape.
- **Harder:** the index is a second place that holds words, written on every message (a few hundred bytes of keys a message, and a GIN insert); a person's
  words are in two tables until the thread is erased; a new Unicode version could change which characters are letters, so the contract's vectors use only
  Unicode 13 and earlier and a change of the fold raises `FOLD_VERSION` and needs a backfill (decision 6, amended).
- **Required elsewhere:** the `ThreadSearch` port is a supertrait of `ThreadStore`: **every implementer of `ThreadStore` must implement it** (a breaking change
  for those who implement the store: the in-memory and Postgres stores do); `GET /api/config` has the member `search`; the web's mock server has the route;
  7b draws the box.
- **A limit, stated:** no stemming, no typo tolerance, no phrase query, no segmentation of Chinese, Japanese or Thai, only the first 3,000 words of a document.

## Alternatives rejected

- **Postgres full-text search with a language** (`to_tsvector('english', …)`, `websearch_to_tsquery`, `ts_rank`, `ts_headline`): it ties the hits to the
  database's configuration and version, gives a stemmer for one language to a person who writes two, and makes the snippets, the offsets and the order
  the database's: a second client could not reproduce them, which ADR 0067's first rule forbids.
- **`pg_trgm` or `unaccent`:** an extension to install and keep in step on every database, for a prefix match this design does without.
- **`GET /api/threads?q=`:** the words in every access log and proxy, and a cursor that has rail meaning.
- **Searching the log with `ILIKE`:** no index; a person's every event for every keystroke.
- **Ordering by last activity:** a cursor that must snapshot the order; by creation time the order is stable between pages.

## What the build settled, for the owner to confirm

1. **The amendment of invariant 3** is text above, noted in `CLAUDE.md` and ADR 0001 with this date.
2. **Order inside a rank:** the newest created, because paging stays stable (last activity would need a snapshot).
3. **Prefix matching with no stemming** for English and French.
4. **Searched:** titles, descriptions, a person's messages and the agent's final answers; file names later; never step input or output or reasoning.
5. **Archived threads are included by default** (the thread says it is archived); edit branches only when asked for, as in the list.
6. **A message that a fork copied is not a hit while its parent exists.**
7. **A limit of 3,000 words a document**, because a `tsvector` is limited to a megabyte and an unbounded one would fail a commit.
8. **The query is cut, not refused, where it is long:** a term over 64 code points is cut to them (a refusal would make a pasted URL an error).

## Facts, and how far each is proven

*Verified 2026-11-15 on a local PostgreSQL 16.15 (the version of `compose.yaml`) by `psql` and by the conformance suite of `orch-ports` run against `PgStore`
(`ORCH_TEST_DATABASE_URL`, 186 cases, 14 of them this ADR's, plus `tests/thread_search.rs` and the binary's `tests/search.rs`):*

- `array_to_tsvector(text[])` keeps lexemes exactly as given (no stop word dropped, no stemming, no case folding), sorts and deduplicates them; Cyrillic, CJK,
  accented and digit lexemes round-trip.
- `'key':* & 'key2':*` cast from text to `tsquery` is a prefix match of every key, in one `tsvector` (never across two): `'прив':*` finds `привет`, `'ab':*` finds
  `abc` and `'bc':*` does not.
- A fork's copied range indexed from the fork's own log, a message the parent later replaced included, and the cascade of `events` leaving no row of an erased thread.
- A search that waits on a lock held on `message_terms` ends in `57014` after the 2 s `statement_timeout`, which the store maps to `Unavailable` (the API's 503).
- Removing the fork predicate from the query makes the fork cases fail (the suite bites), as does removing the delete of a replaced message.

*Verified by running both implementations on the same vectors:* `orch_core::search` (the regex crate's Unicode tables, `unicode-normalization` 0.1.25) and `web/mock/search.ts`
(Node 22's ICU) agree on every vector, which use only characters of Unicode 13 and earlier; whether they agree beyond that is *unverified*.

*Verified 2026-11-15, after the review, on the same PostgreSQL 16 by the conformance suite and `tests/thread_search.rs`:* a thread under a lower fold version (with keys no query
makes) is not found, is indexed again by its commit (rows replaced) or by the backfill, and `--rebuild` goes through every thread; a commit to a thread with 1,200 or 2,000 old
events moves the watermark by exactly 500 and indexes its own events above it, the next commits do the next 500, the backfill finishes (rows written beyond the watermark
are upserted, never doubled) and a message replaced beyond the part caught up is never indexed; a commit that fails after its index write (a trigger of the test) leaves no row;
a later commit rewrites no old row (`xmin`); the backfill's message count is exact. Each of these was made to fail by the mutation that undoes it.

*Unverified:* the cost of a prefix term over a very large shared index (above: **not bounded**), and that the planner combines the GIN index on `terms` with the btree on `owner` (BitmapAnd) at the size of a real deployment (the query is written so that it can; no
plan was read at scale: the plan at scale stays *unverified*); the cost of the index at a million messages; `dev/search-e2e.sh` (it has not run against the stack: no Docker where it was written); and the web (7b).

## Not done here

The web: the box, the results and the highlighting (7b); the screens; the shortcut. Every Postgres claim is proven only where the conformance suite ran
(see the pull request).

# ADR 0094 — Retention is one sweep per class, one key each, and a default of 90 days

- **Status:** **Proposed (2026-11-24).** Mechanism for open questions 28, 29 and 46 (issue
  [#157](https://github.com/vymalo/another-agentic-system/issues/157)). Nothing is built. The ages are recorded (see
  *Decision of the ages*, 2026-11-25) and the questions are Closed; the mechanism is built slice by slice.
- **Owner's decision, ANR-Q51, 2026-11-24 06:07Z:** *"(b) An open-source default of 90 days for all of them"*. Recorded
  in the owner's decision log of 2026-11-24, row 2. The ages are the owner's; this ADR is the
  mechanism, and 90 days is the default it ships.

## Context

Three classes of data grow without bound today, in the database that also holds the chat log and in the artifact store:

| Class | Where | Today |
|---|---|---|
| **Local agents' journal** (question 28) | `orch_agent_runs`, `orch_agent_journal` | Kept for ever. adam-rs has `Store::purge_finished(agent, before)` (batches of 500 runs with their journals); nothing in the orchestrator calls it. |
| **Finished inbox rows and watches** (question 29) | `inbox` rows `applied`, `expired`, `dead`; `watches` | Kept for ever. The inbox rows are what makes a redelivery a duplicate (`UNIQUE (source, idempotency_key)`), so deleting one early lets a late redelivery look new. |
| **Files** (question 46, the half [ADR 0043](0043-deleting-a-thread-erases-it.md) left open) | the artifact store; uploads ([ADR 0058](0058-large-files-go-up-through-a-presigned-url-the-orchestrator-gives-the-agent.md)) | Kept while their thread exists. Objects an agent put with its slot and never named, and objects of a deleted thread's key that a still-valid URL recreated, stay until a sweep exists. |

## Decision

1. **One sweep per class, one key per class, each in its own unit.** A key is an age in days; `0` is not a value.
   A class is kept for ever only by the explicit value `forever`. Unset means **90 days** (the owner's default). A
   value that is neither a positive integer nor `forever` is refused at startup, exit 78.

   | Class | Key (`ORCH_CONFIG_FILE`, `retention.*`) | Default | What a sweep removes |
   |---|---|---|---|
   | Local journal | `retention.agentJournalDays` | 90 | finished runs older than the age, with their journals, through `purge_finished`, per agent kind |
   | Inbox | `retention.inboxDays` | 90 | rows `applied`, `expired`, `dead` whose `updated_at` is older than the age |
   | Watches | follows the inbox key | 90 | a watch whose thread has no live job and whose last activity is older than the age |
   | Files, orphans | `retention.orphanFilesDays` | 90 | objects no artifact names (an unnamed upload, a recreated key of a deleted thread) older than the age |
   | Files, age | `retention.filesDays` | 90 | the files of a thread whose last activity is older than the age, after its log reference is marked erased |

   The age of files is counted from the **thread's** last activity, never from the file's upload, so a conversation that
   is still used keeps its files. A file removed by age leaves its artifact reference in the log, marked
   `erased`, and the API answers `410` for it (the log is not rewritten).

2. **Who runs a sweep.** The orchestrator, in the `worker` and `all` roles, on a timer, like the purge sweep of
   ADR 0043 (`orchestrator/crates/app/src/purge.rs`). The control-plane-only role runs none. Files may instead be
   left to the **bucket's lifecycle rule** when the deployment says so (`retention.files: bucket`): the orchestrator
   then runs no age sweep for them and refuses to start if it cannot read a rule on the bucket. The orphan sweep is
   always the orchestrator's, because only it knows which keys an artifact names.
3. **Batching.** Each pass takes at most 500 rows or runs (the size `purge_finished` already uses) and sleeps between
   passes (`retention.sweepIntervalSecs`, default 600), so a first run on a large table works in slices and never holds
   a long transaction or a lock a request waits on.
4. **Replicas.** The processes are stateless, so a sweep must be safe to run on every replica at once. Rows are
   claimed `FOR UPDATE SKIP LOCKED` (the Postgres inbox and purge claims already do); `purge_finished` and
   `delete_prefix` are idempotent, so a claim that lapses and is taken over does no harm. No advisory lock and no
   leader.
5. **The inbox floor.** A window shorter than the longest time a sender may redeliver would turn a late redelivery into
   a new message. **`retention.inboxDays` below the floor is refused at startup, exit 78**, naming the sender whose window
   it undercuts. The floor is the longest redelivery window of the sources the deployment enables (`ORCH_SURFACES`):

   | Sender | Redelivery window | Status |
   |---|---|---|
   | `webhook-github` | A delivery that failed is not retried by GitHub on its own; a person or a script may redeliver recent deliveries by hand (a window of days). | **Unverified** (2026-11-24): not read at GitHub's docs in this session. Check at the docs and date it before the floor is set from it. |
   | `webhook-generic` (CI systems) | Set by each CI system's own retry policy. | **Unverified**: the floor for a generic source is a key of its own, `webhook.generic.redeliveryDays`, which the operator sets and which defaults to 7. |
   | Timers | The orchestrator's own: no redelivery after a row is `applied`. | Verified by `docs/orchestrator.md` (a timer's key includes the attempt). |

   Until the GitHub window is verified, the built-in floor is **7 days**, a deliberately safe bound; 90 days clears it.
   `forever` always clears the floor.
6. **An index on the finished rows.** A migration adds a partial index
   `inbox_finished ON inbox (updated_at) WHERE status IN ('applied','expired','dead')` (Postgres and SQLite), so the sweep is
   an index range scan and the claim indexes are untouched. The migration is append-only.
7. **Watches go with their thread's finished job.** The inbox sweep deletes the watches of a thread when no job of it
   is live and the thread has been idle for the age. A watch of a running job is never removed; a thread that is deleted
   already loses its watches (ADR 0043).
8. **Observable and honest.** Each sweep counts removed rows in a counter labelled by class only (no thread, no person),
   and logs a count, never a row. A sweep that fails is a warning and is tried again; a purge of files is a promise, as in
   ADR 0043.

## Slice table

Each row is one change with its decisive test. No row changes a default except to the 90 days above.

| # | Slice | Decisive test |
|---|---|---|
| R1 | The `retention` keys, their parsing and the startup refusals (`forever`, a bad value, the inbox floor) | a config below the floor exits 78 naming the sender; `forever` and 90 start; an unset key reads 90 |
| R2 | The inbox sweep and its index migration | rows older than the age and finished are gone, a `pending` or `parked` row and a younger finished row stay, a redelivery inside the window is still a duplicate; two sweeps at once remove each row once |
| R3 | The watches sweep | a watch of a thread with a live job stays; an idle thread's watches go |
| R4 | The local journal sweep through `purge_finished` | finished runs older than the age and their journals go per agent kind; a running run and a younger finished run stay |
| R5 | The file age sweep and the `410` for an erased file | a thread idle past the age loses its files and its artifacts answer `410`; an active thread keeps them |
| R6 | The orphan sweep | an upload never named and a key recreated for a deleted thread are removed after the age; a named object stays |
| R7 | The lifecycle-rule mode (`retention.files: bucket`) | startup is refused when the bucket has no readable rule; with one, no age sweep runs |
| R8 | The metrics and the docs (`docs/orchestrator.md`, `docs/reference` key table, crate READMEs) | the counters exist with the class label alone; docs-check passes |

## Consequences

- A deployment that does nothing loses finished inbox rows, journals, idle threads' files and orphans after 90 days. This
  is a change of default from "kept for ever", chosen by the owner for the open-source distribution; an operator who wants
  the old behaviour writes `forever`.
- The inbox floor makes a short window a startup error rather than a silent duplicate.
- Questions 28, 29 and 46 link to this ADR; they were Closed on 2026-11-25 once the ages were recorded (below).

## Decision of the ages (2026-11-25)

Status note, recorded by ANR-289. The owner decided the ages on 2026-11-24 (ANR-Q51, 06:07Z), in their words: *"(b) An open-source default of 90 days for all of them"*. Each class, as the owner gave it:

| Class | Retention |
|---|---|
| The local agents' journal (`orch_agent_runs`, `orch_agent_journal`) | 90 days, an open-source default |
| Finished inbox rows and their watches | 90 days, an open-source default; never below the redelivery floor |
| Files in the artifact store, and orphaned objects | 90 days, an open-source default |
| Adam's coder's finished runs | 90 days, an open-source default |

The value is the project's default, not only a setting of a reference deployment; a deployment that wants another value, or `forever`, sets the class's key. The owner's answer names no quota, so none is set by default. This note changes no mechanism: the sweeps, keys and the floor stay as decided above and are still not built.

# ADR 0094 — An inspector reads a run, and "replay from a step" is a fork of the journal into a new run

- **Status:** **proposed (2026-11-24)** (task ANR-261, issue [#404](https://github.com/vymalo/another-agentic-system/issues/404),
  ROADMAP M8, first gap of ANR-10). **Nothing is built.** Every name and shape below is the design's, for the owner to
  confirm by a dated note. Builds on [ADR 0039](0039-nobody-reads-another-persons-thread.md) (nobody reads another
  person's thread), [ADR 0053](0053-a-thread-export-says-which-builds-made-it.md) (the export) and
  [ADR 0025](0025-nested-steps-events-carry-their-source-path.md) (the step tree). The adam side is adam's own and gets its
  rows in the slice table, to be recorded as an adam-rs ADR by the same owner decision.

## Context

A developer of an agent has to answer "what did this run do and why". Today there are three partial answers, all
*verified 2026-11-24* at the heads of both repositories:

| What exists | sys, at `main` |
|---|---|
| The step tree: the panel's Activity tab, one line per agent turn | `web/README.md` "The step tree" (line 721); contract `docs/api/steps-v1.md` |
| The JSON export of a thread: thread, job, binding, events, builds | `orchestrator/crates/api/src/export.rs` (`Document`, line 80; `document`, line 200), route `GET /api/threads/{id}/export` (`orchestrator/crates/api/src/routes.rs:845`) |
| What a reader of a thread may see, and what is scrubbed | `orchestrator/crates/app/src/reader.rs` (`reader_event`, line 145; `ReaderRules`, `step_io`, line 91) |
| A fork of a thread (a new thread over a copy of the messages) | `routes.rs:1002` (`fork_thread`) |

| What exists | adam-rs, at `main` |
|---|---|
| The run record, its header, the journal entry | `crates/adam-core/src/store/mod.rs` (`RunRecord` 371, `RunHeader` 403, `JournalEntry` 475) |
| Reads of a run | `Store::load_run` (654), `run_header` (687), `journal_get` (755), `journal_list` (768), `list_runs` (854) |
| Writes of a run | `create_run` (641), `commit_run` (738), `journal_put` (765) |

What is missing: a view of a run's **state** and **journal** (the export holds the orchestrator's events, not adam's
journal), of its **inbox and connections**, of the **store** (the rows a run owns), and any **replay**. A thread's step
tree shows what an agent *reported*, not what the run *did*.

The risk is the inverse of the gap: an inspector is a second way to read a thread's content. ADR 0039 removed every
standing access to another person's thread, and an inspector must not bring one back.

## Decision

1. **Who may inspect.** A person inspects **the runs of threads they own**, nothing else; another person's run is `404`, as
   a thread is (ADR 0039 decision 1). There is no `inspect` role and no `admin` view of content: `admin` stays an
   operational, content-free permission (ADR 0039 decision 4). A **developer of a deployment** is not an administrator: a
   developer inspects runs of their own threads, in a deployment they may start threads in, and reads a user's run only
   when the user shares the thread (a revocable link, ADR 0040) or sends them an export. Break-glass stays outside the
   application (ADR 0039 decision 5); there is no API for it.
2. **What each view shows.**
   - *State*: the run's status, agent, parent, wake time, lease holder and generation (a `RunHeader`), and the head of its
     state with **no run context entries** (the per-run context an agent keeps is never shown).
   - *Journal*: the entries in `seq` order with name, ok or error and a payload that has passed the scrub below.
   - *Inbox and connections*: the orchestrator's inbox rows for the thread (ADR 0016) and the agent's open connections
     (the A2A subscription, the MCP servers it holds), as names and states, never a URL with credentials.
   - *Store view*: the counts and kinds of the rows a run owns (events, journal entries, blobs by digest and size), not
     their bodies.
3. **What each view scrubs.** A payload leaves the orchestrator only after adam's rule "a secret is replaced where it is a
   value" ([adam ADR 0070](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0070-a-secret-is-replaced-where-it-is-a-value.md))
   and the reader rules of `reader.rs` (workspace paths, step input and output as the owner's alone). A blob is shown as
   its digest and size, a file only to its owner. The scrub is applied **where the data is read** (adam for the journal,
   the orchestrator for events), never in the web.
4. **Where it lives.** One API, three surfaces. The orchestrator serves `GET /api/threads/{id}/runs` and
   `GET /api/threads/{id}/runs/{run}` (state, journal page, inbox, connections, store view), documented in `chat-api.yaml`;
   the **web** draws it as a tab beside Activity; a **CLI** reads the same routes; the MCP server of ANR-10's second gap
   exposes read tools over them, under the same ownership rule. Nothing reads the database directly.
5. **What "replay from step N" means.** A replay is a **fork of the journal into a new run**: adam copies the entries
   `1..N` of the old run into a new run (new run id, `parent` = the old run, a new thread branch on the
   orchestrator's side as `fork_thread` does) and continues from step N+1 on the new run. **The old run is never
   rewritten**: its journal, state and generation are untouched, and a replay of a finished run is allowed. The new run
   does not repeat the effects of `1..N` (they are replayed from the journal, not run again); effects after N run for
   real, so a replay is **asked for explicitly, shown with what it will do, and counted and limited as a run of the
   person** (ADR 0081). A step that was not journalled cannot be a start.
6. **Replay is adam's.** The engine has no replay today (*verified 2026-11-24*: no `replay` in the store or run API
   beyond tests of the same name that mean something else). It gets its own rows: a store method (the copy of a journal
   prefix, atomic, testkit-covered), an A2A request, and a card extension the orchestrator reads live and fails closed
   (ADR 0008); an agent that does not list it has no replay button.

## Slices

One row per PR, one repository per row.

| # | Repo | Slice | Decisive test |
|---|---|---|---|
| I1 | adam | `Store::journal_fork(run, upto) -> RunId`, atomic, in memory, Postgres, SQLite, MongoDB and the testkit | the testkit case: after a fork at step N the **original run's journal, state and generation are byte for byte what they were**, and the new run's journal is entries `1..N` |
| I2 | adam | The card extension and A2A request for a read of a run (state, journal page) and for a replay | an agent without the extension answers neither, and a replay of an unknown step is refused with nothing written |
| I3 | sys | `GET /api/threads/{id}/runs[/{run}]`: state and journal through the scrub | another person's thread is `404`, an `admin` is `404`, a planted secret in a journal payload is not in the answer |
| I4 | sys | Inbox, connections and store view | a connection URL with credentials is shown without them; counts match the store |
| I5 | sys | `POST .../runs/{run}/replay` (explicit, counted) | the original run's journal is unchanged after a replay from step N (read through I3 before and after), the new run is a branch of the thread, and the ledger counts it once |
| I6 | sys | The web tab and a `dev/` scenario | a Playwright spec on the mock server for both themes and a narrow width |
| I7 | sys | The CLI and the MCP read tools | the same ownership refusals as I3 through each surface |

## Open questions

1. Does a run's journal hold content a person did not type (tool output with repository contents)? Recommended: yes, so the
   journal view is the **owner's alone** and a shared (public) thread never shows it, as step input and output.
2. Is a replay limited to finished runs? Recommended: no, but never to a run that holds a lease.

## Consequences

An inspector is a read path with the thread's own authorisation, so it adds no standing access. The cost is a second
scrub point (adam's journal) that must stay equal to the orchestrator's rules; the I3 test holds it. Replay is the first
place the orchestrator asks adam to write a run on a person's behalf, so it is the only slice that touches the ledger.

## Alternatives rejected

- **An `inspector` role that reads any run.** Brings back ADR 0039's standing access.
- **Rewriting the journal to "rewind".** Loses the evidence the inspector exists to show.
- **Reading adam's database from the orchestrator.** Breaks the port boundary (invariant 6).

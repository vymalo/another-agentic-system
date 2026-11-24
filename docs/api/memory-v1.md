# A person's memory (v1)

A person's memory is short notes about them — *"I deploy with Helm"*, *"the staging cluster is called
kestrel"* — written by the person or by an agent they enabled, and read by an agent only when it asks. It is **off
until the person turns it on**, **one switch per agent**, and a note saved while it is on is **stored at once**: there
is no approval step ([ADR 0091](../decisions/0091-a-persons-memory-is-off-until-they-turn-it-on.md), the owner's answer
to ANR-Q59, 2026-11-24).

Memory is **tools, not context**: nothing in this page is ever put into a prompt by the orchestrator. An agent reaches a
note only through the three built-in tools of [`thread-tools/v1`](thread-tools-v1.md), and every call is a step of the
thread with its input and output.

**Status:** contract accepted 2026-11-24 with the decision; **nothing is built**. Slice **M1** (the person's own notes
and settings) comes first, then slice **M2** (the agent tools below).

## Constants


| Name | Value | Why |
|---|---|---|
| `MAX_NOTE_BYTES` | 1024 | One note is short by design; a longer text is a file ([ADR 0032](../decisions/0032-files-from-agents-live-in-an-artifact-store.md)) or a prompt ([`prompts-v1`](prompts-v1.md)) |
| `MAX_NOTES` | 200 | A person's memory is a handful of facts, not a document store. `memory_save` over it is refused, never a silent eviction |
| `MAX_READ_NOTES` | 50 | What one `memory_read` returns, newest first, whatever the person holds |
| `MAX_QUERY_BYTES` | 128 | The filter of `memory_read` is a word or a short phrase, matched case-insensitively as a substring of the note's text |

## The switch, and the grants

Two things must be true before an agent gets any memory tool, and they are different questions:

1. **The person's grant, per agent** — `GET`/`PUT /api/me/memory-settings`. Per agent id: `read` (may it read notes) and
   `write` (may it save and forget notes). **Both are `false` by default**; nothing exists for an agent the person never
   enabled. The change is the additive event `memory_switched`, so the person's own settings are as replayable as
   everything else they do.
2. **The thread's switch** — per conversation, `off` by default, set by the person. It is what makes memory a decision
   about *this* conversation rather than a global state of the account.

The tools are listed in `tools/list` **only** to a thread's **main caller**, only while the thread's switch is `on`, and
only for an agent holding the grant for that tool (`memory_read` needs `read`; `memory_save` and `memory_forget` need
`write`). A tool that is not listed is not callable: a call to it is `unknown tool`, exactly as for any other unlisted
name.

**No memory tool is listed to a child session** or to an asked agent (`ask:<n>`): a helper does not write into the
memory of the thread that spawned it ([ADR 0072](../decisions/0072-child-sessions-are-nested-threads-followed-through-child-sessions-v1.md)).

## The tools

All three take a JSON object and answer a JSON object; a refusal is an error result, never an empty success.

### `memory_read`

```json
{ "query": "helm", "limit": 10 }
```

Both members are optional. `query` filters on the note's text (case-insensitive substring, at most `MAX_QUERY_BYTES`);
`limit` is clamped to `1..=MAX_READ_NOTES`. The answer is the person's notes, **newest first**:

```json
{ "notes": [ { "id": "m-7f3a1c2e9b4d", "text": "I deploy with Helm", "savedAt": "2026-11-24T09:41:02Z", "writtenBy": "coder" } ], "total": 1, "truncated": false }
```

`writtenBy` is the member name that saved it, or `person` for a note the person wrote. `truncated` is `true` when more
notes matched than were returned or when `limit` cut the answer — the agent must never infer completeness from a short
list.

### `memory_save`

```json
{ "text": "I deploy with Helm" }
```

Stores the note **at once** and answers `{ "id": "m-…", "savedAt": "…" }`. `text` is trimmed; empty, whitespace-only, or
longer than `MAX_NOTE_BYTES` is refused. Over `MAX_NOTES` the call is refused: an agent must forget something first, and
the person sees the refusal step rather than a note silently disappearing.

### `memory_forget`

```json
{ "id": "m-7f3a1c2e9b4d" }
```

Deletes the note and answers `{ "forgotten": true }`. **Idempotent**: an id that is not there (already deleted, or never
this person's) answers `{ "forgotten": false }` — a success, so a retry is safe. It never reveals whether an id belongs
to somebody else (the answer is the same `false`).

## Errors, and which wins

Checked in this order, and the first that applies is the answer:

| Code | When |
|---|---|
| `memory_off` | The thread's switch is `off`. Nothing else is read; a call never leaks whether notes exist |
| `no_grant` | The thread is `on` but this agent lacks the grant for this tool (`read`, or `write` for save and forget) |
| `not_main_caller` | The caller is not the thread's `main` (a child session or an asked agent) |
| `too_long` | `memory_save`: `text` over `MAX_NOTE_BYTES` (the error names the bound, never echoes the text) |
| `too_many` | `memory_save`: the person already holds `MAX_NOTES` |
| `invalid` | A member of the input is of the wrong type, or `query` is over `MAX_QUERY_BYTES` |

A tool that is not listed is `unknown tool`, the same as any other name that is not there: the refusal of
`not_main_caller` and `no_grant` exists for a client that called a tool from a surface where it briefly was listed, and
the two must not differ in what they reveal.

## What the server logs

- The **call and its outcome** as a step of the thread: the tool's name, its input, its output, cut and redacted as any
  step is ([ADR 0030](../decisions/0030-a-step-carries-its-input-and-output-bounded-and-redacted.md)).
- **Never** a note's text in any other log line: not in the route logs, not in an error, not in a metrics label. A note
  is the person's words, and the log is not the place for them.
- The **event** `memory_switched` records a change of the person's settings; the note's own record is a row, not an
  event.

## The routes, in short

| Route | Method | What |
|---|---|---|
| `/api/me/memories` | `GET` | The person's notes, newest first, paged |
| `/api/me/memories` | `POST` | Write a note of their own (`{text}`) |
| `/api/me/memories/{id}` | `DELETE` | Delete one (idempotent) |
| `/api/me/memory-settings` | `GET` | The grants, per agent, and the default |
| `/api/me/memory-settings` | `PUT` | Change one agent's `read`/`write` |

No permission is needed for any of them: they are the person's own data ([ADR 0039](../decisions/0039-nobody-reads-another-persons-thread.md)),
the same rule as `/api/me/prompts`.

## Vectors

`docs/api/examples/memory/` holds the cases the Rust core and the web's mock server both pass:

| File | What it pins |
|---|---|
| [`examples/memory/tools.json`](examples/memory/tools.json) | The switch and the grants: **on → the save is stored at once**, off → `memory_off`; a missing grant → `no_grant`; a child caller → `not_main_caller`; the bounds of a note and of a read |
| [`examples/memory/isolation.json`](examples/memory/isolation.json) | Owner isolation: person B never reads, forgets or counts person A's notes, and an unknown id answers `forgotten: false` |
| [`examples/memory/settings.json`](examples/memory/settings.json) | The default of every grant (`false`), a round trip of the settings, and that a change is the `memory_switched` event |

## Limitations (what this is not)

- **Not a knowledge base.** 200 notes of 1 KiB each is the bound; anything larger is a file or a prompt.
- **Not shared.** A note is one person's. There is no team memory and no note an agent sees across people.
- **Not a thread artifact.** Memory does not travel with a thread, a fork or an export, and it is erased with the
  account.
- **Not context.** Nothing here is ever prepended to a message; an agent that wants a note must call `memory_read`.
- **Not approval-gated.** A person who does not want an agent to write does not enable it; they are not asked per note.
  What protects them is the switch, the visible step of every call, and their own page (slice M1).

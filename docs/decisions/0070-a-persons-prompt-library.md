# ADR 0070 — A person's prompt library: saved prompts with variables that are catalog questions, rendered by the server, shared by role

- **Status:** **proposed (2026-11-15)**, the design of slices P1 (saved prompts), P4 (share a prompt with a role) and the **server half of P2** (fill the
  variables and render) of [ADR 0067](0067-the-plan-to-librechat-grade-and-the-client-agnostic-rule.md), built in this change as **the API, the store, the
  contract and the mock server; no web UI** (P2's form and P3's `/` menu are later, against the mock). **Proposed**, for the owner to confirm: every
  name, default and shape below, and the [decisions the build settled](#what-the-build-settled-for-the-owner-to-confirm).
  Builds on [ADR 0039](0039-nobody-reads-another-persons-thread.md) (`admin` is content-free), [ADR 0045](0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md)
  (the audience rule), [ADR 0046](0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md) (a person's own definitions, a revision per
  write, a limit kept by the store) and [ADR 0068](0068-a-person-searches-their-own-threads.md) (a contract page with vectors that the core and the mock both
  read). **It amends invariant 3** of `CLAUDE.md` and [ADR 0001](0001-rust-state-machine-on-postgres.md) with a dated note (decision 1), as ADR 0042, 0046,
  0065 and 0068 did before it.

## Context

A person types the same instructions again and again ("review this for security, depth 3"). LibreChat has a prompt library with variables; ADR 0067 puts
it in wave 1 (P1 to P5) and fixes the rule that governs it: **every feature is implementable from `docs/api/` alone**, so a second client (the planned
Flutter app) can offer the library from the contract, and the two clients fill and render a prompt the same way.

*Verified 2026-11-15 by reading the code at `37600d7`:*

| What | Where |
|---|---|
| nothing in the orchestrator reads a question or an answer of the UI catalog: "the orchestrator relays an answer and does not read it". The validators are the web's and the mock's | `docs/api/ui-catalog-v1.md` ("Receiving an answer"), `web/src/features/chat/lib/a2ui/choices-kinds.ts`, `web/mock/receiver.ts` |
| the schema of a question is version 8 of the catalog (`Choices.questions.items`); a rule table and 133 vectors cover only the five newer kinds, and the older types' answer rules are left "to the receiver" | `web/src/features/chat/lib/a2ui/catalog/catalog.json`, `ui-catalog-v1.md` ("Version 7") |
| a person's own definitions have a precedent: its own trait keyed by `(owner, id)`, a revision from one sequence, every write a compare-and-swap, a limit kept by the store under a per-owner advisory lock, `revision` a string member of the PATCH body | `ports/src/user_tools.rs`, `store-postgres/src/user_tool_servers.rs`, `api/src/user_tools.rs` |
| `ThreadStore` already has two supertraits (`UsageLedger`, `ThreadSearch`) and three implementers; `PortSet` is built by struct literal in 58 files | `ports/src/store.rs`, `ports/src/memory/store.rs`, `store-postgres/src/store.rs`, `app/tests/inbox.rs` |
| two permission enums that must agree, a generated `config.schema.json` that a test checks, and `GET /api/config` compared as a whole body in three places | `app/src/authz.rs`, `config/src/types.rs`, `config/tests/schema.rs`, `api/tests/contract.rs`, `surface-agui/tests/history.rs`, `web/mock/server.contract.test.ts` |
| the request log writes the method and the URI and never a body | `api/src/trace.rs` |
| "Nothing deletes a person" is open work (issue 294): there is no account-erasure hook to extend | `ROADMAP.md`, ADR 0065 decision 3 |

## Decision

1. **A person's library is a table of its own, `user_prompts`** (migration `0026`), beside the log. No event records it, the core never folds it, and it is
   never copied into a thread, a fork or an export; deleting a thread erases nothing of it. It belongs to the person and goes with their account
   (issue 294, which has no code today: it must delete `user_prompts WHERE owner = $1`). **This amends invariant 3** (the text is in `CLAUDE.md`):
   *a person's prompt library (their saved prompts, and the roles they share each with) is durable state beside the log, the person's own, read and
   changed only through the API, never folded by the core and never copied into a thread, a fork or an export; a shared prompt is read by its audience
   and never names its author.*
2. **The ids are global.** A prompt is `p-` and 12 lowercase base-36 characters (`PromptId`, made from the entropy port), and the primary key is the `id`
   alone. A person never chooses an id. It must be global because a person in the audience fetches a shared prompt by its id without knowing the author.
   An id is not a capability: without the audience it is a 404, exactly as for an id that never existed.
3. **A person holds at most 100 prompts.** The store keeps the limit atomically under a two-`int4` advisory lock of the owner with a class of its own
   (`0x7072_6d70`, "prmp", not the `usrv` of tool servers). Every write is a compare-and-swap on a revision drawn from one sequence; a revision is never
   given twice for an id, not even across a delete.
4. **A prompt is** a `name` (`^[a-z0-9][a-z0-9-]{0,31}$`, unique per owner: the `/name` of slice P3), a `title` (1 to 120 characters), a `description`
   (0 to 500), a `body` (1 to 16,384 UTF-8 bytes), `variables` (0 to 8 **questions of the UI catalog's `Choices` schema, version 8, without `show` and
   without the type `images`**: both need a surface or a thread's files, which a prompt has neither of) and an `audience` (0 to 32 role values in ADR 0045's
   grammar; `"*"` only alone; empty means private). Lengths are in Unicode scalar values (the unit of JSON Schema's `maxLength` and of the search
   contract), the body in bytes, offsets in errors in UTF-16 code units (as in search).
5. **References** are `{{id}}`, with no white space. `\{{` writes `{{` and there is no other escape. A `{{` that does not start a valid reference refuses
   the save (`bad_reference`, with the UTF-16 offset), so a typo is never literal text by accident. Every reference must be declared and every declared
   variable used.
6. **`renderPrompt` is a pure substitution done by the server** (`POST /api/prompts/{id}/render`), by the rules of `docs/api/prompts-v1.md`: the answer
   is the catalog's Choices answer, validated by the catalog's rules for the five newer kinds and by rules this ADR writes down for the older ones (which
   the catalog left to the receiver); a choice renders as its **label**, a decimal verbatim, a rank as a numbered list; an optional variable left
   unanswered renders as nothing (its default is never substituted); nothing is trimmed; there is no second pass over a value; the text is at most 32,768
   bytes. It writes nothing. The server does it so that two clients cannot differ.
7. **Permissions.** Own prompts need no permission beyond the access every route needs. **Any write whose result is shared** (a non-empty audience) needs the
   new permission **`prompts.share`**, which **no role holds by default and which `admin` does not imply** (the `tools.user.add` precedent). Unsharing and
   deleting need nothing, so a person who lost the permission can still withdraw what they shared; a prompt whose share was granted earlier stays shared
   and **frozen** (only unsharing and deleting are accepted) until the permission returns.
8. **Who sees a shared prompt.** A caller sees it when the audience holds `"*"` or one of the values of their roles claim, **byte for byte**. **Unlike
   ADR 0045's agents, `admin` gives no access** (ADR 0039: `admin` is content-free). A shared prompt never names or reveals its author: the view has no
   owner, no audience and no created time, and `own: false` is all it says.
9. **The library is always on.** There is no configuration switch: the build either has it or not, and `GET /api/config` gains `prompts` as the capability
   (ADR 0067, rule 3). `getMe` lists `prompts.share` for a holder.
10. **The contract is a page with vectors**, `docs/api/prompts-v1.md` and `docs/api/examples/prompts/`: references, whole-prompt saves, renders and raw
    requests. `orch_core::prompt` (Rust) and `web/mock/prompts.ts` (TypeScript) are two implementations of it; the core also reads the catalog page's
    `vectors`, `whole-answers` and `invalid-questions` blocks, so the older rules cannot drift from the newer ones.
11. **Owner question 7 of ADR 0067** ("`prompts.share` held by no role?") is answered by the recommendation (yes: by no role) and left for the owner;
    `docs/open-questions.md` row 90 says so.

## Consequences

- **`PromptStore` is a third supertrait of `ThreadStore`: a breaking change for every implementer** (three: the memory store, the Postgres store and
  the test double `RacyStore`). A store for another database implements it beside its threads. The price is the same one search paid; the alternative
  (an associated type on `Ports`) breaks 58 files.
- Issue 294 must delete `user_prompts WHERE owner = $1`; commenting on the issue is the owner's call.
- P3 (the `/` menu) and P5 (MCP prompts) read the same port and the same page.
- A client that cannot draw a kind of question does not offer that prompt (a stored prompt may use any of the ten kinds).
- A shared prompt's author cannot be asked who wrote it; an abusive shared prompt can be withdrawn only by its author (`admin` reads no content).
- The list carries summaries and no bodies, so a person's list is bounded (100 summaries, about 100 KiB), not 3 MB.

## Alternatives rejected

- **An associated type on `Ports`.** 58 struct literals of `PortSet` break.
- **Ids per owner** (`PRIMARY KEY (owner, id)`, as tool servers). A shared `GET` would need the author's name.
- **Rendering on the client.** Two clients would differ in what they send to a model.
- **`{{ id }}` with spaces tolerated, or a `{{` that is not a reference read as literal text.** Silent mistakes.
- **An `admin` bypass on shared prompts.** ADR 0039.
- **Bodies in the list.** 100 prompts of about 33 KiB each is 3 MB per list.

## What the build settled, for the owner to confirm

The plan's twelve questions, answered by their recommendation (the work proceeds on them):

1. A single, multiple or select choice renders as the option's **label**, not its value.
2. `{{ id }}` with spaces is **refused** (`bad_reference`).
3. A person who loses `prompts.share` keeps their shared prompts shared but **frozen**: only unsharing and deleting are accepted. The server cannot see
   another person's current roles, so "hidden at once" needs more design.
4. A sharer may name roles they do not hold, `"*"` included: the permission is the gate.
5. `admin` has **no bypass** on shared prompts.
6. The library has **no configuration switch**; only `prompts.share` is gated. (So `deploy/chart/values.yaml` is not touched: the chart check reads the
   rendered configuration through the pinned orchestrator image, which refuses a permission it does not know.)
7. The list carries **summaries, no bodies**; `getPrompt` returns the whole.
8. **No rate limiter** in this slice: each call is one indexed row, writes are bounded by the 100, and a render is bounded by 32 KiB of output.
9. The limits: 100 prompts per person, 8 variables, a 16 KiB body, 32 audience values, a 32 KiB rendered text, a 64 KiB request.
10. The count stays **"10 of 35"**, with the server half of P1, P2 and P4: a slice is counted when a person can use it in the web, as search was counted only with its box. (The build first wrote "12 of 35"; corrected before the merge.)
11. Issue 294 should be told to delete `user_prompts` by owner. Recommended: yes; the comment is the owner's to post.
12. `required` is `true` when absent **for every kind**, the older ones included (verified: the web's `choices.ts` reads `required: v.required !== false`).

**After review (2026-11-15), for the owner to confirm:**

13. **Every text a person reads is checked, not only the title.** A variable's `question`, `help`, an option's `label` and `description`, `yesLabel`, `noLabel`, `unit`, `fromLabel` and `toLabel` have no control character at all (the class the title has), and the title, the description and those texts have no bidirectional embedding, override or isolate (U+202A to U+202E, U+2066 to U+2069) and no line or paragraph separator (U+2028, U+2029), all with the code of the member (`bad_title`, `bad_description`, `bad_variable` at the member). The joiners and the left-to-right and right-to-left marks stay allowed. The body is not changed. Why: `jsonb` cannot hold U+0000, so an accepted NUL in a question was a 503 that looked retryable; and a shared title is read by others. The text default of a `text` question and an `other` answer follow the rules of the text answer (a control character is `grammar`; the default has no bidi either).
14. **An integer member is any JSON number whose value is an integer in range** (`5`, `5.0` and `5e0` are 5; `5.5`, a string and a number out of range are refused). It was "integer literals". Why: a Dart or JavaScript client writes a whole double as `5.0`, and a second client must not be refused for it; it is also what `Number.isInteger` after `JSON.parse` says, so the mock agrees and its documented limitation is gone.
15. **A member twice in any object of a request is a 400** (a variable's, an option's, an answer's), not only at the top: `serde_json::Value` keeps the last of two silently. Done with a small `Strict` value in the API and a walk of the JSON text in the mock; the three (core and API, the mock, the contract) agree and a vector holds them.
16. **`from_stored` stays strict**: a row is read back through the same rules a save is held to, so that a row that is not a prompt is `Corrupt`. The consequence is the owner's to weigh: **a later tightening of a save rule needs a migration of the stored rows, or a reader that reads leniently**, else the rows that break the new rule answer 500 until repaired. (The rules of this review are such a tightening, and nothing is stored anywhere yet.)
17. **A database error that no retry will change is `Internal`, not `Unavailable`**: the port gains `PromptStoreError::Internal`; only the lock wait, the network, the pool and the SQLSTATEs that say "try again later" are `Unavailable` (a 503 a client may retry).
18. **The audience arm of every listing states the partial index's predicate** (`cardinality(audience) > 0 AND audience && $2`), because a query uses a partial index only when its `WHERE` implies the index's; before this the GIN index could not be used however big the table. `EXPLAIN` before and after is in the review's report and a test asks it. Roles that cannot be in an audience are not sent to the database.
19. **Every answer under `/api/prompts` is `Cache-Control: no-store`**, the 401, `no_access`, 404, 405 and the timeout's 503 included (one layer outside the guard).
20. **The revision sequence is global**, so a revision's value shows how much the whole deployment writes (the number of writes of all people since the sequence began). It is no secret of a person's, and the contract says a revision is only compared for equality.
21. **There is no test that "a prompt is in no export"**: no route or port joins the library to a thread's log, so a test that created both and looked for one in the other could not fail. It was deleted. What puts a prompt's words in a thread is the person sending the rendered text as a message, which is the thread's by design.
22. **A person who lost `prompts.share` cannot edit a shared prompt without unsharing it first** (the contract says so in one sentence; the web will offer "stop sharing" beside the edit).

Settled while writing the contract, also for the owner to confirm: `min` and `max` are refused on a single-choice question (the catalog does not say; a
prompt is stricter); a text answer is never trimmed by the server and an empty `text` is refused (`grammar`); a prompt's `required` rank is rendered like any
answer; an unknown query parameter of `listPrompts` is ignored; a render body with a `note` is refused (`unexpected`: a prompt has no free box).

## Facts, and how far each is proven

| Fact | State |
|---|---|
| the web reads `required` as true when absent for every kind | *verified 2026-11-15*, `web/src/features/chat/lib/a2ui/choices.ts` (`required: v.required !== false`) |
| `char_length` counts code points, as Rust's `chars().count()`; `octet_length` counts bytes | PostgreSQL documentation, *unverified* here; the Postgres tests in CI pin it |
| a `COLLATE` on a `text[]` column is accepted | *unverified*; the migration leaves it off (equality under a deterministic collation is byte equality) |
| the Postgres store | passes `prompt_store_conformance!` (26 cases) and its eight Postgres-only tests (`tests/prompts.rs`, with `EXPLAIN` of each listing) and the unit test of `db_err`, run on a local PostgreSQL 16.15 on 2026-11-15 (before and after the review); the Rust unit and API tests ran on the in-memory store. CI runs them again, and through PgBouncer in transaction mode (`orchestrator.yml`'s pooler job): *unverified* until it has |
| `dev/prompts-e2e.sh` on the real stack | *verified 2026-11-15* in part: `coder-e2e` run 43731759281 at `8e400e1` ran it on the real stack (two people, sharing, the log step) and every step passed but one, `not as theirs`, which was the script's own fault (`field` read `false` as absent with `// empty`); fixed in the review, and the rerun is the pull request's CI. Before that it was run against the web's mock server (one session per person, so the steps where a second person reads what the first shared fail there) |
| the web's mock and the Rust core read the same vectors | *verified 2026-11-15*: `orch_core::prompt` and `web/mock/prompts.ts` were written one after the other and both pass `references.json`, `prompts.json`, `render.json` and the three answer blocks of the UI catalog page, and the Rust API and the mock pass `requests.json` |

## Not done here

The web: the form that draws a prompt's variables as a `Choices` surface (P2), the `/` menu (P3), the library screen. MCP prompts (P5). Account erasure
(issue 294). A per-person rate limit on render.

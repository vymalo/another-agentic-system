# A person's team presets (v1)

- **Status:** contract written 2026-11-24 for [ADR 0074](../decisions/0074-agent-team.md) (decision 13), slice T3c. It describes what the
  orchestrator does. *Verified 2026-11-24*: the Rust API tests `teams.rs` (every answer is validated against [`chat-api.yaml`](chat-api.yaml)) pass on the
  in-memory stack. *Unverified*: a client other than those tests, and the Postgres store behind the API in a deployed stack.
- **What it covers:** the five routes `listTeams`, `createTeam`, `getTeam`, `replaceTeam` and `deleteTeam` (`/api/teams`) and `teams` in `getConfig`. A
  team that a preset starts, and the thread's own members routes, are not here (slice T3d).
- **Words:** MUST, SHOULD and MAY are used as in RFC 2119.

A preset is a saved team. It is **its owner's alone**: nobody lists, reads, replaces or deletes another person's, and `admin` has no way in. A preset
that is not the caller's is a 404, the answer for an id that never existed. No event of a thread's log records a preset: a thread that a preset started
keeps a copy, never a reference, so deleting a preset changes no conversation.

## Constants

Also served as `teams` in `getConfig`; a client reads them there and offers no preset when `teams` is absent.

| Name | Value | Meaning |
|---|---|---|
| `limit` | 50 | the most presets one person holds |
| `minMembers`, `maxMembers` | 2, 8 | the members of a team |
| `maxCharterBytes` | 4096 | the longest `charter`, in UTF-8 bytes |
| `minMaxHandoffs`, `maxMaxHandoffs` | 6, 20 | the range of `maxHandoffs`; `defaultMaxHandoffs` is what a client sends when the person picks none |
| `minPushTokens`, `maxPushTokens` | 4000, 100000 | the range of `pushTokens`; `defaultPushTokens` likewise |

The page size of `listTeams` is its own `limit` (50).

## A preset

`TeamInput` ([`chat-api.yaml`](chat-api.yaml)): `name` (`^[a-z0-9][a-z0-9-]{0,31}$`, unique for the caller), `title` (1 to 120 characters, no controls or
edge spaces), `lead` (the **name** of a member: two members may be the same agent, so an agent id would not say who leads), `members`, `charter`,
`maxHandoffs`, `handoffApproval` (`"ask"`, `"auto"` or `{"after": {"seconds": 5..600}}`; default `"ask"`), `checkpoints` (`after`: member names;
`beforePullRequest`), `pushTokens` and `summary`. **Zero is not "the default"**: `maxHandoffs: 0` is `bad_max_handoffs`. A member is `name` (as a team's
member name), `target` (`agentId`, optional `release`), `role` (at most 280 characters) and `history` (`agent-keeps`, the default, or
`orchestrator-sends`). A preset read back is the input, `id` (`t-` and 12 of `a-z0-9`), `revision` and `updatedAt`.

## The agents

**Every member's agent MUST be one the caller may use**: the caller's roles grant `agent.invoke` over it and its audience
([ADR 0045](../decisions/0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md)) leaves them in. Otherwise the save is **422 `member_agent_refused`** with `at: members.<i>.agentId`.
The answer is the same for an agent the audience shuts out and for one nobody lists, and it never names the agent. The check runs on every save,
a replace included.

## The routes

| Route | Answer |
|---|---|
| `GET /api/teams?cursor=` | 200 `{limit, held, items, nextCursor?}`; items are summaries (`id`, `name`, `title`, `lead`, `memberCount`, `revision`, `updatedAt`), ordered by `id` |
| `POST /api/teams` | 201 with the preset |
| `GET /api/teams/{id}` | 200 |
| `PUT /api/teams/{id}` | 200: the body is `TeamInput` and `revision`; **the whole preset is replaced**; a body that changes nothing writes nothing and keeps the revision |
| `DELETE /api/teams/{id}?revision=` | 204; with `revision`, only if the preset is still at it |

Every answer, the problems included, is `Cache-Control: no-store`.

## The order of the checks

401, 403 `no_access` (the caller's roles grant nothing), 415 (the body is `application/json`), the shape (400 `bad_request`, in fixed words that quote
nothing), then for a replace 404 and 409 `stale_revision`, then the values (400 with a `code` and `at`), the agents (422), and last the store (409
`name_taken`, then `too_many_teams`). A bad id or cursor is 400 (`bad_request`, `bad_cursor`).

## The rules a save is refused by

400, with `code` and `at` (`name`, `title`, `charter`, `members`, `lead`, `maxHandoffs`, `handoffApproval`, `checkpoints`, `pushTokens`; for a member
`members.<i>` or `members.<i>.<field>`): `bad_name`, `bad_title`, `bad_charter`, `bad_members` (the count, or a name twice), `bad_member_name`,
`bad_member_role`, `bad_member_agent`, `bad_member_release`, `bad_lead`, `bad_max_handoffs`, `bad_handoff_approval`, `bad_checkpoint` and
`bad_push_tokens`. They are `orch_core::TeamPreset::new` (`orchestrator/crates/core/src/team/presets.rs`); a problem never holds a word of the preset.

## Writes and revisions

A `revision` is a number of one sequence, **never given twice**, a delete included. A write that names a revision the preset is no longer at is **409
`stale_revision`**: read it again. A deleted preset stays deleted. Creating is `POST` only.

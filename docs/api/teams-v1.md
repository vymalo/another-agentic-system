# A person's team presets (v1)

- **Status:** contract written 2026-11-24 for slice T3c of the agent-team epic ([ADR 0074](../decisions/0074-agent-team.md), decision 13). Built in the
  orchestrator (`orchestrator/crates/api/src/teams.rs`, `orch_app::App::*_team_preset`, the `TeamPresetStore` port); **no web screen and no mock
  server serve it yet** (a later slice). *Verified 2026-11-24*: `orchestrator/crates/api/tests/teams.rs` and `tests/contract.rs` over HTTP against the in-memory
  stack. *Unverified*: a client other than those tests.
- **What it covers:** the five routes [`listTeams`](chat-api.yaml), `createTeam`, `getTeam`, `replaceTeam` and `deleteTeam` (`/api/teams`) and `teams` of
  `getConfig`. An implementer needs only this page and `chat-api.yaml`.
- **Not here:** starting a thread on a preset (`forwardedProps["vymalo.team"]`), adding and removing members of a thread, hand-offs and checkpoints: later
  slices of the same ADR.
- **Words:** MUST, SHOULD and MAY are used as in RFC 2119.

A team preset is a saved team: a lead, two to eight members with their agents and roles, a charter and the rules of the hand-offs. It is **its owner's and
nobody else's**: no sharing in this version, and `admin` reads none that is not its own ([ADR 0039](../decisions/0039-nobody-reads-another-persons-thread.md)).
A thread that a preset starts keeps **a copy** of the team, never a reference: deleting or changing a preset changes no conversation.

## Constants

They are what `teams` of `getConfig` says; its presence is the capability.

| Name | Value | Meaning |
|---|---|---|
| `limit` | 50 | the most presets one person holds (and the page size of `listTeams`) |
| `minMembers`, `maxMembers` | 2, 8 | the members of a team |
| name | `^[a-z0-9][a-z0-9-]{0,31}$` | a preset's name, unique among its owner's; a member's name has the team ledger's rule, the same shape |
| `title` | 1 to 120 characters | no controls, no edge spaces |
| `maxRoleChars` | 280 | a member's `role` |
| `maxCharterBytes` | 4096 | the `charter`, in UTF-8 bytes |
| `minMaxHandoffs`, `maxMaxHandoffs`, `defaultMaxHandoffs` | 6, 20, 6 | `maxHandoffs` |
| `minPushTokens`, `maxPushTokens`, `defaultPushTokens` | 4000, 100000, 24000 | `pushTokens`, the budget of a push in estimated tokens |
| `minApprovalSeconds`, `maxApprovalSeconds` | 5, 600 | the wait of `handoffApproval: {after: {seconds}}` |

The server fills in **no default**: `maxHandoffs` and `pushTokens` are required and a request without them is 400 (`bad_max_handoffs`, `bad_push_tokens` are for numbers
out of bounds, a missing member is `bad_request`). A form pre-fills the `default*` numbers.

## The preset

`TeamInput` (the body of `createTeam` and `replaceTeam`):

```json
{
  "name": "duo",
  "title": "A pair",
  "lead": "planner",
  "members": [
    {"name": "planner", "target": {"agentId": "coder"}, "role": "plans the work"},
    {"name": "builder", "target": {"agentId": "coder", "release": "stable"}, "role": "builds it", "history": "orchestrator-sends"}
  ],
  "charter": "Ship small changes.",
  "maxHandoffs": 6,
  "handoffApproval": "ask",
  "checkpoints": {"after": ["builder"], "beforePullRequest": true},
  "pushTokens": 24000,
  "summary": false
}
```

| Member | Rule |
|---|---|
| `lead` | the `name` of one of the members: an unaddressed message goes to it |
| `members[i].target` | an `AgentTarget`: any A2A agent the caller may use (below), with an optional `release` of 1 to 64 printable characters |
| `members[i].history` | `agent-keeps` (default) or `orchestrator-sends` |
| `handoffApproval` | `"ask"` (default: every hand-off waits for the person), `"auto"`, or `{"after": {"seconds": 5..600}}` |
| `checkpoints.after` | names of members; a name that is no member is `bad_checkpoint` |
| `summary` | whether the orchestrator's model summarises the older entries for the members (default `false`) |

`Team` is the `TeamInput` and `id` (`t-` and 12 lowercase letters or digits), `revision` and `updatedAt`. `TeamSummary` (a row of `listTeams`) has `id`, `name`, `title`,
`lead`, `memberCount`, `revision` and `updatedAt`: no charter, no roles.

## Members are agents the person may use

Every member's agent MUST be one the caller may invoke, as a message to it would be ([ADR 0045](../decisions/0045-admin-dashboard-in-the-web-and-agent-access-from-the-registry.md)):

1. the caller's **roles** name it: otherwise **403** `forbidden`, as for a message to it;
2. an agent whose **audience** leaves the caller out, and an agent nobody lists, are **the same answer: 422 `unknown_agent`** with `at: "members.<i>.agent"`.
   What a person may not use is, to them, not there.

The agent's live card is **not** read: a `release` is checked when a thread is started on the preset. A registry that cannot say is 503.

## Revisions

A read carries `revision` and the `ETag` (the revision, quoted). A revision is a number of one sequence the store keeps, **never given twice**, a delete included; a
client compares it for equality and never orders by it.

- `replaceTeam` **requires** `If-Match` (quoted or not, `^"?[1-9][0-9]{0,18}"?$`): without it, or with anything else, 400 `bad_request`. A stale one is 409 `stale_revision`.
- `deleteTeam` takes `If-Match` optionally; with it, a stale one is 409 `stale_revision`.
- A write that creates a second preset of the caller's with the same name is 409 `name_taken`; the 51st is 409 `too_many_teams`.

## Routes and the order of the checks

| Route | Answer |
|---|---|
| `GET /api/teams?cursor=` | 200 `{limit, held, items, nextCursor?}`, ordered by `id` bytewise; 400 `bad_cursor` |
| `POST /api/teams` | 201 `Team` and `ETag` |
| `GET /api/teams/{teamId}` | 200 `Team` and `ETag`; 404 |
| `PUT /api/teams/{teamId}` | 200 `Team` and the new `ETag` |
| `DELETE /api/teams/{teamId}` | 204 |

Every answer under `/api/teams`, problems included, is `Cache-Control: no-store`. The order of the checks is: 401, 403 `no_access` (the caller's roles grant
nothing), the id (400 `bad_request`), `If-Match` for a `PUT`, 415 (the body is `application/json`), the body (400 `bad_request`, fixed words, up to 64 KiB), **the preset
(404: another person's, whatever the body says)**, the rules of the page (400 with `code` and `at`), the members' agents (403, 422), and last the store (409).

Another person's preset, an absent one, a deleted one and the second delete are **404 with the same body**: nothing tells a caller that somebody else holds that id.
An administrator is no exception.

## Logs

Nothing of a preset's words (name, title, roles, charter) is in a log or a problem: a body that does not parse is answered in fixed words, and `at` names a field,
never a value.

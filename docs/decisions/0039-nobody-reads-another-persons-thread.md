# ADR 0039 — Nobody reads another person's thread

- **Status:** accepted (2026-10-03), on the owner's decision of the same day: "Admins shouldn't read every thread, it's
  dangerous and not GDPR compliant". **Reverses owner decision 4 of [ADR 0033](0033-the-orchestrator-is-an-oauth2-resource-server.md)**
  ("admins read every thread and act only on their own") and the built-in `admin` scope of its section 4; the rest of ADR 0033
  stands. The way to let another person read a thread is sharing, which is a later decision (ADR 0040, not written).
  **Built (2026-10-03, PR S-A):** the core of authorisation, the listing, the configuration rule, the web and the dev scenario
  (*Decision*, below).

## Context

ADR 0033 gave the built-in `admin` role `thread.read` and `artifact.read` with scope `any`: an administrator opens any
person's thread, its log and its stream, its export and its files, and lists everyone's threads (`GET /api/threads?owner=*`,
`ThreadStore::list_all_threads`, the web's Mine / All switch). The owner accepted that on 2026-10-02 as decision 4, and on
2026-10-03, before real people are invited, reversed it: "Admins shouldn't read every thread, it's dangerous and not GDPR
compliant."

What a thread holds: everything a person typed to an agent, what the agent answered, the tool input and output of every step
(which can carry repository contents and credentials the person pasted), and the files an agent handed over. It is personal
data and, often, more than the person meant to say. A role that can read all of it by default, without the person's knowledge
and without a reason that is recorded anywhere, is a standing access that nothing limits.

The principles the owner points at (*unverified*: the article numbers are from the text of Regulation (EU) 2016/679 as known to
the author, not fetched on 2026-10-03; this is a design reason, not legal advice): **data minimisation**, Article 5(1)(c) (personal
data adequate, relevant and limited to what is necessary), and **security of processing**, Article 32 (access to personal data
limited to what each role needs, by default). Reading every person's thread is not needed to run the service, so the access should
not exist rather than exist and be promised unused.

What the code did (*verified* 2026-10-03 at `5349ba6`): `RoleGrant::admin()` was `user` plus `admin` with `read: Scope::Any`;
`Scope` had `Own` and `Any`, and a role's `scope` in `auth.roles` could grant `any` to any role, for reading and for acting
separately; `GET /api/threads?owner=<e-mail>|*` listed another person's or everyone's threads for a role that held `admin`
and a `thread.read` of scope `any`; an administrator's view of another's thread was read-only (403 `read_only` on every act);
the web's `isAdmin` showed Mine / All and the "Read only: this is alice@example.com's thread" notice.

## Decision

1. **No role reads or acts on another person's thread.** A permission over threads (`thread.read`, `thread.write`,
   `artifact.read`) reaches the threads the person owns and no others, for every role, `admin` included. Another person's thread
   is `404`, the answer for a thread nobody has, on every read and every act (the API, the AG-UI stream and run, the MCP tools,
   the files route). The structure carries it, not a default: `Scope` has one value, `own`, and `RoleGrant` no longer has a read
   scope and a write scope; `Access::scope` answers `own` for the three permissions over threads. There is nothing to configure
   wrongly.
2. **A configuration cannot ask for it.** `auth.roles.<role>.scope` stays a key of `version: 1` (so a file that says `own`, or
   `{ read: own, write: own }`, keeps working), and `any` is refused at startup, in the rules pass, at its own key, for reading and
   for writing, exit 78:
   `auth.roles.<role>.scope: any is refused: reading or acting on another person's thread is not a permission (ADR 0039); share the thread instead`
   (`.scope.read` and `.scope.write` for the split form). Reversing this needs a new ADR, not a line of YAML.
3. **`GET /api/threads?owner=` is removed.** The listing is the caller's own. A request that still carries an `owner` parameter,
   with any value, is `400` with `owner is not supported (ADR 0039)`: louder than ignoring it, so a client that thinks it lists
   everyone's threads learns it does not. `ThreadStore::list_all_threads` is removed from the port, the in-memory and Postgres
   stores and the testkit; `App::list_threads_of` and `Owners` are gone, `App::list_threads` takes no owner.
4. **What `admin` means afterwards: an operational, content-free permission.** It is kept as a name, held by the built-in `admin`
   role, listed in `GET /api/me`, and it gates nothing today beyond what `user` has. It is reserved for endpoints that expose no
   thread content and no personal data beyond counts (for example agent and registry health, a count of threads per agent, a
   configuration check); an endpoint that shows a person's content is not one it may gate. The built-in `admin` role is a user who
   holds it: it reads, starts and acts on its own threads as a user does, and on nobody's else.
5. **Break-glass is outside the application.** A legal request or an abuse case is handled by an operator with database access
   under the deployment's own controls (who may reach the database, how it is logged, who approves), written in the deployment's
   runbook. There is no API for it, so there is no role to hold by mistake, and no query the application could be asked to run.
6. **Export is the owner's.** `GET /api/threads/{id}/export` follows `thread.read`, so it is `404` for anyone but the owner.
7. **`read_only` goes with it.** The `403` with `code: read_only`, "a thread the person may read and not change", existed only for
   the administrator's view of another's thread; nothing produces it any more. `AppError::Forbidden` loses its `read_only` flag and
   the problem's code is always `forbidden`. A role that holds `thread.read` and not `thread.write` is still refused its acts
   with `403 forbidden`, and the web still shows its threads as read-only. A later read-only view (a shared thread) is its own
   resource and its own answer (ADR 0040).

### What changes where

| Where | Change |
|---|---|
| `orch_app::authz` | `Scope::Own` only; `RoleGrant { permissions, agents }`; `RoleGrant::admin()` = user + `admin`; the match arm for `any` is gone |
| `orch_app::App` | `list_threads` (own); `list_threads_of`, `Owners` and `may_list_others` removed; a thread that is not the person's is `NotFound` for acts as for reads; `AppError::read_only_thread` removed |
| `orch_ports`, `orch-store-postgres` | `ThreadStore::list_all_threads` and its conformance case removed |
| `orch_config`, the binary | `any` refused in the rules pass; `scope` carries no reach into the policy |
| `orch-api` | `owner` is a `400`; `code: read_only` removed; `docs/api/chat-api.yaml` follows |
| `web/` | the Mine / All switch, the admin's view of others' threads and the "Read only: this is someone's thread" notice are removed; `isAdmin` is not used to show anything of anyone's content |
| `dev/` | the dev roles say `own` (or nothing); `dev/rbac-e2e.sh` asserts the content-free administrator |

## Consequences

- An administrator can no longer help a person by opening their thread. A person who wants help shares the thread (ADR 0040) or
  exports it themselves (`GET /api/threads/{id}/export`, the web's Export JSON) and sends the file.
- Nobody can read a thread to investigate abuse through the application. That is the intent. The operator's path is the database
  (decision 5), which is more work and leaves its own trail, which is the point.
- A configuration written for ADR 0033 with `scope: any` or `{ read: any }` stops starting. The error names the key and this
  decision. The dev stack's own files never used `any` except the built-in `admin`, which no file spells out.
- `GET /api/me` still reports `scope: own` for the three scoped permissions, so a client written against ADR 0033 reads the same
  shape. It never reports `any` again.
- Clients that listed threads with `?owner=` (the web's All switch, an operator's script) get `400`. The web was the only client
  in this repository and is changed in the same PR.
- The built-in role named `admin` now differs from `user` only by holding `admin`, which gates nothing yet. That is an honest
  state, and recorded here so that nobody reads the role's name as a privilege.
- Sharing, when it exists, is the one way a second person reads a thread; its authorisation is a different resource kind and
  will not widen `own`.

## Alternatives rejected

- **Keep `any`, default it off.** A configuration could re-grant it by accident or under pressure, and a future role would
  inherit a code path that reads everything. Removing the code path removes the question.
- **Keep `owner=` and ignore it.** A client that asks for everyone's threads would be given its own and think it was complete.
  A `400` says what happened.
- **A recorded, audited admin read ("break glass" in the application).** It is access to personal data by a role, with a log
  nobody may read in time. The need is rare and the operator already has the database; an application feature would exist all
  the time to serve a case that is not.
- **Reading allowed for `admin` on threads the owner has shared.** That is sharing, and is decided separately (ADR 0040); it
  needs no role.

## Status: built in S-A

**Built (2026-10-03, PR S-A).** *Verified 2026-10-03 by the change's own tests, run locally:* the matrix of `authz` (the administrator
over another's thread: no, for read, write and files) and `orch-app` `tests/authz.rs`; `orch-api` `tests/rbac.rs` (an administrator's
`GET`, export, branches, files and every act on another's thread are `404`, `?owner=` is `400` for every role) and
`tests/contract.rs` (the `400` of `listThreads` is now documented); the AG-UI and MCP role tests; the configuration rule
(`orch-config`) and the binary's exit `78` for a `scope` of `any` (`tests/smoke.rs`); the web's contract test against its mock, its
jsdom tests and its Playwright specs `roles` and `tools` against the mock. **Unverified here:** `dev/rbac-e2e.sh` against the
compose stack (it runs in CI, `coder-e2e.yml`), and the web in a real browser with a real issuer.

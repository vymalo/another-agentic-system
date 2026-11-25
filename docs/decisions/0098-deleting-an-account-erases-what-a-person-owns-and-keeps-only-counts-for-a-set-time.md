# ADR 0098 — Deleting an account erases what a person owns and keeps only counts, for a set time

- **Status:** **Proposed (2026-11-25).** Task ANR-768, the first slice of [issue 294](https://github.com/vymalo/another-agentic-system/issues/294)
  ("delete my account"). Nothing is built: the route, the port and the sweep below are what the next slices will make. The owner asked for
  the feature (ANR-Q110); the **keep period and its default are the planner's**, for the owner to confirm (*Open for the owner*).
  Builds on [ADR 0043](0043-deleting-a-thread-erases-it.md) (what erasing a thread does), [ADR 0039](0039-nobody-reads-another-persons-thread.md)
  (nobody reads another person's data) and [ADR 0094, retention](0094-retention-is-a-sweep-per-class-with-a-key-each-and-a-default-of-90-days.md)
  (one key per class). It settles what [ADR 0065](0065-a-usage-ledger-per-person.md) decision 3 left to this issue, and
  amends invariant 3 of `CLAUDE.md` only by naming one more erase.

## Context

A person's data is in more places than their threads. *Verified 2026-11-25 by reading the ADRs named; no code was read or run:*

| Record | Decided in | What it holds | Today |
|---|---|---|---|
| Threads: log, outbox, agent binding, watches, timers | [0043](0043-deleting-a-thread-erases-it.md) | the conversation | `DELETE /api/threads/{id}` erases one thread, refused while it works |
| The files agents made, and uploads | [0032](0032-files-from-agents-live-in-an-artifact-store.md), 0043 | bytes in the artifact store | erased with the thread, by the purge sweep |
| Share links | [0040](0040-thread-sharing-by-revocable-link.md) | a nonce on the thread row | go with the thread |
| The organisation of the list: pin, archive, order, nesting | [0042](0042-the-thread-list-is-the-owners.md) | columns of the thread row | go with the thread |
| The search index of the person's threads | [0068](0068-a-person-searches-their-own-threads.md) | folded words of titles and messages | erased with the thread |
| The person's own MCP servers, and their sealed credentials | [0046](0046-people-add-their-own-mcp-servers-secrets-in-a-credential-broker.md) | definitions in Postgres, secrets in the `SecretStore` | each server's delete revokes at the broker, then deletes |
| The prompt library and its shares | [0070](0070-a-persons-prompt-library.md) | prompts, the roles each is shared with | deleted by the person, one by one |
| The person's memory | [0091](0091-a-persons-memory-is-off-until-they-turn-it-on.md) | short notes and the grants | erased with the account (that ADR says so) |
| Push devices, notification settings, presence, intents | [0077](0077-push-notifications-for-a-persons-runs.md) | sealed device tokens | erased with the thread or the account |
| The usage ledger | [0065](0065-a-usage-ledger-per-person.md) | counts per task and month, no content | **kept**, with the thread id cleared; "decides later how much of the ledger the law requires to keep" |
| The link to an external account (GitHub) | [0064](0064-the-person-reaches-the-agent-and-links-their-github.md) | who linked which account, the tokens sealed | **not built**; the unlink there is revoke, then delete |

There is no account row: a person is the key their credential gives (`getMe`). So "an account" is every record above whose owner is that key.

## Decision

1. **The route is one confirmed delete, `DELETE /api/accounts/{account}`,** where `{account}` is the person's key (`getMe` `user`).
   Only that person may call it: any other caller, an administrator included, gets **404**, the one answer for "no such account" and "not yours"
   (ADR 0039). The caller confirms by sending the key again as `?confirm=`; a different value is a `400 confirmation_mismatch` and nothing is
   touched. A role withholds it by not holding the permission **`account.delete`** (403 `forbidden`), as `thread.delete` can be withheld.
   The contract is [`chat-api.yaml`](../api/chat-api.yaml) `deleteAccount`.
2. **A job in progress refuses it: `409 account_busy`.** The delete does not stop work for the person: while any thread of the caller, or an edit
   of one, is `queued`, `working` or `verifying`, or has a running ask, nothing is erased. The answer names how many threads work and no more.
   A client offers "Stop and delete" as ADR 0043 does (stop, wait, ask again). The check and the erase are one decision of the store: a thread that
   starts work between them makes the erase fail with the same 409, and nothing is half erased.
3. **What is erased, at once, and not recoverable:** every record in the table above except the usage ledger and the GitHub link, in this order:
   1. **Revoke what lives elsewhere first.** Each of the person's MCP servers is revoked at the broker and then deleted (ADR 0046); a broker that
      cannot be reached is `503`, `Retry-After`, with nothing erased, so a credential is never left that nobody can name. (The GitHub link, when
      ADR 0064 is built, is revoked at GitHub and deleted in this step; until then there is none.)
   2. **One transaction** deletes the person's threads with everything on their rows (ADR 0043: log, outbox, bindings, watches, timers, share,
      organisation of the list, search words), their prompts and shares, memory notes, push devices and settings, and writes one purge row per
      thread so the files follow.
   3. **The purge worker erases the files** (ADR 0043's sweep); the answer does not wait for it.
   Forks other people made of the person's shared threads are **theirs** and stay (ADR 0029: a fork is a conversation of its own). A prompt someone
   shared *with* the person is untouched: it is another person's.
4. **What is kept, and for how long: counts, not content, for `account.erase.keepDays`** (the key is named in *Configuration*; default **90**,
   ADR 0094's default). The usage ledger rows of the person (provider, model, counts, sandbox time, month, no thread id since ADR 0065) are kept so a
   limit or a receipt already issued can be explained, and **the owner on each row is replaced in the same transaction by a pseudonym that is not
   the key and cannot be turned back into it** (a keyed hash with a deployment secret), so the rows no longer name the person. A sweep (ADR 0094's
   shape: one pass, batches of 500, `SKIP LOCKED`) deletes them once they are older than `keepDays`. **`0` is not a value, and `forever` is refused
   for this class**: keeping a deleted person's counts for ever is not a choice the key offers. The ledger's own `usage.retentionMonths` still
   applies and the shorter of the two wins.
5. **What the answer says it cannot erase,** and the web says in its dialog, in these words' meaning:
   - **backups** of the database and of the artifact store, until they expire (the operator's retention);
   - the **A2A agent's own store** of its context, and, in a build that hosts an agent in-process, the agent's journal rows, which age out by
     ADR 0094's `agentJournalDays`;
   - **the model provider** that saw the conversations, under its own terms;
   - **the identity provider's account**: this system never holds it. Deleting it is the operator's, in the identity provider; the dialog links to
     where the operator says to do it (`account.erase.identityNote`, a string, empty by default), and a person who signs in again starts with
     an empty account.
6. **The answer** is `202` with `{"status": "erasing", "threadsErased": n, "filesPending": m, "keptUntil": <time or null>}`: `202` because the files
   follow; `keptUntil` is when the counts go, `null` when the person had no usage rows. A second call is a `404` (nothing is left of the account to
   delete) and is not an error to a client that retries. **No other person is told, and nothing is sent to the person**: an e-mail is not a feature
   of this system.
7. **The event is counted, not recorded by person:** a counter `accounts_erased_total` and a log line with counts and no key. Nothing keeps that a
   given person was erased: a "deleted" list would be the data the person asked to lose.

## Slice table

| # | Slice | Decisive test | Repository |
|---|---|---|---|
| E1 | This ADR and the contract (`deleteAccount`, the permission, the two problem codes) | the contract lints and `tools/docs-check` passes | sys |
| E2 | The `AccountErase` port and its testkit, an in-memory adapter | after an erase the person's threads, prompts, memory, devices and servers read as empty and another person's are untouched; the ledger rows hold no key | sys |
| E3 | Postgres adapters, the route, the revoke step, the keep sweep and the metric | the same on Postgres in one transaction; a job in progress is `409 account_busy` and nothing changed; a broker down is 503 and nothing changed; another caller is 404; a ledger row older than `keepDays` is gone and a younger one stays | sys |
| E4 | The web: the dialog with the list of what goes, what is kept and what cannot be erased, then the e2e on mocks (`dev/` scenario) | the scenario signs in, creates a thread, deletes the account and reads `404` for the thread and an empty list | sys |

## Configuration

`account.erase.keepDays` (default 90), `account.erase.identityNote` (default empty) and `account.erase.pseudonymKeyEnv` (the name of the variable that
holds the keyed-hash secret; a deployment that enables the route without it is refused at startup, exit 78, as ADR 0094 refuses a bad key). The
permission `account.delete` is in no role by default: a deployment grants it to the roles it wants.

## Open for the owner

1. **90 days** for the kept counts is ADR 0094's default taken, not the owner's. A shorter period needs no change of design; a longer one needs a reason
   the person can be told.
2. **No grace period**: the erase starts when asked. A cooling-off (a delete that waits a few days and can be cancelled) would need a state this ADR
   does not draw; say so if wanted.
3. **`account.delete` is withheld by default.** The alternative is to grant it with the base role; the planner chose the safer default.

## Consequences

- One more path erases a person's data and it reuses ADR 0043's transaction and sweep, so the erase of a thread stays the one proven path.
- A person's counts survive their account for a stated time and then go; the ledger never names the person after the erase.
- The identity provider is out of reach, and the dialog says so instead of promising it.

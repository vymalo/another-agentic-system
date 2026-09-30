# Webhooks: CI results

How a CI system reports the result of a check run on a pushed commit to the orchestrator
([ADR 0017](../decisions/0017-ci-results-by-webhook.md)). Two routes, one meaning: a signed report
that a named check finished with a conclusion, for a commit in a repository. The orchestrator
matches it to the job that pushed that commit and lets the gate decide
([ADR 0018](../decisions/0018-verification-gate-and-rework-loop.md)).

> Status (2026-09-30): **both routes are built** (MVP slices 6 and 9; `orch-surface-webhook`, the surface names
> `webhook-generic` and `webhook-github`). The GitHub adapter was built against **synthetic** payloads shaped after
> GitHub's documented schemas, not recorded deliveries (see [Verified and unverified](#verified-and-unverified-2026-09-30)).
> Facts about GitHub are marked *verified* with a date and a source, or *unverified*.

## Routes

| Surface (`ORCH_SURFACES`) | Route | Sender | Signature | Body limit |
|---|---|---|---|---|
| `webhook-github` | `POST /webhooks/github` | GitHub webhooks | `X-Hub-Signature-256` over the raw body | 5 MiB |
| `webhook-generic` | `POST /webhooks/ci` | any CI | `X-Vymalo-Signature-256` over `"<ts>.<body>"` | 256 KiB |

Both are machine routes: no user identity, no cookie, and `X-Auth-Request-Email` is never read. The
edge must route `/webhooks/*` to the orchestrator without injecting an identity header. A route is
mounted only when its name is in `ORCH_SURFACES`, and its secret variable is then required
(`WEBHOOK_GITHUB_SECRETS`, `WEBHOOK_GENERIC_SECRETS`; up to two comma-separated secrets, for
rotation; a missing one is exit 78, and so is a third secret, and so is a secret shorter than 32 bytes:
`openssl rand -hex 32`). Each secret is trimmed, and a blank one counts for nothing. A signature is accepted if it
matches either secret. A `worker` role serves no routes and does not need the secrets. The secrets are never logged
and never in a `Debug` or an error message.

**Nothing counts unless the gate names it.** A gate that requires `ci` must name at least one check in `ci.required`
(the check's name for GitHub's `check_run`, the workflow's name for `workflow_run`, the `name` of a generic report):
the orchestrator does not start (exit 78) with a deployment or an agent entry that requires `ci` without names
(`ORCH_CI_REQUIRED` names them for the deployment), and a per-thread request that adds `ci` on top of a policy with
none is a `400`. Only a named check decides; every other report is a card and nothing else. (The rule that "the first
completed report decides" was dropped on 2026-09-30: it let a red commit pass on a `skipped` report of another check,
or of another workflow.) A process that mounts neither webhook surface does not honour `ci` at all: it refuses
it, in every layer and per thread, with "no CI webhook surface is mounted (ORCH_SURFACES)".

**Reading a request is bounded.** The body is read incrementally up to the route's limit, never sized from the
`Content-Length` the sender declares, and the whole request must arrive within 10 seconds (`408`, nothing stored).
The refusals are logged once per 30 seconds per route at `warn`, with the number that were held back; the others are
`debug`.

The signature is checked on the bytes as received, **before anything is stored**. The body is a
report about a commit; it cannot start a job, and the only thing a valid report changes is a check
result.

## Generic signed shape (`POST /webhooks/ci`)

### Headers

| Header | Value | Notes |
|---|---|---|
| `Content-Type` | `application/json` | |
| `X-Vymalo-Delivery` | any short visible text (a UUID is fine) | **Optional, and only logged.** It is not signed, so it cannot be what makes a delivery new: anyone who captured a request could replay it under a fresh id. The idempotency key is the SHA-256 (lower-case hex) of the signed string `"<ts>.<body>"`: the same timestamp and body are one delivery, whatever the header says or whether it is there at all |
| `X-Vymalo-Timestamp` | Unix time, whole seconds | ASCII digits only (no sign, blank or fraction; at most 12). Must be within plus or minus `WEBHOOK_GENERIC_MAX_SKEW_SECS` (default 300, at least 1; the boundary is inclusive) of the orchestrator's clock |
| `X-Vymalo-Signature-256` | `sha256=` + lowercase hex | `HMAC-SHA-256(secret, "<ts>.<body>")`, where `<ts>` is the exact `X-Vymalo-Timestamp` text, then a full stop, then the raw body bytes |

### Body

```json
{
  "version": 1,
  "repository": "https://github.com/acme/widgets",
  "sha": "0123456789abcdef0123456789abcdef01234567",
  "branch": "agent/fix-flaky-test",
  "name": "ci/build",
  "conclusion": "success",
  "url": "https://ci.example.com/runs/42",
  "summary": "212 tests passed"
}
```

| Field | Type | Required | Meaning |
|---|---|---|---|
| `version` | integer | yes | Always `1`. Another value is refused |
| `repository` | string (URL) | yes | The repository. Normalised to the key `host/owner/name` (lower-cased, without `.git` or a trailing slash) |
| `sha` | string | yes | The commit the check ran on: 40 hexadecimal characters (case-insensitive; stored lower-case) |
| `branch` | string | no | The branch, for display. It is **not** used to find the job |
| `name` | string | yes | The check's name (`ci/build`). It is what `gate.ci.required` lists. Not blank |
| `conclusion` | string | yes | One of the eight values below, in lower case. Anything else is a `400` |
| `url` | string (URL) | no | A link to the run, shown on the card. Only `http` and `https` are kept |
| `summary` | string | no | A short text, shown on the card and quoted as untrusted data in a rework prompt. Truncated to 16 KiB (at a character boundary) |

Unknown members are ignored, so a sender can add fields without breaking; a required member that is missing or mistyped is a `400`.

### Conclusions

The set is closed. It is GitHub's check-run vocabulary, so both routes speak it.

| Conclusion | Passes the gate? |
|---|---|
| `success` | yes |
| `neutral` | yes |
| `skipped` | yes |
| `failure` | no |
| `cancelled` | no |
| `timed_out` | no |
| `action_required` | no |
| `stale` | no |

Everything that is not `success`, `neutral` or `skipped` fails. A failed report sends the job back
to the agent with the report's `name`, `conclusion`, `url` and `summary` as findings, until
attempts are spent. Which reports are counted is the gate's `ci.required`: only those names count, and all must pass
(a gate that requires `ci` names at least one, see above). Every report gets a card in the chat, one of its own
(the card's id is made of the provider, the commit, the name and the report's place in the log, and it never
replaces another: [`agui.md`](agui.md#ci-results-vymalo-ci)); the verdict of the gate is the `vymalo.check` card of
source `ci`.

### Response codes

| Code | When | Stored? |
|---|---|---|
| `202 Accepted` | The signature and body are valid. Also for a repeat of the same timestamp and body | Once |
| `400 Bad Request` | The signature is valid but the body is not: malformed JSON, a missing or mistyped field, a `version` other than `1`, a bad `sha`, a `conclusion` outside the list. An RFC 9457 problem | No |
| `401 Unauthorized` | A header is missing, the timestamp is not plain digits or is outside the skew window, or the signature matches no configured secret. Also a request that is not a `POST`, because the guard comes first | No |
| `408 Request Timeout` | The request did not arrive within 10 seconds (a body that trickles in, or stalls) | No |
| `413 Content Too Large` | The body is over 256 KiB | No |

A `202` means "received", not "applied": the report is matched to a job by a worker, and a report
that arrives before the job's commit is known is kept (up to `INBOX_PARKED_TTL_SECS`, default a
day) and applied when it is. The sender does not need to retry a `202`. On a `401` it should fix
the secret or the clock rather than retry.

### Worked example

Take the secret `dev-webhook-secret-0123456789abcdef0123` (the compose stack's; a real one is
`openssl rand -hex 32`), the timestamp `1790800000` and the body above, written to
`body.json` with no trailing newline (the signature covers every byte; `printf` adds none, an
editor often does):

```sh
SECRET=dev-webhook-secret-0123456789abcdef0123
TS=1790800000
BODY='{"version":1,"repository":"https://github.com/acme/widgets","sha":"0123456789abcdef0123456789abcdef01234567","branch":"agent/fix-flaky-test","name":"ci/build","conclusion":"success","url":"https://ci.example.com/runs/42","summary":"212 tests passed"}'

# the signature: HMAC-SHA-256 over "<ts>.<body>", as lowercase hex
SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1)
echo "$SIG"
```

This prints (computed with OpenSSL on 2026-09-30):

```text
e7ff72c4411e69debb1f634a339e7c884641e4ada663a42369deead91e617944
```

Post it, with the current time in place of the fixed one (the fixed timestamp is outside the skew
window of a live orchestrator, so use `TS=$(date +%s)` and recompute `SIG`):

```sh
TS=$(date +%s)
SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1)
curl -sS -i https://orchestrator.example.com/webhooks/ci \
  -H 'Content-Type: application/json' \
  -H "X-Vymalo-Timestamp: $TS" \
  -H "X-Vymalo-Signature-256: sha256=$SIG" \
  --data-binary "$BODY"
```

`--data-binary` sends the bytes exactly as they are signed; `-d` would strip newlines. Expect
`202`. Change one byte of the body, or send an old timestamp, and expect `401` and no stored
report. The dev script [`dev/ci-webhook.sh`](../../dev/ci-webhook.sh) does this for either shape, and `dev/ci-e2e.sh`
uses it to drive a gated job to `done`.

### Known-answer vectors

The signature is `HMAC-SHA-256(key = secret, message = "<timestamp>.<body>")`, lowercase hex. These are
checked by the unit tests of `orch-surface-webhook` (`signature.rs`) against the implementation, and were
computed with OpenSSL 3 on 2026-09-30 (`printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r`).

| # | Secret | Timestamp text | Body | Signature (`sha256=` + hex) |
|---|---|---|---|---|
| 1 | `dev-webhook-secret-0123456789abcdef0123` | `1790800000` | the `$BODY` of the worked example above (compact, 250 bytes, no trailing newline) | `e7ff72c4411e69debb1f634a339e7c884641e4ada663a42369deead91e617944` |
| 2 | `Jefe` | none: the message is the two parts of RFC 4231 test case 2, `what do ya want ` and `for nothing?` | | `5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843` (the algorithm's own vector) |

The idempotency key of vector 1 is `sha256("1790800000." + body)` in lower-case hex,
`08b2184adb33f0056f3916d368f0654c615b595995a9fa1089737788724a0db5` (`printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256`).

Skew vectors, for a clock at `T` and `WEBHOOK_GENERIC_MAX_SKEW_SECS` = 60 (the tests hold the clock):
timestamps `T-60`, `T`, `T+60` are accepted; `T-61`, `T+61` are `401`; `+T`, ` T`, `T.0`, `-1`, the empty
string and a 13-digit number are `401` even when signed exactly as written. Two secrets `new,old`: a
signature by either is accepted, by a third is `401`.

## GitHub adapter (`POST /webhooks/github`)

Configure a repository (or organisation) webhook with the payload URL
`https://<orchestrator>/webhooks/github`, content type `application/json`, the secret set to a value
in `WEBHOOK_GITHUB_SECRETS`, and the events **Check runs** and **Workflow runs**. Do **not** subscribe to *Check
suites*: a suite is named by the app that ran it (`github-actions`), not by a check, so a route that took it would
name every check of that app alike (a delivery of it is acknowledged and stored nowhere).

### Request

| Header | Use |
|---|---|
| `X-Hub-Signature-256` | `sha256=` + hex `HMAC-SHA-256(secret, raw body)`. Checked first. Missing or wrong: `401`, nothing stored |
| `X-GitHub-Event` | Selects the parser for the body below. **Not signed**: whoever holds a captured delivery can change it, so the body has to be what that event's parser demands (a `check_run` header on a `workflow_run` body is a `400`) |
| `X-GitHub-Delivery` | Unique per delivery, and not signed: only logged. The idempotency key comes from the signed body (below) |

For a GitHub delivery the signature is over the body alone (no timestamp). With the same secret and
body as the example above:

```sh
printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1
# 3f7810292c8978124166e4004b825bbb80dbd5b64b72ddecd8099f832ba345d0
```

(That body is the generic shape; it is used here only to show the computation. GitHub signs its own
payloads.) GitHub's own documented vector, *verified 2026-09-30*
(<https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>): secret `It's a Secret to Everybody`,
payload `Hello, World!` (no newline) gives
`sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17`. Both are unit tests of `signature.rs`.

### Events accepted

Only the events below, and only with `action` = `completed`, become a report. Every field is read
from the payload; nothing else is trusted.

| `X-GitHub-Event` | Read | Becomes |
|---|---|---|
| `check_run` | `check_run.head_sha`, `check_run.check_suite.head_branch`, `check_run.conclusion`, `check_run.name`, `check_run.html_url`, `check_run.output.summary`, `check_run.id`, `check_run.completed_at`, `check_run.pull_requests[].head.repo`, `repository.html_url` | a report named by the check's name |
| `workflow_run` | `workflow_run.head_sha`, `workflow_run.head_branch`, `workflow_run.conclusion`, `workflow_run.name`, `workflow_run.html_url`, `workflow_run.id`, `workflow_run.run_attempt`, `workflow_run.updated_at`, `workflow_run.head_repository`, `repository.html_url` | a report named by the workflow's name |

Both normalise to the same report as the generic body: `repository` from
`repository.html_url`, `sha` from `head_sha`, `branch` from `head_branch`, then `name`,
`conclusion`, `url`, `summary`. Conclusions use the closed list above; a value outside it (or a
missing one) counts as `failure`, so an unknown outcome never passes. As built:

| Member | `check_run` | `workflow_run` |
|---|---|---|
| `name` | `name` (required, cut to 256 bytes) | `name`; **a run whose name is `null` or blank is acknowledged (`202`) and not stored** |
| `branch` | `check_suite.head_branch` | `head_branch` |
| `url` | `html_url`, if `http(s)` (stored as parsed) | `html_url`, if `http(s)` (stored as parsed) |
| `summary` | `output.summary`, cut to 16 KiB, absent when blank | none |
| idempotency key | `check_run:<id>:<completed_at>` | `workflow_run:<id>:<run_attempt>` |
| dated by | `completed_at` | `updated_at` |

**`check_suite` is not accepted** (it is `202`, not stored): it names an app, not a check, so it cannot be told from the
next suite of the same app, and "the app" is not what a gate names.

**The idempotency key is made of the signed body**, not of `X-GitHub-Delivery` (which GitHub changes on a redelivery
and which anyone can change on a captured request): a redelivery, or a replay under any id, is the same report and is
stored once. A different check run, or the same run completing again later, is another.

**A report must be dated within `WEBHOOK_GITHUB_MAX_AGE_SECS`** (default 86400, at least 1) of the orchestrator's clock:
an event whose signed `completed_at` (`updated_at` for a workflow run) is older is acknowledged (`202`) and not stored,
so a captured delivery is not replayable for ever. A missing or unreadable date is a `400`.

**A report of a fork's code is acknowledged (`202`) and not stored.** A `workflow_run` whose `head_repository` is not
the `repository` (by id, or by full name when there is no id) or has no `head_repository`, and a `check_run` any of whose
`pull_requests[].head.repo` is not the `repository`, ran code the repository does not hold, under a workflow or check
name the fork chose. Its `head_sha` is not a commit the agent pushed to this repository.

`startup_failure`, which GitHub can report for a workflow run that could not start, is kept as its own conclusion (it
fails); any other value outside the list, and `null`, is `failure`. `X-GitHub-Event` is read after the signature is
good: a missing event is `400`. Only `X-Hub-Signature-256` decides `401`. An event this page does not list is `202`
whatever its body (it is not parsed).

### Response codes

| Code | When | Stored? |
|---|---|---|
| `202 Accepted` | A `check_run` or `workflow_run` with `action` = `completed`, validly signed. Also a repeat of the same report, and every event or action that is not stored: another action, another event (`check_suite` included), an unnamed workflow, a fork's run, an event older than the maximum age (those are acknowledged so GitHub does not retry or flag them) | Only the first kind, once |
| `204 No Content` | `ping` (sent when the webhook is created), validly signed. Not parsed | No |
| `401 Unauthorized` | `X-Hub-Signature-256` missing, or matching no configured secret | No |
| `408 Request Timeout` | The request did not arrive within 10 seconds | No |
| `413 Content Too Large` | The body is over 5 MiB | No |
| `400 Bad Request` | Validly signed, but no `X-GitHub-Event`; or an accepted event that is not JSON, has no `action`, or lacks a member it is read from (`repository.html_url`, the commit, the check's name, the run's id, its date). An RFC 9457 problem | No |

GitHub expects a `2xx` within 10 seconds; the route stores the report and answers, and the worker
does the rest, so a busy orchestrator does not turn into failed deliveries.

### Verified and unverified (2026-09-30)

- *Verified*: `X-Hub-Signature-256` is `sha256=` plus the hex HMAC-SHA-256 of the raw payload,
  compared in constant time. <https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries>
- *Verified*: the headers `X-GitHub-Event` and `X-GitHub-Delivery`; payloads are capped at 25 MB;
  the events and their actions. <https://docs.github.com/en/webhooks/webhook-events-and-payloads>
- *Verified*: a receiver should answer with a `2xx` within 10 seconds.
  <https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks>
- *Verified*: the check-run `conclusion` values are `success`, `failure`, `neutral`, `cancelled`,
  `skipped`, `timed_out`, `action_required`, `stale`. <https://docs.github.com/en/rest/checks/runs>
- *Verified in the REST schemas* (check suites, workflow runs): `head_sha`, `head_branch`,
  `conclusion`, `repository.html_url`.
- *Verified 2026-09-30* (the REST page for workflow runs, <https://docs.github.com/en/rest/actions/workflow-runs>):
  `html_url`, `head_sha` and `head_branch` (string or null), `name` (string or null) and `conclusion` (string or null) of
  a workflow run; it lists no values for `conclusion`, and `startup_failure` appears nowhere on it.
- *Verified 2026-09-30* (<https://docs.github.com/en/webhooks/webhook-events-and-payloads>): the `ping` payload has
  `zen`, `hook_id` and `hook`; the `workflow_run` payload has `workflow` and `workflow_run`. The same page names
  `check_suite` and `check_run` as objects and documents **none of their members**.
- *Unverified*: that the webhook payloads carry the members read above identically to the REST schemas, for both
  events (`check_run.output.summary`, `check_run.check_suite.head_branch`, `check_run.completed_at`,
  `check_run.pull_requests[].head.repo`, `workflow_run.head_repository` and `workflow_run.run_attempt` are not on any
  page read), that GitHub sends `startup_failure`, and the `null`s the fixtures assume. A `workflow_run` with no
  `head_repository` is ignored, so a payload that lacks it makes the route silent rather than wrong. **Slice 9 could not record real
  deliveries**: the fixtures in `orchestrator/crates/surface-webhook/testdata/github` are *synthetic*, written by hand
  after the documented schemas (their README says so). Recording a few (GitHub's "Recent Deliveries" shows the payload)
  and replacing the files is the check that settles this; the route's tests then say what differs.

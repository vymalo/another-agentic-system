# Webhooks: CI results

How a CI system reports the result of a check run on a pushed commit to the orchestrator
([ADR 0017](../decisions/0017-ci-results-by-webhook.md)). Two routes, one meaning: a signed report
that a named check finished with a conclusion, for a commit in a repository. The orchestrator
matches it to the job that pushed that commit and lets the gate decide
([ADR 0018](../decisions/0018-verification-gate-and-rework-loop.md)).

> Status (2026-09-30): the **generic route is built** (MVP slice 6; `orch-surface-webhook`, the surface name
> `webhook-generic`). The **GitHub adapter is planned** (slice 9,
> [`mvp.md`](../mvp.md#the-slices-of-steps-2-3-and-6)): this page is the contract it is built to, and the binary
> does not know the name `webhook-github` yet. Facts about GitHub are marked *verified* with a date and a source, or
> *unverified*.

## Routes

| Surface (`ORCH_SURFACES`) | Route | Sender | Signature | Body limit |
|---|---|---|---|---|
| `webhook-github` | `POST /webhooks/github` | GitHub webhooks | `X-Hub-Signature-256` over the raw body | 5 MiB |
| `webhook-generic` | `POST /webhooks/ci` | any CI | `X-Vymalo-Signature-256` over `"<ts>.<body>"` | 256 KiB |

Both are machine routes: no user identity, no cookie, and `X-Auth-Request-Email` is never read. The
edge must route `/webhooks/*` to the orchestrator without injecting an identity header. A route is
mounted only when its name is in `ORCH_SURFACES`, and its secret variable is then required
(`WEBHOOK_GITHUB_SECRETS`, `WEBHOOK_GENERIC_SECRETS`; up to two comma-separated secrets, for
rotation; a missing one is exit 78, and so is a third secret). A signature is accepted if it matches either
secret. A `worker` role serves no routes and does not need the secrets. The secrets are never logged and never in
a `Debug` or an error message.

The signature is checked on the bytes as received, **before anything is stored**. The body is a
report about a commit; it cannot start a job, and the only thing a valid report changes is a check
result.

## Generic signed shape (`POST /webhooks/ci`)

### Headers

| Header | Value | Notes |
|---|---|---|
| `Content-Type` | `application/json` | |
| `X-Vymalo-Delivery` | a UUID | The idempotency key. A repeat with the same value is stored once (`AAAA…` and `aaaa…` are the same UUID). Not a UUID: `400`, after the signature |
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
attempts are spent. Which reports are counted at all, and how, is the target's `gate.ci` policy: if
it lists required names, only those names count and all must pass; otherwise the first completed
report for the commit decides. Every report gets a card in the chat.

### Response codes

| Code | When | Stored? |
|---|---|---|
| `202 Accepted` | The signature and body are valid. Also for a repeated delivery id | Once |
| `400 Bad Request` | The signature is valid but the body is not: malformed JSON, a missing or mistyped field, a `version` other than `1`, a bad `sha`, a `conclusion` outside the list. An RFC 9457 problem | No |
| `401 Unauthorized` | A header is missing, the timestamp is not plain digits or is outside the skew window, or the signature matches no configured secret. Also a request that is not a `POST`, because the guard comes first | No |
| `413 Content Too Large` | The body is over 256 KiB | No |

A `202` means "received", not "applied": the report is matched to a job by a worker, and a report
that arrives before the job's commit is known is kept (up to `INBOX_PARKED_TTL_SECS`, default a
day) and applied when it is. The sender does not need to retry a `202`. On a `401` it should fix
the secret or the clock rather than retry.

### Worked example

Take the secret `dev-webhook-secret`, the timestamp `1790800000` and the body above, written to
`body.json` with no trailing newline (the signature covers every byte; `printf` adds none, an
editor often does):

```sh
SECRET=dev-webhook-secret
TS=1790800000
BODY='{"version":1,"repository":"https://github.com/acme/widgets","sha":"0123456789abcdef0123456789abcdef01234567","branch":"agent/fix-flaky-test","name":"ci/build","conclusion":"success","url":"https://ci.example.com/runs/42","summary":"212 tests passed"}'

# the signature: HMAC-SHA-256 over "<ts>.<body>", as lowercase hex
SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1)
echo "$SIG"
```

This prints (computed with OpenSSL on 2026-09-30):

```text
4fd60f8ffbbb110421e5f2c82030f78bcf59d4fe3e554040fc30502ccac27465
```

Post it, with the current time in place of the fixed one (the fixed timestamp is outside the skew
window of a live orchestrator, so use `TS=$(date +%s)` and recompute `SIG`):

```sh
TS=$(date +%s)
SIG=$(printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1)
curl -sS -i https://orchestrator.example.com/webhooks/ci \
  -H 'Content-Type: application/json' \
  -H "X-Vymalo-Delivery: $(uuidgen)" \
  -H "X-Vymalo-Timestamp: $TS" \
  -H "X-Vymalo-Signature-256: sha256=$SIG" \
  --data-binary "$BODY"
```

`--data-binary` sends the bytes exactly as they are signed; `-d` would strip newlines. Expect
`202`. Change one byte of the body, or send an old timestamp, and expect `401` and no stored
report. The dev script [`dev/ci-webhook.sh`](../../dev/ci-webhook.sh) does this (the generic shape now, the
GitHub shape with slice 9), and `dev/ci-e2e.sh` uses it to drive a gated job to `done`.

### Known-answer vectors

The signature is `HMAC-SHA-256(key = secret, message = "<timestamp>.<body>")`, lowercase hex. These are
checked by the unit tests of `orch-surface-webhook` (`signature.rs`) against the implementation, and were
computed with OpenSSL 3 on 2026-09-30 (`printf '%s.%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r`).

| # | Secret | Timestamp text | Body | Signature (`sha256=` + hex) |
|---|---|---|---|---|
| 1 | `dev-webhook-secret` | `1790800000` | the `$BODY` of the worked example above (compact, 250 bytes, no trailing newline) | `4fd60f8ffbbb110421e5f2c82030f78bcf59d4fe3e554040fc30502ccac27465` |
| 2 | `Jefe` | none: the message is the two parts of RFC 4231 test case 2, `what do ya want ` and `for nothing?` | | `5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843` (the algorithm's own vector) |

Skew vectors, for a clock at `T` and `WEBHOOK_GENERIC_MAX_SKEW_SECS` = 60 (the tests hold the clock):
timestamps `T-60`, `T`, `T+60` are accepted; `T-61`, `T+61` are `401`; `+T`, ` T`, `T.0`, `-1`, the empty
string and a 13-digit number are `401` even when signed exactly as written. Two secrets `new,old`: a
signature by either is accepted, by a third is `401`.

## GitHub adapter (`POST /webhooks/github`)

Configure a repository (or organisation) webhook with the payload URL
`https://<orchestrator>/webhooks/github`, content type `application/json`, the secret set to a value
in `WEBHOOK_GITHUB_SECRETS`, and the events **Check suites**, **Check runs** and **Workflow runs**.

### Request

| Header | Use |
|---|---|
| `X-Hub-Signature-256` | `sha256=` + hex `HMAC-SHA-256(secret, raw body)`. Checked first. Missing or wrong: `401`, nothing stored |
| `X-GitHub-Event` | Selects the handling below |
| `X-GitHub-Delivery` | Unique per delivery. The idempotency key is `github:<X-GitHub-Delivery>` |

For a GitHub delivery the signature is over the body alone (no timestamp). With the same secret and
body as the example above:

```sh
printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" -r | cut -d' ' -f1
# 616371fff6e4a56b699e709bc03c3906c63e443bf9afa56b55793da68b772899
```

(That body is the generic shape; it is used here only to show the computation. GitHub signs its own
payloads.)

### Events accepted

Only the events below, and only with `action` = `completed`, become a report. Every field is read
from the payload; nothing else is trusted.

| `X-GitHub-Event` | Read | Becomes |
|---|---|---|
| `check_suite` | `check_suite.head_sha`, `check_suite.head_branch`, `check_suite.conclusion`, `check_suite.app.slug`, `repository.html_url` | a report named by the app's slug |
| `check_run` | `check_run.head_sha`, `check_run.check_suite.head_branch`, `check_run.conclusion`, `check_run.name`, `check_run.html_url`, `check_run.output.summary`, `repository.html_url` | a report named by the check's name |
| `workflow_run` | `workflow_run.head_sha`, `workflow_run.head_branch`, `workflow_run.conclusion`, `workflow_run.name`, `workflow_run.html_url`, `repository.html_url` | a report named by the workflow's name |

All three normalise to the same report as the generic body: `repository` from
`repository.html_url`, `sha` from `head_sha`, `branch` from `head_branch`, then `name`,
`conclusion`, `url`, `summary`. Conclusions use the closed list above; a value outside it (or a
missing one) counts as `failure`, so an unknown outcome never passes. Which field feeds `summary`
and `url` for each event is fixed by the recorded fixtures of slice 9; where the payload has none,
the member is absent.

### Response codes

| Code | When | Stored? |
|---|---|---|
| `202 Accepted` | A `check_suite`, `check_run` or `workflow_run` with `action` = `completed`, validly signed. Also a repeated `X-GitHub-Delivery`, and any other event or action (those are acknowledged so GitHub does not retry or flag them) | Only the first kind, once |
| `204 No Content` | `ping` (sent when the webhook is created) | No |
| `401 Unauthorized` | `X-Hub-Signature-256` missing, or matching no configured secret | No |
| `413 Content Too Large` | The body is over 5 MiB | No |
| `400 Bad Request` | Validly signed, but not JSON, or an accepted event without the fields above. An RFC 9457 problem | No |

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
- *Unverified*: that the webhook payloads carry these fields identically to the REST schemas, and
  the exact members for `url` and `summary` per event. Slice 9 records real deliveries as fixtures
  and this page is corrected to match them.

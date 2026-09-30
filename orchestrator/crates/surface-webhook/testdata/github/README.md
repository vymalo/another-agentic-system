# GitHub webhook fixtures

**Synthetic, not recorded.** These payloads were written on 2026-09-30 by hand, shaped after GitHub's documented
schemas (the REST objects for check suites, check runs and workflow runs, which webhook payloads embed; the
`ping` event's `zen`, `hook_id` and `hook`). No real GitHub delivery was available when they were written, so nothing
here proves that a live payload has exactly these members: that is *unverified*
([ADR 0017](../../../../../docs/decisions/0017-ci-results-by-webhook.md#verified), [`docs/api/webhooks.md`](../../../../../docs/api/webhooks.md#verified-and-unverified-2026-09-30)).
They carry many members the route does not read on purpose, and some that are `null` where GitHub sends `null`.
When a real delivery is recorded (GitHub's "Recent Deliveries" shows the payload), replace the matching file with it,
keep the name, and delete this paragraph for that file.

The file name is `<X-GitHub-Event>.<action>[.<variant>].json`; `tests/github.rs` states what each one must do.
The times in them (`completed_at`, `updated_at`) are those of 2026-09-30, so a test that runs them against the real
clock (the binary's smoke test) dates them again; the crate's own tests hold a clock at 2026-09-30T12:00:00Z.
`check_suite.completed.success` is kept only to show that a suite is acknowledged and not stored; the
`fork` fixtures (`workflow_run.completed.fork`, `check_run.completed.fork_pull_request`) are runs of code from another repository, and
`workflow_run.completed.unnamed` has a `null` name: none of the three is stored.
`Acme/Widgets` is capitalised on purpose: the repository key is lower case.

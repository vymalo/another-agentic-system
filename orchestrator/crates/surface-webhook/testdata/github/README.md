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
`Acme/Widgets` is capitalised on purpose: the repository key is lower case.

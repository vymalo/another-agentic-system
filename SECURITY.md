# Security policy

## Report a vulnerability privately

Use GitHub's private vulnerability reporting: open the repository's **Security** tab and choose
**Report a vulnerability**, or go to
<https://github.com/vymalo/another-agentic-system/security/advisories/new>.

**Do not open a public issue, a pull request or a discussion for a vulnerability.** A public report gives everyone the
details before there is a fix.

## What to include

- What is affected: the component (orchestrator, web, chart, edge, a dev script), and the commit or image tag.
- What an attacker can do, and what they need first (an account, a role, network access).
- Steps to reproduce, or a proof of concept. The easiest is the local stack, which runs offline with dummy
  credentials: [Test it locally](dev/README.md#test-it-locally).
- Anything you already know about a fix.

Please reproduce against your own local stack. Do not test against a deployment you do not own, and do not read or
change other people's data.

## Scope

In scope: the code in this repository and the chart in `deploy/`. A weakness in a third-party component we use is best
reported to that project; tell us too if our configuration makes it worse.

## Known limitations need no report

Some limits are already written down. Reporting them privately adds nothing; a fix or a better idea is welcome as an
ordinary issue or pull request.

- The coder's known limits: [adam ADR 0041, "Known limits, not built"](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0041-a-run-pod-sees-only-its-own-run.md#known-limits-not-built).
- The open questions of this system, such as rate limiting at the edge: [`docs/open-questions.md`](docs/open-questions.md).
- The hardening work on the [roadmap](ROADMAP.md): the tickets labelled `security`.
- The MVP is proven on mocks only ([`docs/vision.md`](docs/vision.md)). Nothing here has been audited.

## Supported versions

Fixes land on `main`. No older branch is maintained.

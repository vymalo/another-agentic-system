# ADR 0003 — git is the durable artifact; workers are ephemeral

- **Status:** accepted (2026-09-28)

## Context

"All stateless, only chats are stateful" — but coding work has state: the
workspace. Agent Canvas lost uncommitted work when a pod restarted because its
worktrees lived in `/tmp` ([lessons](../lessons-from-agent-canvas.md) #4).

## Decision

- Durable state lives in exactly three places: Postgres (chat, job ledger,
  events), **git** (every worker pushes a branch; the result is a PR), and
  persistent caches (sccache/CI cache, npm, cargo, uv).
- Workers are ephemeral sandbox pods: clone, work, push, die. A step is not
  complete until its branch is pushed.
- Worker images bake toolchains under `/opt` (recipe from
  vymalo/openhand-images).

## Consequences

- A worker crash loses at most the unpushed part of one step; the step is
  retried from the last pushed commit.
- Cold caches cost time on first use; shared persistent caches amortise it.

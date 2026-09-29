# web — chat surface

Next.js (App Router, TypeScript strict) with [assistant-ui](https://www.assistant-ui.com/)'s
external store runtime. It renders a thread's event log from the orchestrator and lets the owner
pick an agent (and a release, when the agent offers one), start threads, follow up, cancel and
watch progress live. Decision: [ADR 0006](../docs/decisions/0006-assistant-ui-external-store.md);
release picker: [ADR 0008](../docs/decisions/0008-platform-integration-via-a2a-extension.md).

The UI renders events. It never invents state: a sent message appears when the server returns it,
and "running" is the server's thread state.

## Contract

The only interface is [`docs/api/chat-api.yaml`](../docs/api/chat-api.yaml). Types are generated
from it at build time (`pnpm gen:api`, openapi-typescript + openapi-fetch); `src/api/schema.d.ts`
is never committed. `src/api/contract.typecheck.ts` holds deliberate mismatches
(`@ts-expect-error`) that must stay type errors, so a contract change that the client does not
follow fails `pnpm typecheck`.

The browser calls `/api/*` on its own origin only. In production oauth2-proxy / the ingress routes
`/api/*` to the orchestrator; there are no Next.js API routes, server-side fetches or secrets.
`MOCK_API_ORIGIN` (dev and e2e only) adds a rewrite to the mock server.

## Scripts

```sh
pnpm install
pnpm dev:mock      # mock contract server on :4010 + the app on :3000
pnpm dev           # the app only; put something that serves /api/* in front of it
pnpm check         # Biome (lint + format), CI mode
pnpm typecheck     # generated types + tsc
pnpm test          # vitest: reducer, mapping, event stream hook, mock-vs-contract
pnpm build         # production build (standalone)
pnpm test:e2e      # Playwright + axe + Lighthouse (>= 95 accessibility) against the mock
```

Playwright uses the browser Playwright pins (`@playwright/test` is pinned exactly; CI runs
`playwright install --with-deps chromium`).

## Mock server

`mock/server.ts` is a small stateful server (not Prism: it needs the create, stream, follow-up and
cancel flow, including `Last-Event-ID` replay). `mock/server.contract.test.ts` validates every
response and SSE frame against the schemas in the contract. The first message picks the script:

| The message contains | Behaviour |
|---|---|
| anything else | working, streamed reply, artifact (PR link), done |
| `question` | asks "Which branch should I use?", blocks; the follow-up resumes to done |
| `slow` | works until cancelled |
| `fail` | error event, failed |

Agents: `coder` (has `releases`) and `reviewer` (none).

## Image

`Dockerfile` (Node 24, non-root, standalone output, `EXPOSE 3000`); the build context is the
repository root:

```sh
docker build -f web/Dockerfile -t web .
```

CI builds it on every change and pushes `ghcr.io/vymalo/another-agentic-system/web:sha-<7>` and
`:latest` from `main` (`.github/workflows/web.yml`).

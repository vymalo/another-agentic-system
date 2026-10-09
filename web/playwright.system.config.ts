import path from "node:path";
import { defineConfig, devices } from "@playwright/test";
import { ORCH, ORCH_SCRIPT, orchestratorEnv, WEB } from "./e2e-system/env";

/**
 * The chat surface against the REAL orchestrator: the `orchestrator` binary on Postgres, the
 * scripted fake agents of `orch-fake-agent`, and the production build of the app whose
 * `/api/*` rewrite points at the orchestrator. Nothing here is a mock of the contract.
 *
 * Identity plays oauth2-proxy: every request of the browser context carries
 * `X-Auth-Request-Email`, and `AUTH_DEV_USER` is unset on the orchestrator.
 *
 * Needs: `ORCH_BIN`, `FAKE_AGENT_BIN` (cargo-built), `DATABASE_URL` (an empty, dedicated
 * database: the tests truncate it). See web/README.md.
 */
const CI = Boolean(process.env.CI);
const FAKE_AGENT_BIN = process.env.FAKE_AGENT_BIN ?? "../orchestrator/target/debug/orch-fake-agent";

export default defineConfig({
  testDir: "e2e-system",
  workers: 1,
  fullyParallel: false,
  retries: 0,
  forbidOnly: CI,
  timeout: 60_000,
  expect: { timeout: 15_000 },
  reporter: CI ? [["list"], ["html", { open: "never" }]] : "list",
  globalTeardown: path.resolve("e2e-system/teardown.ts"),
  use: {
    baseURL: WEB,
    trace: "retain-on-failure",
    extraHTTPHeaders: { "X-Auth-Request-Email": "alice@example.com" },
  },
  webServer: [
    {
      command: FAKE_AGENT_BIN,
      // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
      url: "http://127.0.0.1:4021/.well-known/agent-card.json",
      // both agents list ui-catalog/v1 (ADR 0023) and take a catalog inline: choices.spec.ts; and usage/v1
      // (ADR 0056), whose `usage` script reports its calls and keeps its totals on the task: usage.spec.ts
      env: { FAKE_AGENT_EXTENSIONS: "ui-catalog,usage" },
      reuseExistingServer: false,
    },
    {
      command: `sh ${ORCH_SCRIPT}`,
      // /healthz answers without an identity; /readyz also needs the database
      url: `${ORCH}/readyz`,
      env: orchestratorEnv(),
      reuseExistingServer: false,
    },
    {
      // A production build whose rewrites (fixed at build time) send /api/* to the orchestrator.
      command: "pnpm build:system && pnpm exec next start -p 3100",
      url: WEB,
      timeout: 240_000,
      reuseExistingServer: false,
    },
  ],
  projects: [{ name: "chromium", use: { ...devices["Desktop Chrome"] } }],
});

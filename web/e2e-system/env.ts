import path from "node:path";

/**
 * What the system e2e tests share: where things listen, and the orchestrator's environment.
 * `playwright.system.config.ts` starts the orchestrator with it, and `restart.spec.ts` starts
 * it again after killing it.
 */
// nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
export const ORCH = "http://127.0.0.1:8080";
// nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
export const WEB = "http://127.0.0.1:3100";
// nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
export const FAKE_CONTROL = "http://127.0.0.1:4020/__control";

/** The origin of the forwarder `reconnect.spec.ts` puts in front of the app. */
// nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
export const proxyOrigin = (port: number) => `http://127.0.0.1:${port}`;

export const RUN_DIR = path.resolve(import.meta.dirname, ".run");
export const ORCH_SCRIPT = path.resolve(import.meta.dirname, "orch.sh");

export const DATABASE_URL =
  process.env.DATABASE_URL ?? "postgres://postgres:postgres@localhost:5432/orch_system";

/** `AUTH_DEV_USER` stays unset: the identity comes from the request header, like production. */
export function orchestratorEnv(): Record<string, string> {
  return {
    DATABASE_URL,
    LISTEN_ADDR: "127.0.0.1:8080",
    AGENTS_FILE: path.resolve(import.meta.dirname, "agents.yaml"),
    // a killed replica's delegations are taken over after this long (restart.spec.ts)
    DATABASE_MAX_CONNECTIONS: "4",
    OUTBOX_LEASE_SECS: "5",
    LOG_FORMAT: "text",
    // the agents are on loopback: never through the sandbox/CI proxy
    NO_PROXY: "127.0.0.1,localhost",
    no_proxy: "127.0.0.1,localhost",
    ORCH_BIN: process.env.ORCH_BIN ?? "../orchestrator/target/debug/orchestrator",
  };
}

import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { type APIRequestContext, expect, type Page } from "@playwright/test";
import pg from "pg";
import { badge, errorLine, startThread, THREAD_URL } from "../e2e/helpers";
import { DATABASE_URL, FAKE_CONTROL, ORCH, ORCH_SCRIPT, orchestratorEnv, RUN_DIR } from "./env";

export { badge, errorLine, startThread, THREAD_URL };

export const ALICE = "alice@example.com";
export const BOB = "bob@example.com";
export const PR_URL = "https://github.com/acme/demo/pull/1";

// One connection per worker, kept open: opening one per call needlessly competes for slots on a
// shared Postgres (which then answers "too many clients"; SQLSTATE 53300).
const db = new pg.Pool({ connectionString: DATABASE_URL, max: 1 });

async function query<R extends pg.QueryResultRow>(sql: string): Promise<pg.QueryResult<R>> {
  for (let attempt = 1; ; attempt++) {
    try {
      return await db.query<R>(sql);
    } catch (e) {
      const tooMany = (e as { code?: string }).code === "53300";
      if (!tooMany || attempt >= 10) throw e;
      await new Promise((r) => setTimeout(r, 500));
    }
  }
}

/** How many threads exist, whoever owns them. */
export async function threadCount(): Promise<number> {
  const res = await query<{ n: number }>("SELECT count(*)::int AS n FROM threads");
  return res.rows[0]?.n ?? 0;
}

/** An empty database for the test: threads, events, bindings and the outbox go together. */
export async function resetDb() {
  await query("TRUNCATE threads CASCADE");
}

export type ApiEvent = {
  seq: number;
  kind: string;
  actor: { type: string; name: string; revision?: string };
  data: Record<string, unknown>;
};

/** `kind` (with the status or state, for the kinds that have one) of each event. */
export const shape = (events: ApiEvent[]): string[] =>
  events.map((e) =>
    e.kind === "agent_status"
      ? `${e.kind}:${e.data.status}`
      : e.kind === "thread_state"
        ? `${e.kind}:${e.data.state}`
        : e.kind,
  );

export const FIVE = [
  "user_message",
  "agent_status:working",
  "artifact",
  "agent_status:completed",
  "thread_state:done",
];

export const threadId = (page: Page): string => {
  const id = new URL(page.url()).pathname.split("/").pop();
  if (!id) throw new Error(`no thread id in ${page.url()}`);
  return id;
};

/** `GET /api/threads/{id}/events` through the app's rewrite, as the page's user. */
export async function eventsOf(request: APIRequestContext, id: string): Promise<ApiEvent[]> {
  const res = await request.get(`/api/threads/${id}/events?limit=500`);
  expect(res.status()).toBe(200);
  return (await res.json()) as ApiEvent[];
}

export type AgentCall = {
  kind: "execute" | "cancel";
  taskId: string;
  contextId: string;
  messageId: string | null;
  text: string;
  resuming: boolean;
  extensionsHeader: string[];
  activatesReleaseChannels: boolean;
  release: string | null;
};

/** What the fake agent's executor saw for messages whose text starts with `text`. */
export async function callsFor(
  request: APIRequestContext,
  agent: "coder" | "plain",
  text: string,
): Promise<AgentCall[]> {
  const res = await request.get(`${FAKE_CONTROL}/${agent}/calls`);
  expect(res.status()).toBe(200);
  return ((await res.json()) as AgentCall[]).filter((c) => c.text.startsWith(text));
}

/** Every call the agent's executor saw (cancels carry no text). */
export async function allCalls(
  request: APIRequestContext,
  agent: "coder" | "plain",
): Promise<AgentCall[]> {
  const res = await request.get(`${FAKE_CONTROL}/${agent}/calls`);
  expect(res.status()).toBe(200);
  return (await res.json()) as AgentCall[];
}

/** Lets the one waiting `gate` task of the agent continue. */
export async function releaseGate(request: APIRequestContext, agent: "coder" | "plain") {
  const res = await request.post(`${FAKE_CONTROL}/${agent}/release-gate`);
  expect(res.status()).toBe(204);
}

/** Waits until the agent's executor is running `text` (its `gate` script then waits). */
export async function waitForExecution(
  request: APIRequestContext,
  agent: "coder" | "plain",
  text: string,
) {
  await expect
    .poll(
      async () => (await callsFor(request, agent, text)).filter((c) => c.kind === "execute").length,
    )
    .toBeGreaterThan(0);
}

// ---- restart: the orchestrator process ------------------------------------------------

const pidFile = path.join(RUN_DIR, "orchestrator.pid");

export function orchestratorPid(): number {
  return Number(readFileSync(pidFile, "utf8").trim());
}

const alive = (pid: number): boolean => {
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
};

/** SIGKILL: a crash, no graceful shutdown, no lease release. */
export async function killOrchestrator() {
  const pid = orchestratorPid();
  process.kill(pid, "SIGKILL");
  await expect.poll(() => alive(pid)).toBe(false);
}

/** Starts a new orchestrator like the one Playwright started, and waits until it is ready. */
export async function startOrchestrator() {
  const child = spawn("sh", [ORCH_SCRIPT], {
    env: { ...process.env, ...orchestratorEnv(), ORCH_LOG_APPEND: "1" },
    detached: true,
    stdio: "ignore",
  });
  child.unref();
  await expect
    .poll(
      async () => {
        try {
          return (await fetch(`${ORCH}/readyz`)).status;
        } catch {
          return 0;
        }
      },
      { timeout: 30_000 },
    )
    .toBe(200);
}

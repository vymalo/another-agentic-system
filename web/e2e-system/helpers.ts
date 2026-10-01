import { spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { type APIRequestContext, expect, type Page } from "@playwright/test";
import pg from "pg";
import {
  actorLabel,
  agentMessage,
  badge,
  conversation,
  errorLine,
  exportMenuItem,
  openThreadList,
  revisionOptions,
  selectedOption,
  startThread,
  THREAD_URL,
  threadList,
  threadRows,
} from "../e2e/helpers";
import { uuidv7 } from "../src/lib/uuid";
import { DATABASE_URL, FAKE_CONTROL, ORCH, ORCH_SCRIPT, orchestratorEnv, RUN_DIR } from "./env";

export {
  actorLabel,
  agentMessage,
  badge,
  conversation,
  errorLine,
  exportMenuItem,
  openThreadList,
  revisionOptions,
  selectedOption,
  startThread,
  THREAD_URL,
  threadList,
  threadRows,
};

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

export type Frame = { id?: number; event: Record<string, unknown> & { type: string } };

/** One frame per entry: the event type, and what tells apart the ones that repeat. */
const label = (f: Frame): string => {
  const e = f.event;
  switch (e.type) {
    case "ACTIVITY_SNAPSHOT": {
      const content = e.content as { status?: string };
      return `${e.type}:${String(e.activityType)}${content.status ? `:${content.status}` : ""}`;
    }
    case "RUN_FINISHED":
      return `${e.type}:${(e.outcome as { type: string } | undefined)?.type ?? "success"}`;
    case "RUN_ERROR":
      return `${e.type}:${String(e.code)}`;
    case "STATE_SNAPSHOT":
      return `${e.type}:${String((e.snapshot as { thread: { state: string } }).thread.state)}`;
    default:
      return e.type;
  }
};

/** The frames a thread's connect stream replays (`?mode=run`), as the web's ThreadAgent reads them. */
export async function framesOf(request: APIRequestContext, id: string): Promise<Frame[]> {
  const res = await request.get(`/agui/threads/${id}/connect?mode=run`);
  expect(res.status()).toBe(200);
  const out: Frame[] = [];
  for (const block of (await res.text()).split("\n\n")) {
    const lines = block.split("\n").filter((l) => !l.startsWith(":"));
    const data = lines.filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trim());
    if (data.length === 0) continue;
    const idLine = lines.find((l) => l.startsWith("id:"));
    out.push({
      ...(idLine ? { id: Number(idLine.slice(3).trim()) } : {}),
      event: JSON.parse(data.join("\n")) as Frame["event"],
    });
  }
  return out;
}

/** The type (and outcome, activity, state) of each frame. */
export const shape = (frames: Frame[]): string[] => frames.map(label);

/** The log `seq` of each resume point: the events of the log, one per id. */
export const seqs = (frames: Frame[]): number[] =>
  frames.flatMap((f) => (f.id === undefined ? [] : [f.id]));

/** A thread the way any AG-UI client makes one: the consumer mints the id, the POST runs it. */
export async function createThread(
  request: APIRequestContext,
  agent: "coder" | "plain",
  text: string,
): Promise<string> {
  const id = uuidv7();
  const res = await request.post(`/agui/agents/${agent}`, {
    headers: { Accept: "text/event-stream" },
    data: {
      threadId: id,
      runId: crypto.randomUUID(),
      messages: [{ id: crypto.randomUUID(), role: "user", content: text }],
    },
  });
  expect(res.status()).toBe(200);
  await res.text(); // the response ends with the run
  return id;
}

/** The echo script as a viewer replays it (docs/api/examples/agui/connect-echo.agui.json). */
export const ECHO = [
  "RUN_STARTED",
  "STATE_SNAPSHOT:queued",
  "TEXT_MESSAGE_START",
  "TEXT_MESSAGE_CONTENT",
  "TEXT_MESSAGE_END",
  "SUBAGENT_STARTED",
  "ACTIVITY_SNAPSHOT:vymalo.status:working",
  "STATE_SNAPSHOT:working",
  "ACTIVITY_SNAPSHOT:vymalo.artifact",
  "ACTIVITY_SNAPSHOT:vymalo.status:completed",
  "SUBAGENT_FINISHED",
  "STATE_SNAPSHOT:done",
  "RUN_FINISHED:success",
];

export const threadId = (page: Page): string => {
  const id = new URL(page.url()).pathname.split("/").pop();
  if (!id) throw new Error(`no thread id in ${page.url()}`);
  return id;
};

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
  /** `metadata[ui-catalog/v1]` of the message (ADR 0023); null when the message carried none. */
  uiCatalog: { catalogId: string; version: number; digest: string; inline: boolean } | null;
  /** The catalogs the message carried inline (A2UI's `inlineCatalogs`). */
  inlineCatalogs: { catalogId: string; components: Record<string, unknown> }[];
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

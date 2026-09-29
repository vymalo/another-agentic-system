/**
 * A small stateful mock of docs/api/chat-api.yaml for `pnpm dev:mock` and the e2e tests.
 *
 * It is typed from the generated contract types and checked against the contract's schemas by
 * server.contract.test.ts. Authentication is not enforced (oauth2-proxy's job in production).
 * The first word of the first message picks a scripted agent behaviour: see scripts.ts and
 * web/README.md. The scripts follow what the real orchestrator emits (docs/api/examples).
 */
import { randomUUID } from "node:crypto";
import http from "node:http";
import { pathToFileURL } from "node:url";
import type { components } from "../src/api/schema";
import { AGENTS, DEV_USER } from "./fixtures";
import { cancelSteps, type Step, scriptFor } from "./scripts";

type Thread = components["schemas"]["Thread"];
type Event = components["schemas"]["Event"];
type Actor = components["schemas"]["Actor"];
type ThreadState = components["schemas"]["ThreadState"];

type Run = {
  timer: NodeJS.Timeout | undefined;
  pending: Step[];
  resume: ((answer: string) => Step[]) | undefined;
};

export type MockOptions = { stepMs?: number; keepaliveMs?: number };

export function createMockServer(options: MockOptions = {}): http.Server {
  const stepMs = options.stepMs ?? 400;
  const keepaliveMs = options.keepaliveMs ?? 15_000;

  const threads = new Map<string, Thread>();
  const events = new Map<string, Event[]>();
  const subscribers = new Map<string, Set<http.ServerResponse>>();
  const runs = new Map<string, Run>();

  const reset = () => {
    for (const r of runs.values()) clearTimeout(r.timer);
    for (const set of subscribers.values()) for (const res of set) res.end();
    threads.clear();
    events.clear();
    subscribers.clear();
    runs.clear();
  };

  // ---- helpers -------------------------------------------------------------------------

  const sendJson = (
    res: http.ServerResponse,
    status: number,
    body: unknown,
    type = "application/json",
  ) => {
    res.writeHead(status, { "Content-Type": type, "Cache-Control": "no-store" });
    res.end(JSON.stringify(body));
  };
  const problem = (res: http.ServerResponse, status: number, title: string, detail?: string) =>
    sendJson(
      res,
      status,
      { title, status, ...(detail ? { detail } : {}) },
      "application/problem+json",
    );

  const readJson = (req: http.IncomingMessage): Promise<unknown> =>
    new Promise((resolve, reject) => {
      const chunks: Buffer[] = [];
      req.on("data", (c: Buffer) => chunks.push(c));
      req.on("end", () => {
        try {
          resolve(JSON.parse(Buffer.concat(chunks).toString("utf8") || "null"));
        } catch (e) {
          reject(e);
        }
      });
      req.on("error", reject);
    });

  const touch = (t: Thread) => {
    t.updatedAt = new Date().toISOString();
  };

  function append(threadId: string, kind: Event["kind"], actor: Actor, data: Event["data"]): Event {
    const log = events.get(threadId) ?? [];
    events.set(threadId, log);
    const event: Event = {
      seq: log.length + 1,
      threadId,
      at: new Date().toISOString(),
      kind,
      actor,
      data,
    };
    log.push(event);
    const t = threads.get(threadId);
    if (t) {
      t.lastSeq = event.seq;
      touch(t);
    }
    const frame = `id: ${event.seq}\nevent: ${event.kind}\ndata: ${JSON.stringify(event)}\n\n`;
    for (const res of subscribers.get(threadId) ?? []) res.write(frame);
    return event;
  }

  const setState = (t: Thread, state: ThreadState) => {
    t.state = state;
    touch(t);
  };

  function agentActor(t: Thread): Actor {
    const agent = AGENTS.find((a) => a.id === t.target.agentId);
    const releases = agent?.releases;
    let revision: string | undefined;
    if (releases) {
      const release = t.target.release ?? releases.defaultChannel;
      revision = releases.channels[release] ?? release;
    }
    return { type: "agent", name: t.target.agentId, ...(revision ? { revision } : {}) };
  }

  // ---- scripted agent ------------------------------------------------------------------

  function play(t: Thread, steps: Step[]) {
    const run = runs.get(t.id) ?? { timer: undefined, pending: [], resume: undefined };
    runs.set(t.id, run);
    clearTimeout(run.timer);
    run.pending = [...steps];
    const tick = () => {
      const step = run.pending.shift();
      if (!step) return;
      if ("pause" in step) return; // waits for cancel
      append(
        t.id,
        step.kind,
        step.system ? { type: "system", name: "orchestrator" } : agentActor(t),
        step.data,
      );
      if (step.setState) setState(t, step.setState);
      run.timer = setTimeout(tick, stepMs);
    };
    run.timer = setTimeout(tick, stepMs);
  }

  // ---- routes --------------------------------------------------------------------------

  const server = http.createServer((req, res) => {
    handle(req, res).catch((e: unknown) => {
      if (!res.headersSent) problem(res, 500, "Internal error", String(e));
      else res.end();
    });
  });

  async function handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const url = new URL(req.url ?? "/", "http://mock");
    const method = req.method ?? "GET";
    const path = url.pathname;

    if (path === "/healthz" || path === "/readyz") return void res.writeHead(200).end("ok");
    if (path === "/__mock/reset" && method === "POST") {
      reset();
      return void res.writeHead(204).end();
    }
    if (path === "/api/agents" && method === "GET") return sendJson(res, 200, AGENTS);

    if (path === "/api/threads") {
      if (method === "GET") return listThreads(res, url);
      if (method === "POST") return createThread(req, res);
    }

    const m = /^\/api\/threads\/([^/]+)(?:\/(events|stream|messages|cancel))?$/.exec(path);
    if (m) {
      const id = decodeURIComponent(m[1] ?? "");
      const sub = m[2];
      const thread = threads.get(id);
      if (!thread) return problem(res, 404, "Thread not found");
      if (!sub && method === "GET") return sendJson(res, 200, thread);
      if (sub === "events" && method === "GET") return listEvents(res, url, thread);
      if (sub === "stream" && method === "GET") return stream(req, res, thread);
      if (sub === "messages" && method === "POST") return postMessage(req, res, thread);
      if (sub === "cancel" && method === "POST") return cancel(res, thread);
    }
    return problem(res, 404, "Not found");
  }

  function listThreads(res: http.ServerResponse, url: URL) {
    const limit = Math.min(100, Math.max(1, Number(url.searchParams.get("limit") ?? 50) || 50));
    const before = url.searchParams.get("before");
    let all = [...threads.values()].reverse(); // newest first
    if (before) {
      const i = all.findIndex((t) => t.id === before);
      all = i >= 0 ? all.slice(i + 1) : [];
    }
    sendJson(res, 200, all.slice(0, limit));
  }

  async function createThread(req: http.IncomingMessage, res: http.ServerResponse) {
    let body: unknown;
    try {
      body = await readJson(req);
    } catch {
      return problem(res, 400, "Invalid JSON");
    }
    const b = body as Partial<components["schemas"]["NewThread"]> | null;
    const text = b?.text;
    if (typeof text !== "string" || text.length < 1 || text.length > 100_000) {
      return problem(res, 400, "Invalid request", "text must be 1 to 100000 characters");
    }
    const target = b?.target;
    if (typeof target?.agentId !== "string") {
      return problem(res, 400, "Invalid request", "target.agentId is required");
    }
    const agent = AGENTS.find((a) => a.id === target.agentId);
    if (!agent) return problem(res, 400, "Unknown agent", `No agent "${target.agentId}"`);
    const release = target.release;
    if (release !== undefined) {
      if (!agent.releases) {
        return problem(res, 400, "Invalid request", `${agent.id} does not offer releases`);
      }
      const known =
        release in agent.releases.channels || (agent.releases.revisions ?? []).includes(release);
      if (!known) return problem(res, 400, "Unknown release", `No release "${release}"`);
    }
    const now = new Date().toISOString();
    const thread: Thread = {
      id: randomUUID(),
      title: (b?.title ?? text).slice(0, 60),
      target: { agentId: agent.id, ...(release !== undefined ? { release } : {}) },
      state: "queued",
      createdAt: now,
      updatedAt: now,
      lastSeq: 0,
    };
    threads.set(thread.id, thread);
    events.set(thread.id, []);
    append(thread.id, "user_message", { type: "user", name: DEV_USER }, { text });
    const script = scriptFor(text);
    runs.set(thread.id, { timer: undefined, pending: [], resume: script.resume });
    play(thread, script.start);
    sendJson(res, 201, thread);
  }

  function listEvents(res: http.ServerResponse, url: URL, thread: Thread) {
    const after = Number(url.searchParams.get("after") ?? 0) || 0;
    const limit = Math.min(500, Math.max(1, Number(url.searchParams.get("limit") ?? 200) || 200));
    sendJson(res, 200, (events.get(thread.id) ?? []).filter((e) => e.seq > after).slice(0, limit));
  }

  function stream(req: http.IncomingMessage, res: http.ServerResponse, thread: Thread) {
    res.writeHead(200, {
      "Content-Type": "text/event-stream",
      "Cache-Control": "no-cache, no-transform",
      Connection: "keep-alive",
      "X-Accel-Buffering": "no",
    });
    res.write("retry: 1000\n\n");
    const last = Number(req.headers["last-event-id"] ?? 0) || 0;
    for (const e of events.get(thread.id) ?? []) {
      if (e.seq > last) res.write(`id: ${e.seq}\nevent: ${e.kind}\ndata: ${JSON.stringify(e)}\n\n`);
    }
    const set = subscribers.get(thread.id) ?? new Set();
    subscribers.set(thread.id, set);
    set.add(res);
    const keepalive = setInterval(() => res.write(": keepalive\n\n"), keepaliveMs);
    keepalive.unref();
    res.on("close", () => {
      clearInterval(keepalive);
      set.delete(res);
    });
  }

  async function postMessage(req: http.IncomingMessage, res: http.ServerResponse, thread: Thread) {
    let body: unknown;
    try {
      body = await readJson(req);
    } catch {
      return problem(res, 400, "Invalid JSON");
    }
    const text = (body as { text?: unknown } | null)?.text;
    if (typeof text !== "string" || text.length < 1 || text.length > 100_000) {
      return problem(res, 400, "Invalid request", "text must be 1 to 100000 characters");
    }
    if (thread.state === "done" || thread.state === "failed" || thread.state === "cancelled") {
      return problem(res, 409, "Thread is finished", "Start a new thread to continue.");
    }
    const event = append(thread.id, "user_message", { type: "user", name: DEV_USER }, { text });
    if (thread.state === "blocked") {
      setState(thread, "queued");
      const resume = runs.get(thread.id)?.resume;
      if (resume) play(thread, resume(text));
    }
    sendJson(res, 202, event);
  }

  function cancel(res: http.ServerResponse, thread: Thread) {
    const active =
      thread.state === "queued" || thread.state === "working" || thread.state === "blocked";
    if (active) {
      const run = runs.get(thread.id);
      if (run) clearTimeout(run.timer);
      play(thread, cancelSteps);
    }
    res.writeHead(202).end();
  }

  server.on("close", reset);
  return server;
}

async function main() {
  const port = Number(process.env.MOCK_PORT ?? 4010);
  const stepMs = process.env.MOCK_STEP_MS ? Number(process.env.MOCK_STEP_MS) : undefined;
  const server = createMockServer(stepMs === undefined ? {} : { stepMs });
  server.listen(port, "127.0.0.1", () => {
    console.log(`mock chat API on http://127.0.0.1:${port}`);
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  void main();
}

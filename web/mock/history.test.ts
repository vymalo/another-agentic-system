import { readFileSync } from "node:fs";
import type { AddressInfo } from "node:net";
import path from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import type { components } from "../src/lib/api/schema";
import { DEFAULT_LIMITS, type HistoryLimits, type Page, readPage, type Window } from "./history";
import { Projector } from "./projection";
import { createMockServer } from "./server";

/**
 * The mock's fold of a thread's history (docs/api/history.md) against the real one: the page boundaries that
 * `orch-agui-projection` writes for every golden log (docs/api/examples/history), and the frames of the pages against the
 * golden stream (docs/api/examples/agui), which the pages must tile. A fold that drifts from the orchestrator's is found here.
 */

type Event = components["schemas"]["Event"];
const EXAMPLES = path.resolve(import.meta.dirname, "../../docs/api/examples");

/** The golden logs the mock's projection reads as they are (golden.test.ts drives the others through its server). */
const READ_AS_THEY_ARE = [
  "a2ui",
  "ask-agent",
  "ask",
  "cancel",
  "catalog",
  "description",
  "echo",
  "fail",
  "file",
  "followup-after-cancel",
  "followup",
  "fork-blocked",
  "fork",
  "mentions",
  "steer",
  "steps-ask",
  "steps",
  "stop-and-send",
  "talk",
  "title",
  "tools-attach",
  "tools-relay",
  "turn-output",
  "usage",
  "working",
];

const logOf = (name: string): Event[] =>
  (JSON.parse(readFileSync(path.join(EXAMPLES, `${name}.events.json`), "utf8")) as Event[]).map(
    (e) => ({
      ...e,
      at: new Date((1_800_000_000 + e.seq) * 1000).toISOString(),
      data: e.kind === "agent_message" ? { ...e.data, messageId: `msg-${e.seq}` } : e.data,
    }),
  );

const infoOf = (events: Event[]) => ({
  threadId: "<thread-id>",
  title: String(events.find((e) => e.kind === "user_message")?.data.text).split("\n")[0] ?? "",
  target: { agentId: "plain" },
});

const head = (events: Event[]) => events.at(-1)?.seq ?? 0;

/** Every page back from the newest, `turns` at a time. */
function walk(events: Event[], turns: number, limits: HistoryLimits): Page[] {
  const pages: Page[] = [];
  let before: number | undefined;
  for (;;) {
    const window: Window = { kind: "turns", turns, ...(before !== undefined ? { before } : {}) };
    const page = readPage(infoOf(events), events, window, limits, head(events));
    pages.push(page);
    if (!page.earlier) return pages;
    before = page.start;
    if (pages.length > events.length + 2) throw new Error("a walk that never ends");
  }
}

const boundaries = (pages: Page[]) =>
  pages.map(({ start, end, earlier }) => ({ earlier, end, start }));

/** The comparison ignores the clock: an activity's `at` and a message's `vymalo.at` (golden.test.ts says why). */
const untimed = (x: unknown): unknown =>
  JSON.parse(
    JSON.stringify(x, (k, v) =>
      k === "at" || k === "startedAt" || k === "vymalo.at" ? "<timestamp>" : v,
    ),
  );

describe("the mock's fold against the orchestrator's", () => {
  for (const name of READ_AS_THEY_ARE) {
    it(`finds the boundaries the orchestrator wrote and tiles the golden stream: ${name}`, () => {
      const events = logOf(name);
      const pinned = JSON.parse(
        readFileSync(path.join(EXAMPLES, "history", `${name}.pages.json`), "utf8"),
      ) as Record<string, ReturnType<typeof boundaries>>;
      const golden = JSON.parse(
        readFileSync(path.join(EXAMPLES, "agui", `${name}.agui.json`), "utf8"),
      ) as unknown[];
      const walks: [string, number, number][] = [
        ["limit1", 1, DEFAULT_LIMITS.maxPageBytes],
        ["limit2", 2, DEFAULT_LIMITS.maxPageBytes],
        ["capped", 100, 1],
      ];
      for (const [key, turns, maxPageBytes] of walks) {
        const pages = walk(events, turns, { maxTurns: 100, maxPageBytes });
        expect(boundaries(pages), `${name} ${key}`).toEqual(pinned[key]);
        const told = pages.toReversed().flatMap((p) => p.frames);
        expect(untimed(told), `${name} ${key} frames`).toEqual(untimed(golden));
      }
    });
  }

  it("keeps a steered chain whole and the open chain out of a page", () => {
    const events = logOf("steer");
    const steering = events.filter((e) => e.kind === "user_message" && e.data.delivery);
    expect(steering.length).toBeGreaterThan(0);
    for (const turns of [1, 2, 3]) {
      for (const page of walk(events, turns, { maxTurns: 100, maxPageBytes: 1 })) {
        for (const e of steering) expect(page.start).not.toBe(e.seq);
      }
    }
    // cut the log while a run is open: the page ends before the run, and the head is further
    for (let upto = 1; upto <= events.length; upto++) {
      const log = events.slice(0, upto);
      const p = readPage(infoOf(log), log, { kind: "turns", turns: 20 }, DEFAULT_LIMITS, upto);
      expect(p.end).toBeLessThanOrEqual(p.head);
      const projector = new Projector(infoOf(log));
      const open = log.map((e) => {
        projector.apply(e);
        return projector.runOpen;
      });
      if (p.end > 0) expect(open[p.end - 1]).toBe(false);
    }
  });

  it("is a catch-up from a settled point, with the anchor of the last run that had ended", () => {
    const events = logOf("followup");
    const second = events.filter((e) => e.kind === "user_message")[1]?.seq ?? 0;
    const page = readPage(
      infoOf(events),
      events,
      { kind: "after", after: second - 1 },
      DEFAULT_LIMITS,
      head(events),
    );
    expect([page.start, page.end]).toEqual([second, head(events)]);
    expect(page.anchor?.seq).toBeLessThan(second);
    expect(page.anchor?.runId).toMatch(/^run-/);
    const none = readPage(
      infoOf(events),
      events,
      { kind: "after", after: 0 },
      DEFAULT_LIMITS,
      head(events),
    );
    expect(none.anchor).toBeUndefined();
  });
});

describe("the mock's history route", () => {
  const server = createMockServer({ stepMs: 2, keepaliveMs: 1000 });
  let base = "";
  beforeAll(async () => {
    await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
    // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
    base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  });
  afterAll(async () => {
    server.closeAllConnections();
    await new Promise<void>((r) => server.close(() => r()));
  });

  const makeLong = async (turns: number): Promise<string> => {
    const res = await fetch(`${base}/__mock/long-thread?turns=${turns}`, { method: "POST" });
    expect(res.status).toBe(201);
    return ((await res.json()) as { threadId: string }).threadId;
  };
  const get = (p: string, init?: RequestInit) => fetch(`${base}${p}`, init);

  it("answers a page of the newest turns, and older ones before it, that tile the stream", async () => {
    const id = await makeLong(30);
    const first = (await (await get(`/agui/threads/${id}/history?limit=5`)).json()) as {
      start: number;
      end: number;
      head: number;
      earlier: boolean;
      projection: number;
      frames: { id?: number; event: { type: string } }[];
    };
    expect(first.earlier).toBe(true);
    expect(first.end).toBe(first.head);
    expect(first.projection).toBeGreaterThan(0);
    expect(first.frames[0]?.event.type).toBe("RUN_STARTED");
    expect(first.frames.at(-1)?.event.type).toBe("RUN_FINISHED");
    const older = (await (
      await get(`/agui/threads/${id}/history?limit=5&before=${first.start}`)
    ).json()) as typeof first;
    expect(older.end + 1).toBe(first.start);
    const whole = await (await get(`/agui/threads/${id}/connect?mode=run`)).text();
    const frames = [...older.frames, ...first.frames].map((f) => f.event.type);
    expect(whole.split("\n\n").filter(Boolean).length).toBeGreaterThan(frames.length);
    // and the connect from `end` says nothing more
    const rest = await (
      await get(`/agui/threads/${id}/connect?mode=run`, {
        headers: { "Last-Event-ID": String(first.end) },
      })
    ).text();
    expect(rest.replace(/^: .*$/gm, "").trim()).toBe("");
  });

  it("refuses what the orchestrator refuses", async () => {
    const id = await makeLong(3);
    const status = async (p: string, init?: RequestInit) => (await get(p, init)).status;
    for (const q of [
      "before=0",
      "before=x",
      "limit=0",
      "limit=101",
      "since=0",
      "after=0",
      "limit=1&since=2",
      "limit=1&after=2",
      "since=1&after=2",
      "before=3&after=2",
    ]) {
      expect(await status(`/agui/threads/${id}/history?${q}`), q).toBe(400);
    }
    expect(
      await status(`/agui/threads/${id}/history`, { headers: { Accept: "text/event-stream" } }),
    ).toBe(406);
    expect(await status(`/agui/threads/00000000-0000-4000-8000-00000000dead/history`)).toBe(404);
    expect(await status(`/agui/threads/not-a-uuid/history`)).toBe(404);
  });

  it("serves `ui.history` as a session sets it, and leaves it out when told to", async () => {
    const config = async (session: string) =>
      (await (
        await get("/api/config", { headers: { Cookie: `mock-registry=${session}` } })
      ).json()) as {
        ui: { history?: Record<string, unknown> };
      };
    expect((await config("a")).ui.history).toEqual({
      initialTurns: 12,
      pageTurns: 20,
      maxTurns: 100,
      projection: 1,
      windowed: false,
    });
    await get("/__mock/config?session=a&history=windowed&initialTurns=3", { method: "POST" });
    expect((await config("a")).ui.history).toMatchObject({ initialTurns: 3, windowed: true });
    await get("/__mock/config?session=a&history=off", { method: "POST" });
    expect((await config("a")).ui).not.toHaveProperty("history");
  });
});

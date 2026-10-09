// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import path from "node:path";
import { cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import {
  foldUsage,
  NO_USAGE,
  summarize,
  type ThreadUsage,
  USAGE_EVENT,
  USAGE_TOTAL_EVENT,
  usageFromCarry,
} from "@/features/chat/lib/usage";
import { type Call, fakeFetch, GOLDEN_DIR, LiveStream, sse, THREAD_ID } from "./testing";
import { ThreadAgent } from "./thread-agent";

configure({ asyncUtilTimeout: 5_000 });
afterEach(cleanup);

/**
 * The carry of a page (docs/api/history.md, "Carry"), with the web's own folds: the pages of two logs as the orchestrator
 * writes them (docs/api/examples/history/*.walk.json, from `orch-agui-projection`'s `tests/carry.rs`), and a reader that holds
 * the newest page and the carry that came with it, then older pages, must show what a replay of the whole thread shows.
 */

type Frame = { id?: number; event: { type: string; name?: string; value?: unknown } };
type Page = {
  start: number;
  end: number;
  head: number;
  earlier: boolean;
  projection: number;
  frames: Frame[];
  carry?: { turns: number; usage?: unknown; files?: Record<string, unknown>[] };
};

const walk = (name: string): Page[] =>
  (
    JSON.parse(
      readFileSync(path.join(GOLDEN_DIR, "..", "history", `${name}.walk.json`), "utf8"),
    ) as { pages: Page[] }
  ).pages;

/** The usage state of a reader that holds `pages[0..=k]`: the carry of the oldest, then every page's usage frames, oldest first. */
function held(pages: Page[], k: number): ThreadUsage {
  let state = usageFromCarry(pages[k]?.carry?.usage);
  for (const page of pages.slice(0, k + 1).reverse()) {
    for (const { event } of page.frames) {
      if (event.type === "CUSTOM") state = foldUsage(state, event.name, event.value);
    }
  }
  return state;
}

describe("the usage a reader shows from the carry", () => {
  const pages = walk("usage-turns");

  it("is the thread's, whichever pages it holds: the newest alone, and every older one added", () => {
    expect(pages.length).toBeGreaterThan(5);
    const whole = summarize(held(pages, pages.length - 1));
    expect(whole.groups.length).toBeGreaterThan(0);
    expect(whole.models.length).toBeGreaterThan(0);
    for (let k = 0; k < pages.length; k++) {
      expect(summarize(held(pages, k)), `holding ${k + 1} pages`).toEqual(whole);
    }
  });

  it("leaves the thread's totals out of the newest page alone: without the carry the ring shows less", () => {
    const whole = summarize(held(pages, pages.length - 1));
    let bare = NO_USAGE;
    for (const { event } of pages[0]?.frames ?? []) {
      if (event.type === "CUSTOM") bare = foldUsage(bare, event.name, event.value);
    }
    const total = (s: ReturnType<typeof summarize>) =>
      s.models.reduce((n, m) => n + m.counts.totalTokens, 0);
    expect(total(summarize(bare))).toBeLessThan(total(whole));
  });

  it("lets a real total of a task replace the total the carry gives it, and counts the calls after it", () => {
    const carry = {
      tasks: [
        {
          task: "T",
          models: [
            { provider: "p", model: "m", inputTokens: 100, outputTokens: 10, totalTokens: 110 },
          ],
        },
      ],
      groups: [
        {
          kind: "agent",
          name: "a",
          calls: 3,
          inputTokens: 100,
          outputTokens: 10,
          totalTokens: 110,
        },
      ],
    };
    const call = (id: string, input: number) => ({
      task: "T",
      call: id,
      model: "m",
      provider: "p",
      inputTokens: input,
      outputTokens: 1,
      totalTokens: input + 1,
      by: { kind: "agent", name: "a" },
    });
    // a call after the carry adds to it
    let state = foldUsage(usageFromCarry(carry), USAGE_EVENT, call("c4", 5));
    expect(summarize(state).models[0]?.counts.totalTokens).toBe(116);
    expect(summarize(state).groups[0]).toMatchObject({ calls: 4 });
    // the task's own total, said later, covers what came before it (the carry's too) and the calls after it add again
    state = foldUsage(state, USAGE_TOTAL_EVENT, {
      task: "T",
      totals: [{ provider: "p", model: "m", inputTokens: 200, outputTokens: 20, totalTokens: 220 }],
    });
    state = foldUsage(state, USAGE_EVENT, call("c5", 7));
    expect(summarize(state).models[0]?.counts.totalTokens).toBe(228);
  });

  it("reads nothing from a carry it does not understand", () => {
    expect(usageFromCarry(undefined)).toBe(NO_USAGE);
    expect(usageFromCarry("x")).toBe(NO_USAGE);
    const state = usageFromCarry({
      tasks: [{ task: "" }, 3],
      groups: [{ kind: "nope" }],
      latest: 5,
    });
    expect(summarize(state).groups).toEqual([]);
    expect(summarize(state).latest).toBeUndefined();
  });
});

/** An agent that opens a thread whose history is `pages` (newest first), served as the route would. */
function open(pages: Page[]) {
  const connect = new LiveStream();
  const asked: string[] = [];
  const { fetch } = fakeFetch((call: Call, request: Request) => {
    if (!call.path.endsWith("/history")) return sse(connect.body);
    const url = new URL(request.url);
    asked.push(url.search.slice(1));
    const before = Number(url.searchParams.get("before"));
    const page = url.searchParams.has("before")
      ? pages.find((p) => p.end === before - 1)
      : pages[0];
    if (!page) return new Response("{}", { status: 404 });
    return new Response(JSON.stringify(page), { headers: { "content-type": "application/json" } });
  });
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    history: () => ({ initialTurns: 1, pageTurns: 1, maxTurns: 100 }),
  });
  return { agent, asked };
}

describe("a thread opened at its end", () => {
  it("shows the usage of the whole thread from the first page, and after each older page", async () => {
    const pages = walk("usage-turns");
    const whole = summarize(held(pages, pages.length - 1));
    const { agent } = open(pages);
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seeded();
    expect(summarize(agent.getSnapshot().usage)).toEqual(summarize(held(pages, 0)));
    expect(summarize(agent.getSnapshot().usage)).toEqual(whole);
    expect(agent.getHistory().turnsBefore).toBe(pages[0]?.carry?.turns);
    for (let k = 1; k < pages.length; k++) {
      await agent.fetchEarlier();
      expect(summarize(agent.getSnapshot().usage), `after ${k} older pages`).toEqual(whole);
      expect(agent.getHistory().turnsBefore).toBe(pages[k]?.carry?.turns ?? 0);
    }
    expect(agent.getHistory().earlier).toBe(false);
    expect(agent.getHistory().turnsBefore).toBe(0);
    agent.stop();
  });

  it("shows the same totals when an older page goes in front of the ones held", async () => {
    const pages = walk("usage-turns");
    const { agent } = open(pages);
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seeded();
    const before = summarize(agent.getSnapshot().usage);
    expect(before.groups[0]).toBeDefined();
    await agent.fetchEarlier();
    expect(summarize(agent.getSnapshot().usage)).toEqual(before);
    agent.stop();
  });

  it("names the kept files before the oldest page it holds, each hash once, and none when it holds them all", async () => {
    const pages = walk("file-turns");
    const { agent } = open(pages);
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seeded();
    const hashes = () => agent.getHistory().files.map((f) => f.sha256);
    expect(hashes()).toEqual(pages[0]?.carry?.files?.map((f) => f.sha256));
    expect(hashes().length).toBeGreaterThan(0);
    for (let k = 1; k < pages.length; k++) {
      await agent.fetchEarlier();
      expect(hashes()).toEqual(pages[k]?.carry?.files?.map((f) => f.sha256) ?? []);
    }
    expect(hashes()).toEqual([]);
    agent.stop();
  });

  it("names a reader's files by the link's route", async () => {
    const pages = walk("file-turns");
    const connect = new LiveStream();
    const { fetch } = fakeFetch((call) =>
      call.path.endsWith("/history")
        ? new Response(JSON.stringify(pages[0]), {
            headers: { "content-type": "application/json" },
          })
        : sse(connect.body),
    );
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: "http://orch.test",
      target: () => ({ agentId: "plain", release: null }),
      backoff: () => 1,
      history: () => ({ initialTurns: 1, pageTurns: 1, maxTurns: 100 }),
      source: { audience: "public", token: "tok" },
    });
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    const files = agent.getHistory().files;
    expect(files.length).toBeGreaterThan(0);
    for (const f of files) expect(String(f.href)).toContain("/shared/tok/artifacts/");
    agent.stop();
  });
});

// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import path from "node:path";
import { cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { components } from "@/lib/api/schema";
import { DEFAULT_LIMITS, readPage } from "../../../../../mock/history";
import { buildTurnSteps, isAgentTurn, type StepMessage } from "../step-tree";
import { buildMessages, joinMessages } from "./seed";
import { fakeFetch, GOLDEN_DIR, LiveStream, sse, THREAD_ID } from "./testing";
import { type ExternalRun, ThreadAgent } from "./thread-agent";

configure({ asyncUtilTimeout: 5_000 });
afterEach(cleanup);

/**
 * The number of a turn ("Turn 3" in the panel) when the thread is opened at its end: the carry's `turns` plus the place among the
 * turns held, which is what the whole thread numbers it, whichever pages are held. The logs are the goldens of
 * docs/api/examples, folded into pages by the mock (whose boundaries and carry are pinned to the orchestrator's:
 * mock/history.test.ts), and the messages are made by the seed (seed.dom.test.tsx says they are the runtime's).
 */

type Event = components["schemas"]["Event"];

const NAMES = [
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
  (
    JSON.parse(readFileSync(path.join(GOLDEN_DIR, "..", `${name}.events.json`), "utf8")) as Event[]
  ).map((e) => ({
    ...e,
    at: new Date((1_800_000_000 + e.seq) * 1000).toISOString(),
    data: e.kind === "agent_message" ? { ...e.data, messageId: `msg-${e.seq}` } : e.data,
  }));

const infoOf = (events: Event[]) => ({
  threadId: THREAD_ID,
  title: String(events.find((e) => e.kind === "user_message")?.data.text).split("\n")[0] ?? "",
  target: { agentId: "plain" },
});

/** The pages of the log the route would answer: the newest `limit` turns, or the ones before `before`. */
function routeOf(events: Event[]) {
  const head = events.at(-1)?.seq ?? 0;
  return (search: URLSearchParams) => {
    const before = search.has("before") ? Number(search.get("before")) : undefined;
    const page = readPage(
      infoOf(events),
      events,
      { kind: "turns", turns: Number(search.get("limit")), ...(before ? { before } : {}) },
      DEFAULT_LIMITS,
      head,
    );
    return { threadId: THREAD_ID, projection: 1, ...page };
  };
}

const view = { state: "done" as const, waiting: false, agentId: "plain" };
const numbers = (messages: readonly StepMessage[], turnsBefore: number): number[] =>
  buildTurnSteps(messages, { ...view, turnsBefore }).map((t) => t.number);

/** A thread opened at its end over `answer`, asking for `initial` turns and then `page`: the agent and the runs of its first page. */
async function open(
  answer: (search: URLSearchParams) => unknown,
  initial: number,
  page: number,
): Promise<{ agent: ThreadAgent; runs: ExternalRun[] }> {
  const connect = new LiveStream();
  const { fetch } = fakeFetch((call, request) =>
    call.path.endsWith("/history")
      ? new Response(JSON.stringify(answer(new URL(request.url).searchParams)), {
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
    history: () => ({ initialTurns: initial, pageTurns: page, maxTurns: 100 }),
  });
  agent.start();
  await waitFor(() => expect(agent.getHistory().enabled).toBe(true));
  const runs = agent.takeSeed() ?? [];
  agent.seeded();
  return { agent, runs };
}

describe("the number of a turn when older turns load", () => {
  for (const name of NAMES) {
    it(`does not change: ${name}`, async () => {
      const answer = routeOf(logOf(name));

      // how the whole thread numbers its turns
      const everything = await open(answer, 100, 100);
      const all = await buildMessages(everything.runs, THREAD_ID);
      everything.agent.stop();
      const whole = numbers(all, 0);
      expect(whole.length, `${name} has agent turns`).toBeGreaterThan(0);
      expect(whole).toEqual(whole.map((_, i) => i + 1));

      // one turn at a time from the end: the numbers of the turns held are the whole thread's
      const { agent, runs } = await open(answer, 1, 1);
      let held = await buildMessages(runs, THREAD_ID);
      let shown = numbers(held, agent.getHistory().turnsBefore);
      expect(shown, "the newest page").toEqual(whole.slice(whole.length - shown.length));
      for (let pages = 1; agent.getHistory().earlier; pages++) {
        expect(pages).toBeLessThan(50);
        const older = await buildMessages(await agent.fetchEarlier(), THREAD_ID);
        held = joinMessages(older, held);
        const now = numbers(held, agent.getHistory().turnsBefore);
        // the turns already shown keep their numbers, and the ones that came are the ones before them
        expect(now.slice(now.length - shown.length), `${name} after ${pages} older pages`).toEqual(
          shown,
        );
        expect(now, `${name} after ${pages} older pages`).toEqual(
          whole.slice(whole.length - now.length),
        );
        shown = now;
      }
      expect(shown).toEqual(whole);
      expect(held.filter((m) => isAgentTurn(m as StepMessage)).length).toBe(whole.length);
      agent.stop();
    }, 30_000);
  }
});

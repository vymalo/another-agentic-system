// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { type Call, fakeFetch, type GoldenFrame, LiveStream, sse, THREAD_ID } from "./testing";
import { mountRuntime } from "./testing-runtime";
import { ThreadAgent } from "./thread-agent";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/*
 * The edges of how the agent reads child sessions: which subagent frames make a card home, what an older
 * page may add to the cards, and what a restart forgets.
 */

const CHILD = "00000000-0000-7000-8000-000000000003";
const REQ = "00000000-0000-7000-8000-0000000000a1";

type Part = { type: string; name?: string; data?: Record<string, unknown> };
type Mounted = ReturnType<typeof mountRuntime>;
const cardsOf = (mounted: Mounted) =>
  mounted
    .messages()
    .flatMap((m) =>
      (m.content as readonly Part[]).filter(
        (p) => p.type === "data" && p.name?.startsWith("agui-activity/vymalo.session"),
      ),
    );

async function playFrames(frames: GoldenFrame[]) {
  const stream = new LiveStream();
  const mounted = mountRuntime(() => sse(stream.body));
  mounted.agent.start();
  await act(async () => {
    stream.frames(frames);
  });
  const last = frames.at(-1)?.id;
  await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(last));
  for (let i = 0; i < 10; i++)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 80));
    });
  return mounted;
}

const runStarted = (id: number | undefined, runId: string) => ({
  ...(id ? { id } : {}),
  event: { type: "RUN_STARTED", threadId: THREAD_ID, runId },
});
const runFinished = (id: number, runId: string) => ({
  id,
  event: { type: "RUN_FINISHED", threadId: THREAD_ID, runId, outcome: { type: "success" } },
});
const reply = (message: string) => [
  { event: { type: "TEXT_MESSAGE_START", messageId: message, role: "assistant" } },
  { event: { type: "TEXT_MESSAGE_CONTENT", messageId: message, delta: "hi" } },
  { event: { type: "TEXT_MESSAGE_END", messageId: message } },
];
const subagent = (type: string, id: string): GoldenFrame =>
  ({
    event: { type, subagentRunId: id, name: "planner", outcome: { type: "success" } },
  }) as GoldenFrame;
const sessionCard = (id: number, state: string, seq = 2): GoldenFrame => ({
  id,
  event: {
    type: "ACTIVITY_SNAPSHOT",
    messageId: `session-${seq}`,
    activityType: "vymalo.session",
    replace: true,
    content: {
      session: `s${seq}`,
      threadId: CHILD,
      agent: "planner",
      description: "d",
      keepOpen: true,
      state,
    },
  },
});
const startCard = (id: number, state: string, request = REQ): GoldenFrame => ({
  id,
  event: {
    type: "ACTIVITY_SNAPSHOT",
    messageId: `session-start-${request}`,
    activityType: "vymalo.sessionStart",
    replace: true,
    content: { request, agent: "planner", description: "d", keepOpen: true, state },
  },
});

describe("which subagent frames make a session card home", () => {
  it("is not a subagent whose id only ends like a session's", async () => {
    const mounted = await playFrames([
      runStarted(1, "run-1"),
      ...reply("m1"),
      subagent("SUBAGENT_STARTED", "other-sub-session-2"),
      sessionCard(2, "idle"),
      subagent("SUBAGENT_FINISHED", "other-sub-session-2"),
      runFinished(3, "run-1"),
    ] as GoldenFrame[]);
    expect(cardsOf(mounted).map((p) => p.data?.home)).toEqual([false]);
    mounted.agent.stop();
  });

  it("is not the run that only says the card again, though that run has a reply of its own", async () => {
    const mounted = await playFrames([
      runStarted(1, "run-1"),
      ...reply("m1"),
      subagent("SUBAGENT_STARTED", "sub-session-2"),
      sessionCard(2, "working"),
      subagent("SUBAGENT_FINISHED", "sub-session-2"),
      runFinished(3, "run-1"),
      runStarted(4, "run-2"),
      ...reply("m2"),
      sessionCard(5, "idle"),
      runFinished(6, "run-2"),
    ] as GoldenFrame[]);
    // the part is there, marked: only the home one is drawn
    expect(cardsOf(mounted).map((p) => p.data?.home)).toEqual([true, false]);
    expect(mounted.agent.getSnapshot().sessions.get("session-2")).toMatchObject({ state: "idle" });
    mounted.agent.stop();
  });
});

describe("the cards across pages and restarts", () => {
  const CONFIG = { initialTurns: 12, pageTurns: 20, maxTurns: 100 };
  const json = (body: unknown) =>
    new Response(JSON.stringify(body), { headers: { "content-type": "application/json" } });
  const isHistory = (call: Call) => call.path === `/agui/threads/${THREAD_ID}/history`;

  const older = {
    start: 1,
    end: 7,
    head: 11,
    earlier: false,
    projection: 1,
    frames: [
      runStarted(1, "run-1"),
      ...reply("m1"),
      subagent("SUBAGENT_STARTED", "sub-session-1"),
      sessionCard(2, "working", 1),
      sessionCard(3, "idle", 2),
      startCard(4, "failed", "00000000-0000-7000-8000-0000000000b2"),
      subagent("SUBAGENT_FINISHED", "sub-session-1"),
      runFinished(4, "run-1"),
    ].map((f, i) => ({ ...f, id: i + 1 })),
  };
  const newest = {
    start: 8,
    end: 11,
    head: 11,
    earlier: true,
    projection: 1,
    frames: [
      runStarted(8, "run-2"),
      ...reply("m2"),
      subagent("SUBAGENT_STARTED", "sub-session-2"),
      sessionCard(9, "closed", 2),
      startCard(10, "opened"),
      subagent("SUBAGENT_FINISHED", "sub-session-2"),
      runFinished(11, "run-2"),
    ],
  };

  function open(page: (query: Record<string, string>) => unknown) {
    const connect = new LiveStream();
    const { fetch, calls } = fakeFetch((call, request) =>
      isHistory(call)
        ? json(page(Object.fromEntries(new URL(request.url).searchParams)))
        : sse(connect.body),
    );
    const agent = new ThreadAgent({
      threadId: THREAD_ID,
      fetch,
      baseUrl: "http://orch.test",
      target: () => ({ agentId: "plain", release: null }),
      backoff: () => 1,
      history: () => CONFIG,
    });
    return { agent, calls };
  }

  it("takes the sessions and starts only an older page says, and keeps what the newer one said", async () => {
    const { agent } = open((q) => (q.before ? older : newest));
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seeded();
    expect(agent.getSnapshot().sessions.get("session-2")).toMatchObject({ state: "closed" });
    expect([...agent.getSnapshot().sessionStarts.keys()]).toEqual([REQ]);
    await agent.fetchEarlier();
    const { sessions, sessionStarts } = agent.getSnapshot();
    expect([...sessions.keys()].sort()).toEqual(["session-1", "session-2"]);
    // the newer page's word of session-2 stands over the older page's
    expect(sessions.get("session-2")).toMatchObject({ state: "closed" });
    expect([...sessionStarts.keys()].sort()).toEqual(
      [REQ, "00000000-0000-7000-8000-0000000000b2"].sort(),
    );
    expect(sessionStarts.get(REQ)).toMatchObject({ state: "opened" });
    agent.stop();
  });

  it("takes the starts only an older page says even when no session is new", async () => {
    const isCard = (f: GoldenFrame) => f.event.activityType === "vymalo.session";
    const onlyStarts = {
      ...older,
      frames: (older.frames as GoldenFrame[]).filter((f) => !isCard(f)),
    };
    const { agent } = open((q) => (q.before ? onlyStarts : newest));
    agent.start();
    await waitFor(() => expect(agent.takeSeed()).not.toBeNull());
    agent.seeded();
    await agent.fetchEarlier();
    expect(agent.getSnapshot().sessionStarts.size).toBe(2);
    agent.stop();
  });

  it("forgets the cards of a start that left before its seed was in", async () => {
    let page: unknown = newest;
    const { agent } = open(() => page);
    agent.start();
    await waitFor(() => expect(agent.getSnapshot().sessions.size).toBe(1));
    agent.stop();
    page = { ...newest, frames: [runStarted(8, "run-2"), runFinished(11, "run-2")] };
    agent.start();
    await waitFor(() => expect(agent.getSnapshot().lastSeq).toBe(11));
    expect(agent.getSnapshot().sessions.size).toBe(0);
    expect(agent.getSnapshot().sessionStarts.size).toBe(0);
    agent.stop();
  });
});

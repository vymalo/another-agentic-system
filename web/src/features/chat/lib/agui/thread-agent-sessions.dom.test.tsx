// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { type GoldenFrame, LiveStream, loadGolden, sse, THREAD_ID } from "./testing";
import { mountRuntime, summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/*
 * Child sessions in the parent (docs/api/agui.md, "Child sessions as subagents"): every late event of a
 * session is a run of its own and says the card again. The agent collects the cards, marks each card's
 * home run (the one that is drawn), and does not offer a run that is only cards, subagent frames and
 * state as a turn.
 */

async function play(name: string) {
  const stream = new LiveStream();
  const mounted = mountRuntime(() => sse(stream.body));
  mounted.agent.start();
  const frames = loadGolden(name);
  await act(async () => {
    stream.frames(frames);
  });
  const last = frames.at(-1)?.id;
  await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(last));
  let seen = -1;
  for (let i = 0; i < 100; i++) {
    const count = mounted.messages().length;
    const running = mounted.runtime().thread.getState().isRunning;
    if (!running && count === seen) break;
    seen = count;
    await act(async () => {
      await new Promise((r) => setTimeout(r, 80));
    });
  }
  return mounted;
}

type Part = { type: string; name?: string; data?: Record<string, unknown> };
const cards = (messages: ReturnType<Awaited<ReturnType<typeof play>>["messages"]>) =>
  messages.flatMap((m) =>
    (m.content as readonly Part[]).filter(
      (p) => p.type === "data" && p.name?.startsWith("agui-activity/vymalo.session"),
    ),
  );

describe("the cards of child sessions, in the agent", () => {
  it("collects each session by its card id, with the latest content", async () => {
    const { agent } = await play("sessions-person");
    const { sessions } = agent.getSnapshot();
    expect([...sessions.keys()]).toEqual(["session-7"]);
    expect(sessions.get("session-7")).toMatchObject({ session: "s1", state: "closed" });
    agent.stop();
  });

  it("collects each start by its request, with the latest state", async () => {
    const { agent } = await play("sessions-person");
    const { sessionStarts } = agent.getSnapshot();
    expect([...sessionStarts.keys()]).toEqual(["00000000-0000-7000-8000-0000000000a1"]);
    expect(sessionStarts.values().next().value).toMatchObject({ state: "opened", session: "s1" });
    agent.stop();
  });

  it("keeps the child's own card apart, and draws nothing of it", async () => {
    const { agent, messages } = await play("sessions-person-child");
    expect(agent.getSnapshot().self).toMatchObject({ state: "closed" });
    expect(agent.getSnapshot().sessions.size).toBe(0);
    expect(summarize(messages()).flatMap((m) => m.parts)).not.toContain("session");
    agent.stop();
  });

  it("marks a card home only in the run that opened it, and offers no late run as a turn", async () => {
    const { agent, messages } = await play("sessions-person");
    expect(summarize(messages())).toEqual([
      { role: "user", parts: ["text:survey the options"] },
      {
        role: "assistant",
        status: "complete",
        parts: [
          "actor",
          "status:working",
          "text:Surveyed; ask a helper to go deeper.",
          "status:completed",
        ],
      },
      { role: "assistant", status: "complete", parts: ["sessionStart"] },
      { role: "assistant", status: "complete", parts: ["actor", "session"] },
    ]);
    expect(cards(messages()).map((p) => p.data?.home)).toEqual([true, true]);
    agent.stop();
  });

  it("marks the start home where it is starting", async () => {
    const { agent, messages } = await play("sessions-start-failed");
    expect(summarize(messages()).map((m) => m.parts)).toEqual([
      ["text:ask the planner"],
      ["actor", "status:working", "text:Ready.", "status:completed"],
      ["sessionStart"],
    ]);
    expect(cards(messages()).map((p) => p.data?.home)).toEqual([true]);
    expect(agent.getSnapshot().sessionStarts.values().next().value).toMatchObject({
      state: "failed",
    });
    agent.stop();
  });

  it("draws the sessions of an agent's run in it, and its late reports in no turn", async () => {
    const { agent, messages } = await play("sessions");
    const turns = summarize(messages());
    expect(turns).toHaveLength(2);
    expect(turns[1]?.parts.filter((p) => p === "session")).toHaveLength(2);
    expect(cards(messages()).every((p) => p.data?.home === true)).toBe(true);
    expect(agent.getSnapshot().sessions.size).toBe(2);
    agent.stop();
  });
});

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

const CHILD = "00000000-0000-7000-8000-000000000003";
const REQ = "00000000-0000-7000-8000-0000000000a1";
const reply = [
  { event: { type: "TEXT_MESSAGE_START", messageId: "m1", role: "assistant" } },
  { event: { type: "TEXT_MESSAGE_CONTENT", messageId: "m1", delta: "hi" } },
  { event: { type: "TEXT_MESSAGE_END", messageId: "m1" } },
];
const runStarted = (id?: number) => ({
  ...(id ? { id } : {}),
  event: { type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-1" },
});
const runFinished = (id: number) => ({
  id,
  event: {
    type: "RUN_FINISHED",
    threadId: THREAD_ID,
    runId: "run-1",
    outcome: { type: "success" },
  },
});
const sessionCard = (id: number, state: string): GoldenFrame => ({
  id,
  event: {
    type: "ACTIVITY_SNAPSHOT",
    messageId: "session-2",
    activityType: "vymalo.session",
    replace: true,
    content: {
      session: "s1",
      threadId: CHILD,
      agent: "planner",
      description: "d",
      keepOpen: true,
      state,
    },
  },
});
const startCard = (id: number, state: string): GoldenFrame => ({
  id,
  event: {
    type: "ACTIVITY_SNAPSHOT",
    messageId: `session-start-${REQ}`,
    activityType: "vymalo.sessionStart",
    replace: true,
    content: {
      request: REQ,
      agent: "planner",
      description: "d",
      keepOpen: true,
      state,
      ...(state === "opened" ? { threadId: CHILD, session: "s1" } : {}),
      ...(state === "failed" ? { reason: "unknown_helper" } : {}),
    },
  },
});

describe("a card's home across a resume and within one run", () => {
  it("keeps a session card home when the run is resumed in the middle", async () => {
    const { agent, messages } = await playFrames([
      runStarted(1),
      ...reply,
      {
        id: 2,
        event: { type: "SUBAGENT_STARTED", subagentRunId: "sub-session-2", name: "planner" },
      },
      sessionCard(3, "working"),
      runStarted(),
      { event: { type: "SUBAGENT_STARTED", subagentRunId: "sub-session-2", name: "planner" } },
      sessionCard(4, "idle"),
      {
        event: {
          type: "SUBAGENT_FINISHED",
          subagentRunId: "sub-session-2",
          outcome: { type: "suspended" },
        },
      },
      runFinished(5),
    ] as GoldenFrame[]);
    expect(cards(messages()).map((p) => p.data?.home)).toEqual([true]);
    agent.stop();
  });

  for (const outcome of ["opened", "failed"]) {
    it(`keeps a start line home when its ${outcome} arrives in the same run`, async () => {
      const { agent, messages } = await playFrames([
        runStarted(1),
        ...reply,
        startCard(2, "starting"),
        startCard(3, outcome),
        runFinished(5),
      ] as GoldenFrame[]);
      const starts = messages().flatMap((m) =>
        (m.content as readonly Part[]).filter(
          (p) => p.type === "data" && p.name?.startsWith("agui-activity/vymalo.sessionStart"),
        ),
      );
      expect(starts.map((p) => [p.data?.state, p.data?.home])).toEqual([[outcome, true]]);
      agent.stop();
    });
  }

  describe("a card that cannot be read", () => {
    for (const [name, type] of [
      ["session", "vymalo.session"],
      ["start", "vymalo.sessionStart"],
    ] as const) {
      it(`is quiet: a run of nothing else is no turn (${name})`, async () => {
        const { agent, messages } = await playFrames([
          runStarted(1),
          {
            id: 2,
            event: {
              type: "ACTIVITY_SNAPSHOT",
              messageId: name === "session" ? "session-2" : `session-start-${REQ}`,
              activityType: type,
              replace: true,
              content: { state: "nonsense" },
            },
          },
          runFinished(3),
        ] as GoldenFrame[]);
        expect(messages()).toHaveLength(0);
        agent.stop();
      });
    }
  });
});

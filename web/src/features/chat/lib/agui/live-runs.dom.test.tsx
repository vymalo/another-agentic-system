// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { type GoldenFrame, LiveStream, loadGolden, sse, THREAD_ID } from "./testing";
import { mountRuntime, summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * A replay hands the runtime several runs at once (a fork's marker run, two jobs of one thread),
 * and `thread.getState()` shows what the runtime holds one render late. These tests make that
 * render late by as much as they like: inside an async `act`, React does not render what the
 * runtime notifies until the scope ends, so the transcript stays as it was while the replay has
 * long been applied. Whatever the delay, the transcript has to come out whole.
 */
const LATE_MS = 400;

const USER = (text: string) => ({ role: "user", parts: [`text:${text}`] });
const WORK = ["actor", "status:working", "artifact", "status:completed"];

/**
 * The frames of runs that hold only what is given: a message, a marker. Resume points count up
 * through the whole replay, as the log's do.
 */
function replayOf(...runs: ((n: number) => GoldenFrame[])[]): GoldenFrame[] {
  return runsAfter(0, ...runs);
}
/** The same runs, numbered on from the log's resume point `from`. */
function runsAfter(from: number, ...runs: ((n: number) => GoldenFrame[])[]): GoldenFrame[] {
  let id = from;
  return runs.flatMap((body, i) => {
    const runId = `run-${from + i + 1}`;
    const frames = [
      { type: "RUN_STARTED", threadId: THREAD_ID, runId },
      ...body(from + i + 1).map((f) => f.event),
      { type: "RUN_FINISHED", threadId: THREAD_ID, runId, outcome: { type: "success" } },
    ];
    // the end of a message and of a run are the events that carry a resume point
    return frames.map((event) => ({
      event,
      ...(event.type === "TEXT_MESSAGE_END" || event.type === "RUN_FINISHED" ? { id: ++id } : {}),
    }));
  });
}
const frame = (event: Record<string, unknown>): GoldenFrame => ({ event });
const said = (text: string) => (n: number) => {
  const messageId = `msg-${n}`;
  return [
    frame({ type: "TEXT_MESSAGE_START", messageId, role: "user" }),
    frame({ type: "TEXT_MESSAGE_CONTENT", messageId, delta: text }),
    frame({ type: "TEXT_MESSAGE_END", messageId }),
  ];
};
const marker = (n: number) => [frame({ type: "CUSTOM", name: "marker", value: { n } })];
const bare = (role: string, parts: string[]) => ({ role, status: "complete", parts });

/**
 * Writes `frames` in one burst in a scope that holds the render back for `lateMs`, and gives the
 * mounted runtime. `drawn` are frames written first and drawn normally (`messages` is what the
 * transcript shows of them): the page is up, and then the runs come.
 */
async function replay(
  frames: GoldenFrame[],
  drawn: { frames: number; messages: number } = { frames: 0, messages: 0 },
  lateMs = LATE_MS,
) {
  const stream = new LiveStream();
  const mounted = mountRuntime(() => sse(stream.body));
  mounted.agent.start();
  if (drawn.frames > 0) {
    await act(async () => {
      stream.frames(frames.slice(0, drawn.frames));
    });
    await waitFor(() => expect(mounted.messages()).toHaveLength(drawn.messages));
  }
  await act(async () => {
    stream.frames(frames.slice(drawn.frames));
    await new Promise((r) => setTimeout(r, lateMs));
  });
  return mounted;
}

describe("a replay whose render comes late", () => {
  for (const [golden, expected] of [
    [
      "connect-fork",
      [
        USER("echo one"),
        bare("assistant", WORK),
        bare("assistant", ["fork"]),
        USER("echo three"),
        bare("assistant", ["job", ...WORK]),
      ],
    ],
    [
      "connect-fork-blocked",
      [
        USER("ask about branches"),
        bare("assistant", [
          "actor",
          "status:working",
          "text:Which branch?",
          "status:input_required",
        ]),
        bare("assistant", ["fork"]),
        USER("echo thanks"),
        bare("assistant", ["job", ...WORK]),
      ],
    ],
    [
      "followup",
      [
        USER("echo hi"),
        bare("assistant", WORK),
        USER("echo now add tests"),
        bare("assistant", ["job", ...WORK]),
      ],
    ],
  ] as const) {
    it(`${golden}: every run is in the transcript, the render ${LATE_MS} ms behind`, async () => {
      const mounted = await replay(loadGolden(golden));
      await waitFor(() => expect(summarize(mounted.messages())).toEqual(expected));
      await waitFor(() => expect(mounted.agent.getSnapshot().state).toBe("done"));
      mounted.agent.stop();
    });
  }

  it("markers that follow a transcript already on screen add one message each, in order", async () => {
    const frames = replayOf(said("first"), marker, marker, said("then"));
    const first = frames.findIndex((f) => f.event.type === "RUN_FINISHED");
    const mounted = await replay(frames, { frames: first + 1, messages: 2 });
    await waitFor(() =>
      expect(summarize(mounted.messages())).toEqual([
        USER("first"),
        bare("assistant", []),
        bare("assistant", ["marker"]),
        bare("assistant", ["marker"]),
        USER("then"),
        bare("assistant", []),
      ]),
    );
    mounted.agent.stop();
  });

  it("a run that answers a question, and the marker after it: the marker is not hung off the question's turn", async () => {
    // the question is drawn; the answer's run goes through the runtime's steer-away, which
    // leaves a run after it with nothing else to say how many messages there are
    const ask = loadGolden("ask");
    const frames = [...ask, ...runsAfter(9, marker)];
    const asked = ask.findIndex((f) => f.id === 4) + 1;
    const mounted = await replay(frames, { frames: asked, messages: 2 });
    await waitFor(() =>
      expect(summarize(mounted.messages())).toEqual([
        USER("ask about branches"),
        bare("assistant", [
          "actor",
          "status:working",
          "text:Which branch?",
          "status:input_required",
        ]),
        USER("main"),
        bare("assistant", WORK),
        bare("assistant", ["marker"]),
      ]),
    );
    mounted.agent.stop();
  });

  it("a run that failed, and the markers after it: the failed turn stays", async () => {
    const one = replayOf(said("first"));
    const failing = [
      frame({ type: "RUN_STARTED", threadId: THREAD_ID, runId: "run-2" }),
      ...said("second")(2),
      frame({ type: "RUN_ERROR", code: "agent_failed", message: "it broke" }),
    ].map((f, i, all) => ({ ...f, ...(i === all.length - 1 ? { id: 3 } : {}) }));
    const frames = [...one, ...failing, ...runsAfter(3, marker, marker)];
    const mounted = await replay(frames, { frames: one.length, messages: 2 });
    await waitFor(() =>
      expect(summarize(mounted.messages()).map((m) => [m.role, m.parts.at(0)])).toEqual([
        ["user", "text:first"],
        ["assistant", undefined],
        ["user", "text:second"],
        ["assistant", undefined],
        ["assistant", "marker"],
        ["assistant", "marker"],
      ]),
    );
    mounted.agent.stop();
  });
});

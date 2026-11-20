// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LiveStream, loadGolden, sse } from "./testing";
import { mountRuntime, type Summary, summarize } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

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

/**
 * The child-session goldens (docs/api/examples/agui/sessions*.agui.json) through the runtime the app uses.
 * Every late event of a session is a run of its own and says the same card again: the transcript has one
 * card for each session, in the run that opened it, one start line for each request, and no turn for a run
 * that only says a card again. The child's own card (`session-self`) is never in the transcript.
 */
const DONE = "complete";
const EXPECTED: Record<string, Summary> = {
  // two sessions opened inside the agent's run; their closes and late reports are runs of their own
  sessions: [
    { role: "user", parts: ["text:survey the options and write a summary"] },
    {
      role: "assistant",
      status: DONE,
      parts: [
        "actor",
        "status:working",
        "actor",
        "step",
        "actor",
        "session",
        "actor",
        "session",
        "actor",
        "actor",
        "text:Both helpers reported.",
        "status:completed",
      ],
    },
  ],
  "sessions-failed": [
    { role: "user", parts: ["text:build it with a helper"] },
    {
      role: "assistant",
      status: DONE,
      parts: [
        "actor",
        "status:working",
        "actor",
        "session",
        "text:The helper failed; I built it myself.",
        "status:completed",
      ],
    },
  ],
  // the child's log: only the rounds
  "sessions-child": [
    {
      role: "assistant",
      status: DONE,
      parts: ["job", "actor", "status:working", "step", "text:Found it.", "status:completed"],
    },
    {
      role: "assistant",
      status: DONE,
      parts: ["job", "actor", "status:working", "text:Second round done.", "status:completed"],
    },
  ],
  "sessions-person-child": [
    { role: "user", parts: ["text:List every place the writers are named"] },
    {
      role: "assistant",
      status: DONE,
      parts: ["actor", "status:working", "text:Found the writers.", "status:completed"],
    },
    { role: "user", parts: ["text:Now the editors"] },
    { role: "assistant", status: DONE, parts: ["job", "actor", "status:working"] },
    { role: "user", parts: ["text:Include the translators"] },
    {
      role: "assistant",
      status: DONE,
      parts: ["actor", "text:Found the editors and the translators.", "status:completed"],
    },
  ],
  // a start (run 6), the session it opened (run 7), the start opened, a report and a close: two cards' worth, one each
  "sessions-person": [
    { role: "user", parts: ["text:survey the options"] },
    {
      role: "assistant",
      status: DONE,
      parts: [
        "actor",
        "status:working",
        "text:Surveyed; ask a helper to go deeper.",
        "status:completed",
      ],
    },
    { role: "assistant", status: DONE, parts: ["sessionStart"] },
    { role: "assistant", status: DONE, parts: ["actor", "session"] },
  ],
  "sessions-start-failed": [
    { role: "user", parts: ["text:ask the planner"] },
    {
      role: "assistant",
      status: DONE,
      parts: ["actor", "status:working", "text:Ready.", "status:completed"],
    },
    { role: "assistant", status: DONE, parts: ["sessionStart"] },
  ],
};

/** The ids a golden says: each session card id and each start request, once. */
function said(name: string) {
  const ids = (activityType: string) =>
    new Set(
      loadGolden(name).flatMap(({ event }) =>
        event.type === "ACTIVITY_SNAPSHOT" && event.activityType === activityType
          ? [String(event.messageId)]
          : [],
      ),
    );
  const sessions = ids("vymalo.session");
  sessions.delete("session-self");
  return { sessions: sessions.size, starts: ids("vymalo.sessionStart").size };
}

describe("the child-session goldens through the runtime", () => {
  for (const [name, expected] of Object.entries(EXPECTED)) {
    it(`${name}: one card for each session, one line for each start, no turn for a late report`, async () => {
      const { messages, agent } = await play(name);
      const turns = summarize(messages());
      expect(turns).toEqual(expected);
      const parts = turns.flatMap((m) => m.parts);
      const count = said(name);
      expect(parts.filter((p) => p === "session")).toHaveLength(count.sessions);
      expect(parts.filter((p) => p === "sessionStart")).toHaveLength(count.starts);
      agent.stop();
    });
  }
});

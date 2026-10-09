import { readdirSync } from "node:fs";
import { act, waitFor } from "@testing-library/react";
import { expect } from "vitest";
import { fakeFetch, GOLDEN_DIR, type GoldenFrame, LiveStream, sse, THREAD_ID } from "./testing";
import { mountRuntime } from "./testing-runtime";
import { ThreadAgent } from "./thread-agent";

/*
 * Test support for the transcripts of a thread opened at its end (ADR 0059): what the visible runtime holds after a replay,
 * the runs a page of frames makes, and the messages compared without what the runtime makes up each time. jsdom only.
 */

const SKIP = /^(capabilities|run-)/;
/** The names of the goldens that are a thread's frames. */
export const GOLDENS = readdirSync(GOLDEN_DIR)
  .filter((f) => f.endsWith(".agui.json") && !SKIP.test(f))
  .map((f) => f.replace(".agui.json", ""))
  .sort();

/** The goldens that end at a settled point (every run that starts ends, and the last frame is a resume point): a page never ends elsewhere. */
export const settled = (frames: GoldenFrame[]): boolean => {
  const types = frames.map((f) => f.event.type);
  const starts = types.filter((t) => t === "RUN_STARTED").length;
  const ends = types.filter((t) => t === "RUN_FINISHED" || t === "RUN_ERROR").length;
  return starts > 0 && starts === ends && frames.at(-1)?.id !== undefined;
};

/** What the visible runtime holds after the replay of a golden. */
export async function replay(frames: GoldenFrame[]) {
  const stream = new LiveStream();
  const mounted = mountRuntime(() => sse(stream.body));
  mounted.agent.start();
  await act(async () => {
    stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(mounted.agent.getSnapshot().lastSeq).toBe(last));
  await waitFor(() => expect(mounted.runtime().thread.getState().isRunning).toBe(false));
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
  const messages = [...mounted.messages()];
  mounted.agent.stop();
  return messages;
}

/** The runs a page of these frames makes, as a thread opened at its end collects them (none: the page shows nothing). */
export async function seedRuns(frames: GoldenFrame[]) {
  const stream = new LiveStream();
  const end = frames.filter((f) => f.id !== undefined).at(-1)?.id as number;
  const { fetch, calls } = fakeFetch((call) =>
    call.path.endsWith("/history")
      ? new Response(
          JSON.stringify({ start: 1, end, head: end, earlier: false, projection: 1, frames }),
          { headers: { "content-type": "application/json" } },
        )
      : sse(stream.body),
  );
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    history: () => ({ initialTurns: 12, pageTurns: 20, maxTurns: 100 }),
  });
  agent.start();
  let runs: ReturnType<typeof agent.takeSeed> = null;
  // a page that makes no run is not seeded: the stream is opened straight away
  await waitFor(() => {
    runs = agent.takeSeed();
    expect(
      runs !== null || calls.some((c) => c.method === "GET" && !c.path.endsWith("/history")),
    ).toBe(true);
  });
  agent.seeded();
  agent.stop();
  return runs ?? [];
}

/** A message without what the runtime makes up each time: the ids of messages the server gave none (seven random characters), and the clock. */
const bare = (messages: readonly { id: string }[]) =>
  JSON.parse(
    JSON.stringify(messages, (key, value) =>
      key === "createdAt" || key === "timing" ? undefined : value,
    ),
  ) as { id: string; content: unknown[] }[];

/** The messages with the ids the runtime makes up replaced, so two transcripts of the same turns compare equal. */
export const stable = (list: readonly { id: string }[]) =>
  bare(list).map((m) => ({ ...m, id: /^[A-Za-z0-9]{7}$/.test(m.id) ? "generated" : m.id }));

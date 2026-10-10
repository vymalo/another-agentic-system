// @vitest-environment jsdom
import { AssistantRuntimeProvider, ThreadPrimitive } from "@assistant-ui/react";
import { type AgUiAssistantRuntime, useAgUiRuntime } from "@assistant-ui/react-ag-ui";
import { act, cleanup, configure, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { HistorySeed } from "@/features/chat/components/history-seed";
import { LiveRuns } from "@/features/chat/components/live-runs";
import { dropFailedSend } from "./failed-send";
import {
  fakeFetch,
  type GoldenFrame,
  LiveStream,
  loadGolden,
  problem,
  sse,
  THREAD_ID,
} from "./testing";
import { GOLDENS, settled, stable } from "./testing-history";
import { summarize } from "./testing-runtime";
import { ThreadAgent } from "./thread-agent";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

/**
 * A thread opened at its end (ADR 0059) holds the transcript the seed imported, not the one a replay built. A send the server
 * refuses is taken back out of it (`dropFailedSend`, from the runtime's `onError`), and what the transcript shows must survive
 * that, whenever the refusal comes.
 */

const lastId = (frames: GoldenFrame[]) => frames.filter((f) => f.id !== undefined).at(-1)?.id ?? 0;

function mount(
  frames: GoldenFrame[],
  {
    refuse,
    drop = true,
    seed = true,
  }: {
    /** The server's answer to a message: refused (403), or accepted. */
    refuse: boolean;
    /** Take the message back from `onError`, as the page does; off, the test does it at the moment it chooses. */
    drop?: boolean;
    /** The seed is mounted at once; off, `box.seed()` mounts it. */
    seed?: boolean;
  },
) {
  const end = lastId(frames);
  const connect = new LiveStream();
  const { fetch, calls } = fakeFetch((call) => {
    if (call.method === "POST") {
      if (refuse) return problem(403, "Forbidden", "your roles do not grant thread.write");
      const accepted = new LiveStream();
      accepted.frames([
        {
          event: {
            type: "RUN_STARTED",
            threadId: THREAD_ID,
            runId: (call.body as { runId: string }).runId,
            protocolVersion: "1.0",
          },
        },
      ]);
      return sse(accepted.body);
    }
    return call.path.endsWith("/history")
      ? new Response(
          JSON.stringify({ start: 1, end, head: end, earlier: false, projection: 1, frames }),
          { headers: { "content-type": "application/json" } },
        )
      : sse(connect.body);
  });
  const agent = new ThreadAgent({
    threadId: THREAD_ID,
    fetch,
    baseUrl: "http://orch.test",
    target: () => ({ agentId: "plain", release: null }),
    backoff: () => 1,
    history: () => ({ initialTurns: 12, pageTurns: 20, maxTurns: 100 }),
  });
  const box: {
    runtime?: AgUiAssistantRuntime;
    seed: () => void;
    refused: string[];
    texts: (string | undefined)[];
  } = { seed: () => {}, refused: [], texts: [] };
  function Harness() {
    const [seeding, setSeeding] = useState(seed);
    box.seed = () => setSeeding(true);
    const runtime = useAgUiRuntime({
      agent,
      resumeTranscript: "appended",
      // what use-chat-runtime does with a refused send
      onError: () => {
        const failure = agent.takeSendError();
        if (!failure || !box.runtime) return;
        box.refused.push(failure.message);
        if (drop) box.texts.push(dropFailedSend(box.runtime));
      },
    });
    box.runtime = runtime;
    return (
      <AssistantRuntimeProvider runtime={runtime}>
        <LiveRuns agent={agent} runtime={runtime} />
        {seeding ? <HistorySeed agent={agent} runtime={runtime} /> : null}
        <ThreadPrimitive.Root>
          <div role="log" aria-label="Conversation">
            <ThreadPrimitive.Messages>
              {({ message }) => <p data-role={message.role}>{message.id}</p>}
            </ThreadPrimitive.Messages>
          </div>
        </ThreadPrimitive.Root>
      </AssistantRuntimeProvider>
    );
  }
  render(<Harness />);
  const runtime = () => box.runtime as AgUiAssistantRuntime;
  const thread = () => runtime().thread;
  const messages = () => thread().getState().messages;
  const shown = () => screen.getByRole("log", { name: "Conversation" }).querySelectorAll("p");
  return { agent, calls, box, runtime, thread, messages, shown };
}

const ECHO = loadGolden("echo");

const userText = (message: { content: readonly { type: string; text?: string }[] }) =>
  message.content.flatMap((p) => (p.type === "text" && p.text !== undefined ? [p.text] : []));

describe("a message the server refuses, on a thread opened at its end", () => {
  it("is taken back and the transcript on screen is the one before it", async () => {
    const m = mount(ECHO, { refuse: true });
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
    await waitFor(() => expect(m.shown()).toHaveLength(2));
    const before = stable(m.messages());

    await act(async () => {
      m.thread().append("echo more");
    });
    await waitFor(() => expect(m.box.texts).toEqual(["echo more"]));
    expect(m.box.refused[0]).toContain("your roles do not grant");
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    expect(stable(m.messages())).toEqual(before);
    await waitFor(() => expect(m.shown()).toHaveLength(2));
    m.agent.stop();
  });

  it("leaves a transcript whose every message can be looked up, from the moment it is taken back", async () => {
    // the page draws what the runtime lists (a message client per id) in a render of its own, which can come after the
    // refusal: an id the runtime lists and cannot look up is a render that throws and takes the page with it
    const m = mount(ECHO, { refuse: true, drop: false });
    m.agent.start();
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    await waitFor(() => expect(m.agent.idleForImport()).toBe(true));

    await act(async () => {
      m.thread().append("echo more");
    });
    await waitFor(() => expect(m.box.refused).toHaveLength(1));
    await waitFor(() => expect(m.messages().length).toBeGreaterThan(2));

    let text: string | undefined;
    await act(async () => {
      text = dropFailedSend(m.runtime());
      for (const { id } of m.messages()) expect(() => m.thread().getMessageById(id)).not.toThrow();
    });
    expect(text).toBe("echo more");
    await waitFor(() => expect(m.messages()).toHaveLength(2));
    for (const { id } of m.messages()) expect(() => m.thread().getMessageById(id)).not.toThrow();
    m.agent.stop();
  });

  for (const name of GOLDENS) {
    const frames = loadGolden(name);
    if (!settled(frames)) continue;
    it(`${name}: leaves the transcript as the seed made it`, async () => {
      const m = mount(frames, { refuse: true });
      m.agent.start();
      await waitFor(() => expect(m.messages().length).toBeGreaterThan(0));
      await waitFor(() => expect(m.agent.idleForImport()).toBe(true));
      const before = stable(m.messages());
      const summary = summarize(m.messages());

      await act(async () => {
        m.thread().append("one more thing");
      });
      await waitFor(() => expect(m.box.texts).toEqual(["one more thing"]));
      await waitFor(() => expect(m.messages()).toHaveLength(before.length));
      expect(summarize(m.messages())).toEqual(summary);
      expect(stable(m.messages())).toEqual(before);
      m.agent.stop();
    }, 60_000);
  }
});

describe("the import of the seed", () => {
  it("replaces a message added before it, which is why the composer holds a send until the transcript is in", async () => {
    const m = mount(ECHO, { refuse: false, seed: false });
    m.agent.start();
    // the page has been read (the thread says Done) and the seed waits to be imported
    await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(lastId(ECHO)));

    await act(async () => {
      m.thread().append("echo more");
    });
    await waitFor(() => expect(m.calls.some((c) => c.method === "POST")).toBe(true));
    await waitFor(() => expect(m.messages().map((x) => x.role)).toEqual(["user", "assistant"]));

    await act(async () => {
      m.box.seed();
    });
    await waitFor(() => expect(m.agent.getSnapshot().replaying).toBe(false));
    // only the seed's own turn is left: the message was accepted by the server and is not in the transcript
    expect(m.messages().flatMap((x) => (x.role === "user" ? userText(x) : []))).toEqual([
      "echo hi",
    ]);
    m.agent.stop();
  });
});

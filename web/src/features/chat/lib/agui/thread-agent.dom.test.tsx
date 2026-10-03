// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { dropFailedSend } from "./failed-send";
import {
  framesThrough,
  type GoldenFrame,
  LiveStream,
  loadGolden,
  problem,
  sse,
  THREAD_ID,
} from "./testing";
import { mountRuntime, summarize } from "./testing-runtime";
import { SendError } from "./thread-agent";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

const startedFrame = (runId: string): GoldenFrame => ({
  event: { type: "RUN_STARTED", threadId: THREAD_ID, runId, protocolVersion: "1.0" },
});

describe("the runtime with a ThreadAgent: runs the user starts", () => {
  it("posts the message, is accepted at RUN_STARTED and takes the run from the connect stream", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime, messages } = mountRuntime((call) => {
      if (call.method === "GET") return sse(connect.body);
      const post = new LiveStream();
      post.frames([startedFrame((call.body as { runId: string }).runId)]);
      return sse(post.body);
    });
    agent.start();
    await act(async () => {
      await runtime().thread.append("ask about branches");
    });
    // the runtime's message went out, and only that message
    const post = calls.find((c) => c.method === "POST");
    expect(post?.path).toBe("/agui/agents/plain");
    const body = post?.body as { runId: string; messages: { role: string; content: string }[] };
    expect(body.messages).toHaveLength(1);
    expect(body.messages[0]).toMatchObject({ role: "user", content: "ask about branches" });

    // the connect stream carries the run: its user message is the runtime's own, shown once
    // the first run of `ask`: up to its interrupt (resume point 4)
    const golden = framesThrough(loadGolden("ask"), 4).map((f) =>
      f.event.type === "RUN_STARTED" || f.event.type === "RUN_FINISHED"
        ? { ...f, event: { ...f.event, runId: body.runId } }
        : f,
    );
    await act(async () => {
      connect.frames(golden);
    });
    await waitFor(() => expect(messages()).toHaveLength(2));
    expect(summarize(messages())).toEqual([
      { role: "user", parts: ["text:ask about branches"] },
      {
        role: "assistant",
        status: "requires-action:interrupt",
        parts: ["actor", "status:working", "text:Which branch?", "status:input_required"],
      },
    ]);
    agent.stop();
  });

  it("answers the interrupt with a resume, once: the answer is sent as resume, not as a message", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime, messages } = mountRuntime((call) => {
      if (call.method === "GET") return sse(connect.body);
      const post = new LiveStream();
      post.frames([startedFrame((call.body as { runId: string }).runId)]);
      return sse(post.body);
    });
    agent.start();
    await act(async () => {
      connect.frames(framesThrough(loadGolden("ask"), 4));
    });
    await waitFor(() => expect(runtime().unstable_getPendingInterrupts()).toHaveLength(1));

    let runId = "";
    await act(async () => {
      const done = runtime().unstable_submitInterruptResponses([
        { interruptId: "int-3", status: "resolved", payload: { text: "main" } },
      ]);
      await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
      runId = (calls.find((c) => c.method === "POST") as { body: { runId: string } }).body.runId;
      const ask = loadGolden("ask");
      const rest = ask
        .slice(framesThrough(ask, 4).length)
        .map((f) =>
          f.event.type === "RUN_STARTED" || f.event.type === "RUN_FINISHED"
            ? { ...f, event: { ...f.event, runId } }
            : f,
        );
      connect.frames(rest);
      await done;
    });
    const post = calls.find((c) => c.method === "POST")?.body as Record<string, unknown>;
    expect(post.messages).toEqual([]);
    expect(post.resume).toEqual([
      { interruptId: "int-3", status: "resolved", payload: { text: "main" } },
    ]);
    // the answer's user triad (evt-5) is the same message the requester never showed: not twice
    expect(summarize(messages()).map((m) => m.role)).toEqual(["user", "assistant", "assistant"]);
    agent.stop();
  });

  it("a refused send fails the run with the problem, and dropFailedSend takes the message back", async () => {
    const { agent, runtime, messages, box } = mountRuntime((call) =>
      call.method === "POST"
        ? problem(409, "Conflict", "a run is already open on this thread; wait for it to finish")
        : sse(new LiveStream().body),
    );
    agent.start();
    await act(async () => {
      runtime().thread.append("hello");
    });
    // the runtime reports a failed run through onError (twice, from two of its paths)
    await waitFor(() => expect(box.errors.length).toBeGreaterThanOrEqual(1));
    expect(box.errors[0]?.message).toContain("a run is already open");
    expect(agent.takeSendError()).toMatchObject({ status: 409 });
    expect(messages().map((m) => m.role)).toEqual(["user", "assistant"]);

    let text: string | undefined;
    await act(async () => {
      text = dropFailedSend(runtime());
    });
    expect(text).toBe("hello");
    expect(messages()).toEqual([]);
    agent.stop();
  });
});

describe("the runtime with a ThreadAgent: runs nobody here started", () => {
  it("a run in flight when the page loads is followed to its end", async () => {
    const stream = new LiveStream();
    const { agent, runtime, messages } = mountRuntime(() => sse(stream.body));
    agent.start();
    const echo = loadGolden("connect-echo");
    const upTo2 = echo.slice(0, echo.findIndex((f) => f.id === 2) + 1);
    await act(async () => {
      stream.frames(upTo2);
    });
    await waitFor(() => expect(messages()).toHaveLength(2));
    expect(runtime().thread.getState().isRunning).toBe(true);
    expect(summarize(messages())[1]).toEqual({
      role: "assistant",
      status: "running:",
      parts: ["actor", "status:working"],
    });

    await act(async () => {
      stream.frames(echo.slice(upTo2.length));
    });
    await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(false));
    expect(summarize(messages())[1]).toEqual({
      role: "assistant",
      status: "complete",
      parts: ["actor", "status:working", "artifact", "status:completed"],
    });
    agent.stop();
  });

  it("a connection cut in the middle of a run makes no difference to the transcript", async () => {
    const talk = loadGolden("talk");
    const at4 = talk.findIndex((f) => f.id === 4); // the agent's message ends here
    const preamble = [talk[0], talk[5], talk[7]] as GoldenFrame[]; // run, invocation, state
    const first = new LiveStream();
    const second = new LiveStream();
    const streams = [first, second];
    const { agent, calls, runtime, messages } = mountRuntime(() =>
      sse((streams.shift() as LiveStream).body),
    );
    agent.start();
    await act(async () => {
      first.frames(talk.slice(0, at4 - 1)); // cut inside the agent's message
    });
    await waitFor(() => expect(messages().length).toBe(2));
    await act(async () => {
      first.cut();
    });
    await waitFor(() => expect(calls).toHaveLength(2));
    // the last resume point it holds is id 3 (the second status line)
    expect(calls[1]?.lastEventId).toBe("3");
    const after3 = talk.findIndex((f) => f.id === 3) + 1;
    await act(async () => {
      second.frames([...preamble, ...talk.slice(after3)]);
    });
    await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(false));
    expect(summarize(messages())).toEqual([
      { role: "user", parts: ["text:talk to me"] },
      {
        role: "assistant",
        status: "complete",
        parts: [
          "actor",
          "status:working",
          "status:working",
          "text:Plan: add a test",
          "artifact",
          "status:completed",
        ],
      },
    ]);
    agent.stop();
  });

  it("a run the orchestrator starts itself (no user message) lands after an unanswered interrupt", async () => {
    // ask ends in an interrupt; later the producer opens a run of its own that raises it again
    const stream = new LiveStream();
    const { agent, runtime, messages } = mountRuntime(() => sse(stream.body));
    agent.start();
    const ask = loadGolden("connect-ask");
    const own: GoldenFrame[] = [
      startedFrame("run-7"),
      { event: { type: "SUBAGENT_STARTED", subagentRunId: "sub-2", name: "plain" } },
      {
        event: {
          type: "ACTIVITY_SNAPSHOT",
          messageId: "evt-7",
          activityType: "vymalo.status",
          content: { status: "input_required", detail: "Still there?" },
          subagentRunId: "sub-2",
        },
      },
      {
        id: 7,
        event: {
          type: "SUBAGENT_FINISHED",
          subagentRunId: "sub-2",
          outcome: { type: "suspended", interruptIds: ["int-7"] },
        },
      },
      {
        id: 8,
        event: {
          type: "RUN_FINISHED",
          threadId: THREAD_ID,
          runId: "run-7",
          outcome: {
            type: "interrupt",
            interrupts: [{ id: "int-7", reason: "input_required", message: "Still there?" }],
          },
        },
      },
    ];
    await act(async () => {
      stream.frames([...ask.slice(0, 12), ...own]);
    });
    await waitFor(() => expect(messages()).toHaveLength(3));
    expect(summarize(messages()).map((m) => m.role)).toEqual(["user", "assistant", "assistant"]);
    expect(runtime().unstable_getPendingInterrupts()).toMatchObject([
      { id: "int-7", message: "Still there?" },
    ]);
    agent.stop();
  });
});

/** The frames with the runs named as the requests named them, and the message as it was sent. */
const relabel = (
  frames: GoldenFrame[],
  runs: Record<string, string>,
  messages: Record<string, string> = {},
): GoldenFrame[] => {
  let text = JSON.stringify(frames);
  for (const [from, to] of Object.entries(messages)) {
    text = text.replaceAll(JSON.stringify(from), JSON.stringify(to));
  }
  return (JSON.parse(text) as GoldenFrame[]).map((f) =>
    (f.event.type === "RUN_STARTED" || f.event.type === "RUN_FINISHED") &&
    typeof f.event.runId === "string" &&
    runs[f.event.runId]
      ? { ...f, event: { ...f.event, runId: runs[f.event.runId] } }
      : f,
  );
};

type Posted = {
  runId: string;
  messages: { id: string; role: string; content: string }[];
  forwardedProps: Record<string, unknown>;
};

/** What the orchestrator answers a POST with: the run's own RUN_STARTED and nothing more. */
const orchestrator = (connect: LiveStream) => (call: { method: string; body?: unknown }) => {
  if (call.method === "GET") return sse(connect.body);
  const post = new LiveStream();
  post.frames([startedFrame((call.body as { runId: string }).runId)]);
  return sse(post.body);
};
const postsOf = (calls: { method: string; body?: unknown }[]) =>
  calls.filter((c) => c.method === "POST").map((c) => c.body as Posted);

const SENDS = [
  { golden: "steer", mode: "steer", text: "echo you were wrong since line 1", last: 6 },
  { golden: "stop-and-send", mode: "interrupt", text: "echo do X instead", last: 9 },
] as const;

describe("a second run while one is open: what the runtime does with its own append (react-ag-ui 0.0.62, ADR 0036)", () => {
  // The first test of the send-while-working change. The runtime supersedes the run it is showing
  // (`abortActiveRun`) and dispatches RUN_CANCELLED to it: the transcript keeps every message once,
  // but the run's message ends `incomplete: cancelled`, which the turn draws as "Stopped" and the
  // run's later events never reach (it is detached). That is why `sendWhileWorking` does not use it.
  for (const { golden, mode, text, last } of SENDS) {
    it(`${golden}: no duplicate message and the first run's messages are kept, but its message is ended as cancelled`, async () => {
      const connect = new LiveStream();
      const { agent, calls, runtime, messages } = mountRuntime(orchestrator(connect));
      agent.start();
      await act(async () => {
        await runtime().thread.append("gate refactor the parser");
      });
      const run1 = postsOf(calls)[0]?.runId ?? "";
      const frames = loadGolden(golden);
      const working = framesThrough(frames, 2); // run-1: the message, the invocation, working
      await act(async () => {
        connect.frames(relabel(working, { "run-1": run1 }));
      });
      await waitFor(() => expect(messages()).toHaveLength(2));
      await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(true));

      await act(async () => {
        void runtime().thread.append(text);
      });
      await waitFor(() => expect(postsOf(calls)).toHaveLength(2));
      const sent = postsOf(calls)[1] as Posted;
      expect(sent.messages).toHaveLength(1);
      const rest = frames.slice(working.length);
      const through = rest.findIndex((f) => f.id === last) + 1;
      await act(async () => {
        connect.frames(
          relabel(
            rest.slice(0, through),
            { "run-1": run1, "run-2": sent.runId },
            { "msg-2": sent.messages[0]?.id ?? "" },
          ),
        );
      });
      await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(false));

      const summary = summarize(messages());
      expect(summary.map((m) => m.role)).toEqual(["user", "assistant", "user", "assistant"]);
      expect(summary[0]).toEqual({ role: "user", parts: ["text:gate refactor the parser"] });
      expect(summary[1]?.parts.slice(0, 2)).toEqual(["actor", "status:working"]);
      expect(summary[1]?.status).toBe("incomplete:cancelled");
      expect(summary[2]).toEqual({ role: "user", parts: [`text:${text}`] });
      expect(summary[3]?.status).toBe("complete");
      expect(mode).toBeDefined();
      agent.stop();
    });
  }
});

describe("ThreadAgent.sendWhileWorking: a message sent while the agent works (ADR 0036)", () => {
  for (const { golden, mode, text, last } of SENDS) {
    for (const opener of ["a page opened while the run was open", "a run this page started"]) {
      it(`${golden}, ${opener}: the first run ends as the log ends it, and the message comes back once, with its delivery`, async () => {
        const connect = new LiveStream();
        const { agent, calls, runtime, messages } = mountRuntime(orchestrator(connect));
        agent.start();
        const frames = loadGolden(golden);
        const working = framesThrough(frames, 2);
        let run1 = "run-1";
        if (opener.startsWith("a run")) {
          await act(async () => {
            await runtime().thread.append("gate refactor the parser");
          });
          run1 = postsOf(calls)[0]?.runId ?? "";
        }
        await act(async () => {
          connect.frames(relabel(working, { "run-1": run1 }));
        });
        await waitFor(() => expect(messages()).toHaveLength(2));
        await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(true));
        await waitFor(() => expect(agent.getSnapshot().replaying).toBe(false));

        await act(async () => {
          await agent.sendWhileWorking(text, mode);
        });
        // the runtime did nothing: it is still showing the open run, which nobody superseded
        expect(runtime().thread.getState().isRunning).toBe(true);
        const sent = postsOf(calls).at(-1) as Posted;
        expect(sent.messages).toHaveLength(1);
        expect(sent.messages[0]).toMatchObject({ role: "user", content: text });
        expect(sent.forwardedProps["vymalo.send"]).toBe(mode);

        const rest = frames.slice(working.length);
        const through = rest.findIndex((f) => f.id === last) + 1;
        await act(async () => {
          connect.frames(
            relabel(
              rest.slice(0, through),
              { "run-1": run1, "run-2": sent.runId },
              { "msg-2": sent.messages[0]?.id ?? "" },
            ),
          );
        });
        await waitFor(() => expect(agent.getSnapshot().state).toBe("done"));
        await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(false));
        await waitFor(() => expect(agent.getSnapshot().replaying).toBe(false));

        const summary = summarize(messages());
        expect(summary.map((m) => m.role)).toEqual(["user", "assistant", "user", "assistant"]);
        // the first run ended as the orchestrator ended it (the run that was open at the message,
        // success), not as the runtime's cancel
        expect(summary[1]).toMatchObject({ status: "complete" });
        expect(summary[2]).toEqual({ role: "user", parts: [`text:${text}`] });
        const users = messages().filter((m) => m.role === "user");
        expect(users.map((m) => m.metadata.custom.delivery)).toEqual([undefined, mode]);
        agent.stop();
      });
    }
  }

  it("a refused message leaves the open run alone, rejects with the problem, and starts no run", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime, messages, box } = mountRuntime((call) =>
      call.method === "POST"
        ? problem(422, "Unprocessable", "the message is too long")
        : sse(connect.body),
    );
    agent.start();
    const frames = loadGolden("steer");
    const working = framesThrough(frames, 2);
    await act(async () => {
      connect.frames(working);
    });
    await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(true));
    await waitFor(() => expect(agent.getSnapshot().replaying).toBe(false));

    let failure: unknown;
    await act(async () => {
      await agent.sendWhileWorking("too long", "steer").catch((e: unknown) => {
        failure = e;
      });
    });
    expect(failure).toBeInstanceOf(SendError);
    expect(failure).toMatchObject({ status: 422, message: "the message is too long" });
    expect(postsOf(calls)).toHaveLength(1);
    // nothing of it reached the runtime: it still shows the run, which is open
    expect(runtime().thread.getState().isRunning).toBe(true);
    expect(box.errors).toEqual([]);
    expect(summarize(messages()).map((m) => m.role)).toEqual(["user", "assistant"]);

    // and the run goes on to its end, in the same message
    await act(async () => {
      connect.frames([
        { event: { type: "SUBAGENT_FINISHED", subagentRunId: "sub-2" } },
        {
          event: {
            type: "STATE_SNAPSHOT",
            snapshot: { thread: { state: "done", target: { agentId: "plain" } } },
          },
        },
        {
          id: 3,
          event: {
            type: "RUN_FINISHED",
            threadId: THREAD_ID,
            runId: "run-1",
            outcome: { type: "success" },
          },
        },
      ]);
    });
    await waitFor(() => expect(runtime().thread.getState().isRunning).toBe(false));
    expect(summarize(messages())[1]).toMatchObject({ role: "assistant", status: "complete" });
    agent.stop();
  });

  it("a request that never got an answer rejects too, in words", async () => {
    const { agent } = mountRuntime(() => {
      throw new TypeError("Failed to fetch");
    });
    await expect(agent.sendWhileWorking("hello", "interrupt")).rejects.toMatchObject({
      name: "SendError",
    });
  });

  it("carries vymalo.send on that one run: the runtime's own runs carry none", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(orchestrator(connect));
    agent.start();
    await act(async () => {
      await runtime().thread.append("first");
    });
    await act(async () => {
      await agent.sendWhileWorking("second", "interrupt");
    });
    await act(async () => {
      await runtime().thread.append("third");
    });
    const props = postsOf(calls).map((p) => p.forwardedProps);
    expect(props).toHaveLength(3);
    expect(props[0]).not.toHaveProperty("vymalo.send");
    expect(props[1]?.["vymalo.send"]).toBe("interrupt");
    expect(props[2]).not.toHaveProperty("vymalo.send");
    // the message has an id of its own, and the run too
    const [, second] = postsOf(calls) as [Posted, Posted, Posted];
    expect(second.messages[0]?.id).toMatch(/^[0-9a-f-]{36}$/);
    expect(second.runId).toMatch(/^[0-9a-f-]{36}$/);
    agent.stop();
  });
});

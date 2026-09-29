// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { dropFailedSend } from "./failed-send";
import { type GoldenFrame, LiveStream, loadGolden, problem, sse, THREAD_ID } from "./testing";
import { mountRuntime, summarize } from "./testing-runtime";

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
    const golden = loadGolden("ask")
      .slice(0, 12)
      .map((f) =>
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
        parts: ["actor", "status:working", "status:input_required"],
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
      connect.frames(loadGolden("ask").slice(0, 12));
    });
    await waitFor(() => expect(runtime().unstable_getPendingInterrupts()).toHaveLength(1));

    let runId = "";
    await act(async () => {
      const done = runtime().unstable_submitInterruptResponses([
        { interruptId: "int-3", status: "resolved", payload: { text: "main" } },
      ]);
      await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
      runId = (calls.find((c) => c.method === "POST") as { body: { runId: string } }).body.runId;
      const rest = loadGolden("ask")
        .slice(12)
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

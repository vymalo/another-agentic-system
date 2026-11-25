// @vitest-environment jsdom
import { act, cleanup, configure } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LiveStream, sse } from "./testing";
import { mountRuntime } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

type Body = {
  runId: string;
  messages: { id: string; content: string }[];
  forwardedProps: Record<string, unknown>;
};

const accepting = (connect: LiveStream) => (call: { method: string; body?: unknown }) => {
  if (call.method === "GET") return sse(connect.body);
  const post = new LiveStream();
  post.frames([
    {
      event: {
        type: "RUN_STARTED",
        threadId: "t",
        runId: (call.body as Body).runId,
        protocolVersion: "1.0",
      },
    },
  ]);
  return sse(post.body);
};

describe("the model and the reasoning effort on the runs ThreadAgent posts (ADR 0086)", () => {
  it("posts the person's pick as vymalo.model and vymalo.reasoningEffort with the message", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect), {
      target: () => ({ agentId: "plain", release: null, model: "alias-a", effort: "high" }),
    });
    agent.start();
    await act(async () => {
      await runtime().thread.append("look around");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.model"]).toBe("alias-a");
    expect(body.forwardedProps["vymalo.reasoningEffort"]).toBe("high");
    agent.stop();
  });

  it("sends no member for an axis the person left alone", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect), {
      target: () => ({ agentId: "plain", release: null, model: "alias-a" }),
    });
    agent.start();
    await act(async () => {
      await runtime().thread.append("plain words");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.model"]).toBe("alias-a");
    expect(body.forwardedProps).not.toHaveProperty("vymalo.reasoningEffort");
    agent.stop();
  });

  it("sends neither when the person picked neither: the agent's own defaults apply", async () => {
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect), {
      target: () => ({ agentId: "plain", release: null }),
    });
    agent.start();
    await act(async () => {
      await runtime().thread.append("plain words");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps).not.toHaveProperty("vymalo.model");
    expect(body.forwardedProps).not.toHaveProperty("vymalo.reasoningEffort");
    agent.stop();
  });

  it("sendWhileWorking carries it too, the pick rides with its message", async () => {
    const connect = new LiveStream();
    const { agent, calls } = mountRuntime(accepting(connect), {
      target: () => ({ agentId: "plain", release: null, model: "alias-a", effort: "low" }),
    });
    agent.start();
    await act(async () => {
      await agent.sendWhileWorking("redo it", "steer");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.send"]).toBe("steer");
    expect(body.forwardedProps["vymalo.model"]).toBe("alias-a");
    expect(body.forwardedProps["vymalo.reasoningEffort"]).toBe("low");
    agent.stop();
  });
});

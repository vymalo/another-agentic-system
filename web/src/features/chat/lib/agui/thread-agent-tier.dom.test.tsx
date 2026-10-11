// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { SentTiers } from "@/features/tier/lib/sent";
import type { Tier } from "@/features/tier/lib/tier";
import { LiveStream, problem, sse } from "./testing";
import { mountRuntime } from "./testing-runtime";

configure({ asyncUtilTimeout: 10_000 });
afterEach(cleanup);

type Body = {
  runId: string;
  messages: { id: string; content: string }[];
  forwardedProps: Record<string, unknown>;
};

const started = (runId: string) => ({
  event: { type: "RUN_STARTED", threadId: "t", runId, protocolVersion: "1.0" },
});

/** An orchestrator that accepts every run. */
const accepting = (connect: LiveStream) => (call: { method: string; body?: unknown }) => {
  if (call.method === "GET") return sse(connect.body);
  const post = new LiveStream();
  post.frames([started((call.body as Body).runId)]);
  return sse(post.body);
};

describe("the access tier on the runs ThreadAgent posts (ADR 0061)", () => {
  it("posts the person's pick as forwardedProps[vymalo.tier] with the message, and tells the sink under the message's id", async () => {
    const tiers = new SentTiers();
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect), {
      tiers,
      target: () => ({ agentId: "plain", release: null, tier: "plan" as Tier }),
    });
    agent.start();
    await act(async () => {
      await runtime().thread.append("look around");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.tier"]).toBe("plan");
    expect(tiers.of(body.messages[0]?.id ?? "")).toBe("plan");
    agent.stop();
  });

  it("sends no member when the person picked nothing: the agent works under its default", async () => {
    const tiers = new SentTiers();
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect), { tiers });
    agent.start();
    await act(async () => {
      await runtime().thread.append("plain words");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps).not.toHaveProperty("vymalo.tier");
    expect(tiers.of(body.messages[0]?.id ?? "")).toBeUndefined();
    agent.stop();
  });

  it("reads the pick at each send: a later message goes under the pick it was sent with", async () => {
    let tier: Tier | undefined = "edits";
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect), {
      target: () => ({ agentId: "plain", release: null, tier }),
    });
    agent.start();
    await act(async () => {
      await runtime().thread.append("first");
    });
    tier = "bypass";
    await act(async () => {
      await runtime().thread.append("second");
    });
    const posts = calls.filter((c) => c.method === "POST").map((c) => c.body as Body);
    expect(posts.map((b) => b.forwardedProps["vymalo.tier"])).toEqual(["edits", "bypass"]);
    agent.stop();
  });

  it("sendWhileWorking posts it too, beside vymalo.send", async () => {
    const connect = new LiveStream();
    const { agent, calls } = mountRuntime(accepting(connect), {
      target: () => ({ agentId: "plain", release: null, tier: "auto" as Tier }),
    });
    agent.start();
    await act(async () => {
      await agent.sendWhileWorking("redo it", "steer");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.send"]).toBe("steer");
    expect(body.forwardedProps["vymalo.tier"]).toBe("auto");
    agent.stop();
  });

  it("forgets the tier of a send the orchestrator refused (422): the message is not in the transcript", async () => {
    const tiers = new SentTiers();
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(
      (call) =>
        call.method === "GET"
          ? sse(connect.body)
          : problem(422, "Unprocessable", "this agent does not take the tier 'bypass'"),
      { tiers, target: () => ({ agentId: "plain", release: null, tier: "bypass" as Tier }) },
    );
    agent.start();
    await act(async () => {
      void runtime().thread.append("go wild");
    });
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.tier"]).toBe("bypass");
    await waitFor(() => expect(tiers.of(body.messages[0]?.id ?? "")).toBeUndefined());
    agent.stop();
  });

  it("reads the tier a message carried from the log (a reload, another tab) onto the transcript's message", async () => {
    const connect = new LiveStream();
    const { agent, runtime, messages } = mountRuntime(() => sse(connect.body));
    agent.start();
    await act(async () => {
      connect.frames([
        { event: { type: "RUN_STARTED", threadId: "t", runId: "r1", protocolVersion: "1.0" } },
        {
          event: {
            type: "TEXT_MESSAGE_START",
            messageId: "m-1",
            role: "user",
            metadata: { "vymalo.tier": "bypass" },
          },
        },
        { event: { type: "TEXT_MESSAGE_CONTENT", messageId: "m-1", delta: "go" } },
        { id: 1, event: { type: "TEXT_MESSAGE_END", messageId: "m-1" } },
      ]);
    });
    await waitFor(() => expect(messages().some((m) => m.role === "user")).toBe(true));
    const user = runtime()
      .thread.getState()
      .messages.find((m) => m.role === "user");
    expect(user?.metadata.custom.tier).toBe("bypass");
    agent.stop();
  });

  it("ignores a tier in the log that is not one of the four words", async () => {
    const connect = new LiveStream();
    const { agent, runtime, messages } = mountRuntime(() => sse(connect.body));
    agent.start();
    await act(async () => {
      connect.frames([
        { event: { type: "RUN_STARTED", threadId: "t", runId: "r1", protocolVersion: "1.0" } },
        {
          event: {
            type: "TEXT_MESSAGE_START",
            messageId: "m-1",
            role: "user",
            metadata: { "vymalo.tier": "root" },
          },
        },
        { event: { type: "TEXT_MESSAGE_CONTENT", messageId: "m-1", delta: "go" } },
        { id: 1, event: { type: "TEXT_MESSAGE_END", messageId: "m-1" } },
      ]);
    });
    await waitFor(() => expect(messages().some((m) => m.role === "user")).toBe(true));
    const user = runtime()
      .thread.getState()
      .messages.find((m) => m.role === "user");
    expect(user?.metadata.custom).not.toHaveProperty("tier");
    agent.stop();
  });
});

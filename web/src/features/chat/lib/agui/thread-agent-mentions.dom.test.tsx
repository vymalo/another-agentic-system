// @vitest-environment jsdom
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { MentionsStore } from "@/features/mentions/lib/store";
import { LiveStream, loadGolden, problem, sse } from "./testing";
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

describe("mentions on the runs ThreadAgent posts (ADR 0026)", () => {
  it("a message is posted with the mentions the store holds for its text, and the bubble can find them by the message's id", async () => {
    const store = new MentionsStore();
    const text = "😄 @coder ask";
    store.set(text, [{ agentId: "coder", label: "@coder", start: 3, end: 9 }]);
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(
      (call) => {
        if (call.method === "GET") return sse(connect.body);
        const post = new LiveStream();
        post.frames([started((call.body as Body).runId)]);
        return sse(post.body);
      },
      { mentions: store },
    );
    agent.start();
    await act(async () => {
      await runtime().thread.append(text);
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.mentions"]).toEqual([
      { agentId: "coder", label: "@coder", start: 3, end: 9 },
    ]);
    // the offsets are into the text as it went out
    expect(body.messages[0]?.content.slice(3, 9)).toBe("@coder");
    // the run's own message is the runtime's, and its id is the one posted: the bubble finds the chips by it
    const mine = runtime()
      .thread.getState()
      .messages.find((m) => m.role === "user");
    expect(mine?.id).toBe(body.messages[0]?.id);
    expect(store.sentOf(body.messages[0]?.id ?? "")).toEqual([
      { agentId: "coder", label: "@coder", start: 3, end: 9 },
    ]);
  });

  it("a message that mentions nobody has no member, and a refused one is forgotten", async () => {
    const store = new MentionsStore();
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(
      (call) =>
        call.method === "GET" ? sse(connect.body) : problem(422, "Unprocessable", "unknown agent"),
      { mentions: store },
    );
    agent.start();
    await act(async () => {
      void runtime().thread.append("plain words");
    });
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps).not.toHaveProperty("vymalo.mentions");

    store.set("@coder x", [{ agentId: "coder", label: "@coder", start: 0, end: 6 }]);
    await act(async () => {
      void runtime().thread.append("@coder x");
    });
    await waitFor(() => expect(calls.filter((c) => c.method === "POST")).toHaveLength(2));
    const second = calls.filter((c) => c.method === "POST")[1]?.body as Body;
    expect(second.forwardedProps["vymalo.mentions"]).toHaveLength(1);
    await waitFor(() => expect(store.sentOf(second.messages[0]?.id ?? "")).toBeUndefined());
  });

  it("sendWhileWorking posts the mentions too, with vymalo.send", async () => {
    const store = new MentionsStore();
    store.set("redo @coder", [{ agentId: "coder", label: "@coder", start: 5, end: 11 }]);
    const connect = new LiveStream();
    const { agent, calls } = mountRuntime(
      (call) => {
        if (call.method === "GET") return sse(connect.body);
        const post = new LiveStream();
        post.frames([started((call.body as Body).runId)]);
        return sse(post.body);
      },
      { mentions: store },
    );
    agent.start();
    await act(async () => {
      await agent.sendWhileWorking("redo @coder", "interrupt");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.send"]).toBe("interrupt");
    expect(body.forwardedProps["vymalo.mentions"]).toEqual([
      { agentId: "coder", label: "@coder", start: 5, end: 11 },
    ]);
  });

  it("the golden's user message comes back with its mentions on the transcript's message, the offsets of an emoji intact", async () => {
    const connect = new LiveStream();
    const { agent, runtime, messages } = mountRuntime(() => sse(connect.body));
    agent.start();
    await act(async () => {
      connect.frames(loadGolden("mentions"));
    });
    await waitFor(() => expect(messages().length).toBeGreaterThan(1));
    const user = runtime()
      .thread.getState()
      .messages.find((m) => m.role === "user");
    const text = user?.content.flatMap((p) => (p.type === "text" ? [p.text] : [])).join("") ?? "";
    expect(text).toBe("😄 @coder echo the build, please");
    const mentions = user?.metadata.custom.mentions as {
      label: string;
      start: number;
      end: number;
    }[];
    expect(mentions).toEqual([{ agentId: "coder", label: "@coder", start: 3, end: 9 }]);
    expect(text.slice(mentions[0]?.start, mentions[0]?.end)).toBe("@coder");
    agent.stop();
  });
});

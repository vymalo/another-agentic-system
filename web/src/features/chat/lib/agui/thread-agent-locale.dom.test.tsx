// @vitest-environment jsdom
import type { RunAgentInput } from "@ag-ui/client";
import { act, cleanup, configure, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { LiveStream, sse, THREAD_ID } from "./testing";
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

/** The page's tag, as a browser holds it: jsdom's `<html>` names no language until a test says one. */
const drawIn = (lang: string) => {
  document.documentElement.lang = lang;
};

afterEach(() => {
  document.documentElement.lang = "";
});

/*
 * What the page puts on the wire for `locale/v1` (ADR 0085, docs/api/locale-v1.md): the tag of the
 * page the person reads (`document.documentElement.lang`) rides the run that carries a message, read
 * at send time, and only a message carries it — an action is a gesture on a surface, not the words of
 * a screen. `e2e/locale.spec.ts` proves the same in a real browser through a production build; this
 * runs without a build, so it pins the wiring and the "message only" rule where a change to
 * `ThreadAgent` is felt first.
 */
describe("the interface language on the runs ThreadAgent posts (locale/v1, ADR 0085)", () => {
  it("posts the page's tag as forwardedProps[vymalo.locale] with the message", async () => {
    drawIn("fr-FR");
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect));
    agent.start();
    await act(async () => {
      await runtime().thread.append("bonjour");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.forwardedProps["vymalo.locale"]).toBe("fr-FR");
    agent.stop();
  });

  it("reads the tag at each send: a page that changed its language sends the new one", async () => {
    drawIn("en-GB");
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect));
    agent.start();
    await act(async () => {
      await runtime().thread.append("first");
    });
    drawIn("gsw-CH");
    await act(async () => {
      await runtime().thread.append("second");
    });
    const posts = calls.filter((c) => c.method === "POST").map((c) => c.body as Body);
    expect(posts.map((b) => b.forwardedProps["vymalo.locale"])).toEqual(["en-GB", "gsw-CH"]);
    agent.stop();
  });

  it("sends no member when the page's lang is not a tag: a hint never holds a message back", async () => {
    drawIn("en_CM");
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect));
    agent.start();
    await act(async () => {
      await runtime().thread.append("plain words");
    });
    const body = calls.find((c) => c.method === "POST")?.body as Body;
    expect(body.messages).toHaveLength(1);
    expect(body.forwardedProps).not.toHaveProperty("vymalo.locale");
    agent.stop();
  });

  it("sends no member on a run that carries no message: an action is not a screen's words", async () => {
    drawIn("fr-FR");
    const connect = new LiveStream();
    const { agent, calls, runtime } = mountRuntime(accepting(connect));
    agent.start();
    await act(async () => {
      await runtime().thread.append("bonjour");
    });
    const action: RunAgentInput = {
      threadId: THREAD_ID,
      runId: "run-action",
      messages: [],
      state: {},
      tools: [],
      context: [],
      forwardedProps: { a2uiAction: { userAction: { name: "go", surfaceId: "s1" } } },
    };
    await act(async () => {
      agent.run(action).subscribe({ error: () => {} });
      await waitFor(() => expect(calls.filter((c) => c.method === "POST")).toHaveLength(2));
    });
    const posts = calls.filter((c) => c.method === "POST").map((c) => c.body as Body);
    // the message carried the tag; the action, on the same page, carried none
    expect(posts[0]?.forwardedProps["vymalo.locale"]).toBe("fr-FR");
    expect(posts[1]?.messages).toEqual([]);
    expect(posts[1]?.forwardedProps).not.toHaveProperty("vymalo.locale");
    agent.stop();
  });
});

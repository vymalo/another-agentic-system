// @vitest-environment jsdom
import { act, cleanup, configure, fireEvent, screen, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { Thread } from "@/components/assistant-ui/elements/thread.aui";
import { TooltipProvider } from "@/components/ui/tooltip";
import { column, event, labelled, surface } from "@/features/chat/lib/a2ui/testing";
import { type Call, LiveStream, sse, THREAD_ID } from "@/features/chat/lib/agui/testing";
import { mountRuntime } from "@/features/chat/lib/agui/testing-runtime";
import type { ThreadState } from "@/lib/api/types";
import { DataUIs } from "../data-uis";
import { SurfaceHostProvider } from "./surface-host";
import { resetSeq, stubLayout, surfaceRun } from "./testing";

configure({ asyncUtilTimeout: 10_000 });
beforeAll(stubLayout);
beforeEach(resetSeq);
afterEach(cleanup);

/**
 * The app's side of a surface, with the real runtime and a fake orchestrator: what a click puts on
 * the wire, and that nothing else does. The chat shell mounts this provider (chat-shell.tsx).
 */

const started = (runId: string) =>
  `data: ${JSON.stringify({ type: "RUN_STARTED", threadId: THREAD_ID, runId, protocolVersion: "1.0" })}\n\n`;

function mount(state: ThreadState | undefined, refuse?: { status: number; detail: string }) {
  const connect = new LiveStream();
  const rejected = vi.fn();
  const composerRef = createRef<HTMLTextAreaElement>();
  const posts: Call[] = [];
  const mounted = mountRuntime(
    (call) => {
      if (call.method === "GET") return sse(connect.body);
      posts.push(call);
      if (refuse) {
        return new Response(
          JSON.stringify({ title: "Refused", status: refuse.status, detail: refuse.detail }),
          {
            status: refuse.status,
            headers: { "content-type": "application/problem+json" },
          },
        );
      }
      const post = new LiveStream();
      post.write(started((call.body as { runId: string }).runId));
      return sse(post.body);
    },
    {},
    (agent) => (
      <TooltipProvider>
        <SurfaceHostProvider
          agent={agent}
          state={state}
          composerRef={composerRef}
          onRejected={rejected}
        >
          <DataUIs />
          <Thread loading={false} empty={false} />
        </SurfaceHostProvider>
      </TooltipProvider>
    ),
  );
  mounted.agent.start();
  return { ...mounted, connect, rejected, composerRef, posts };
}

const ops = surface(
  [column("root", ["go", "go_label"]), ...labelled("go", "Go", event("go", { choice: "a" }))],
  {},
);

async function feed(m: ReturnType<typeof mount>, end: "interrupt" | "error") {
  const frames = surfaceRun([ops], { end });
  await act(async () => {
    m.connect.frames(frames);
  });
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(frames.at(-1)?.id));
  await screen.findByRole("button", { name: "Go" });
}

describe("what a click of a surface puts on the wire", () => {
  it("nothing is posted by rendering the surface, however long it is open", async () => {
    const m = mount("blocked");
    await feed(m, "interrupt");
    await new Promise((r) => setTimeout(r, 200));
    expect(m.posts).toHaveLength(0);
    m.agent.stop();
  });

  it("with the agent's question open: one run, the action, no message and no resume", async () => {
    const m = mount("blocked");
    await feed(m, "interrupt");
    expect(m.runtime().unstable_getPendingInterrupts()).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    await waitFor(() => expect(m.posts).toHaveLength(1));
    const body = m.posts[0]?.body as Record<string, unknown>;
    expect(m.posts[0]?.path).toBe("/agui/agents/plain");
    expect(body.messages).toEqual([]);
    expect(body.resume).toBeUndefined();
    expect(body.threadId).toBe(THREAD_ID);
    expect(typeof body.runId).toBe("string");
    expect(body.forwardedProps).toMatchObject({
      a2uiAction: {
        userAction: {
          name: "go",
          surfaceId: "s1",
          sourceComponentId: "go",
          context: { choice: "a" },
        },
      },
    });
    // the timestamp is the runtime's; the orchestrator replaces it with the log's own time
    m.agent.stop();
  });

  it("with no question open (blocked another way): the runtime's own sendA2uiAction, the same request", async () => {
    const m = mount("blocked");
    await feed(m, "error");
    expect(m.runtime().unstable_getPendingInterrupts()).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    await waitFor(() => expect(m.posts).toHaveLength(1));
    const body = m.posts[0]?.body as Record<string, unknown>;
    expect(body.messages).toEqual([]);
    expect(body.resume).toBeUndefined();
    expect(body.forwardedProps).toMatchObject({
      a2uiAction: { userAction: { name: "go", surfaceId: "s1", context: { choice: "a" } } },
    });
    m.agent.stop();
  });

  it("a second click before the thread moves on is not a second run", async () => {
    const m = mount("blocked");
    await feed(m, "interrupt");
    const goButton = screen.getByRole("button", { name: "Go" });
    fireEvent.click(goButton);
    fireEvent.click(goButton);
    fireEvent.click(goButton);
    await waitFor(() => expect(m.posts.length).toBeGreaterThan(0));
    await new Promise((r) => setTimeout(r, 100));
    expect(m.posts).toHaveLength(1);
    m.agent.stop();
  });

  it("a thread that is not blocked takes no action: the button is off and a click posts nothing", async () => {
    const m = mount("working");
    await feed(m, "interrupt");
    const goButton = screen.getByRole("button", { name: "Go" }) as HTMLButtonElement;
    expect(goButton.disabled).toBe(true);
    fireEvent.click(goButton);
    await new Promise((r) => setTimeout(r, 100));
    expect(m.posts).toHaveLength(0);
    m.agent.stop();
  });

  it("a refused action is reported, is not a message to take back, and can be tried again", async () => {
    const m = mount("blocked", { status: 409, detail: "a run is already open on this thread" });
    await feed(m, "interrupt");
    const before = m.messages().length;
    fireEvent.click(screen.getByRole("button", { name: "Go" }));
    await waitFor(() => expect(m.posts).toHaveLength(1));
    await waitFor(() => expect(m.agent.getSnapshot().sendFailures).toBe(1));
    expect(m.agent.takeSendError()).toMatchObject({ status: 409, action: true });
    // the transcript is as it was: the surface, the question; no message was dropped
    expect(m.messages().length).toBeGreaterThanOrEqual(before - 1);
    expect(m.messages()[0]?.role).toBe("user");
    // the button works again
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Go" }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
    m.agent.stop();
  });
});

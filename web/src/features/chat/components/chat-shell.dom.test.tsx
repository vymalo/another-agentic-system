// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import {
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

const push = vi.fn();
const router = { push }; // stable, like the App Router's
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

// The app in jsdom against the mock orchestrator: the shell, the runtime, the agent and LiveRuns.
let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the app made: `METHOD path status`. */
let calls: string[] = [];
/** Answer this request (by `METHOD path`) with a problem instead of asking the mock. */
let failing: { key: string; status: number; detail: string } | undefined;
/** Hand the app this request's response (by `METHOD path`) only once `until` resolves. */
let holding: { key: string; until: Promise<void> } | undefined;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 10_000 }); // the app and a mock server share the cores with other suites
  // jsdom has no layout: the viewport only needs these to exist
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};

  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  // the app calls relative URLs (the page's own origin); Node's Request wants absolute ones
  const abs = (input: unknown) =>
    typeof input === "string" && input.startsWith("/") ? base + input : input;
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(abs(input) as RequestInfo, init);
    }
  } as typeof Request;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : undefined;
    const url = new URL(request ? request.url : String(abs(String(input))));
    const method = request?.method ?? init?.method ?? "GET";
    const key = `${method} ${url.pathname}`;
    if (failing && key === failing.key) {
      calls.push(`${key} ${failing.status}`);
      return new Response(
        JSON.stringify({ title: "Failure", status: failing.status, detail: failing.detail }),
        { status: failing.status, headers: { "content-type": "application/problem+json" } },
      );
    }
    const res = await realFetch(abs(input) as RequestInfo, init);
    calls.push(`${key} ${res.status}`);
    if (holding && key === holding.key) await holding.until;
    return res;
  }) as typeof fetch;
  // the API client binds fetch and Request when it is created: import the app after the patch
  ({ ChatShell } = await import("./chat-shell"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  calls = [];
  failing = undefined;
  holding = undefined;
  push.mockClear();
});
afterEach(cleanup);

const shell = (threadId: string | null) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread made the way any AG-UI client makes one; the promise ends with the run (or at RUN_STARTED). */
async function makeThread(text: string, agent = "coder", untilStarted = false): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    }),
  });
  if (untilStarted) await res.body?.cancel();
  else await res.text();
  return threadId;
}

const log = () => screen.getByRole("log", { name: "Conversation" });
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });

describe("ChatShell over AG-UI", () => {
  it("the new-thread page offers the agents and, for the coder, its releases", async () => {
    shell(null);
    await screen.findByLabelText("Agent");
    expect(screen.getByLabelText("Release")).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Agent"), { target: { value: "reviewer" } });
    await waitFor(() => expect(screen.queryByLabelText("Release")).toBeNull());
  });

  it("a first message mints a UUIDv7, posts it, and goes to the thread", async () => {
    shell(null);
    await screen.findByLabelText("Agent");
    await waitFor(() =>
      expect((screen.getByLabelText("Release") as HTMLSelectElement).value).toBe("production"),
    );
    fireEvent.change(screen.getByLabelText("Release"), { target: { value: "staging" } });
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "echo hello" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(push).toHaveBeenCalledTimes(1));
    expect(push.mock.calls[0]?.[0]).toMatch(
      /^\/threads\/[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/coder 200",
    ]);
    // nothing of the legacy interaction API
    expect(calls.some((c) => c.startsWith("POST /api/threads"))).toBe(false);
  });

  it("a rejected first message shows the problem and gives the text back once", async () => {
    shell(null);
    await screen.findByLabelText("Agent");
    failing = {
      key: "POST /agui/agents/coder",
      status: 400,
      detail: "text must be 1 to 100000 characters",
    };
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "keep me" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByText("text must be 1 to 100000 characters");
    await waitFor(() =>
      expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("keep me"),
    );
    await new Promise((r) => setTimeout(r, 100)); // and it stays once
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("keep me");
    expect(push).not.toHaveBeenCalled();
  });

  it("a finished thread is replayed from the connect stream: transcript, PR, Done, no composer", async () => {
    const id = await makeThread("Implement the thing");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    await waitFor(() => transcript.getByText("Implement the thing"));
    await waitFor(() => transcript.getByText("Completed"));
    expect(transcript.getByText("Working")).toBeTruthy();
    const pr = transcript.getByRole("link", { name: "Pull request acme/demo#1" });
    expect(pr.getAttribute("href")).toBe("https://github.com/acme/demo/pull/1");
    expect(transcript.getAllByText("coder · coder-r47").length).toBeGreaterThan(0);
    expect((screen.getByLabelText("Message") as HTMLTextAreaElement).disabled).toBe(true);
    expect(screen.getByText(/This thread is done\./)).toBeTruthy();
    // replay: one connect, and the finished thread needs no more than that
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.includes("/connect"))).toEqual([
      `GET /agui/threads/${id}/connect 200`,
    ]);
  });

  it("a blocked thread offers the question, and the answer is a resume, not a message", async () => {
    const id = await makeThread("ask pick a branch");
    shell(id);
    await screen.findByText("Waiting for your answer.");
    // the question is the runtime's interrupt, which arrives a moment after the thread state does
    await screen.findByText("Which branch?");
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "main" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    await waitFor(() => expect(transcript.getAllByText("answered: main")).toHaveLength(1));
    expect(transcript.getAllByText("main")).toHaveLength(1);
    // the mock answers 422 to a message together with a resume: one POST, and it was accepted
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/coder 200",
    ]);
  });

  it("an answer whose run the connect stream finishes before the POST answers still shows its reply", async () => {
    // The orchestrator's order: the run happens (and the connect stream shows it, to Done) while
    // the POST's own RUN_STARTED is still on its way. The finished thread pauses the connect
    // stream; that pause must not take the send, whose events are already here, with it.
    const id = await makeThread("ask pick a branch");
    shell(id);
    await screen.findByText("Which branch?");
    let release = () => {};
    holding = {
      key: "POST /agui/agents/coder",
      until: new Promise<void>((r) => {
        release = r;
      }),
    };
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "main" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await new Promise((r) => setTimeout(r, 50)); // the paused stream has let go
    release();
    const transcript = within(log());
    await waitFor(() => expect(transcript.getAllByText("answered: main")).toHaveLength(1));
    expect(transcript.getAllByText("main")).toHaveLength(1);
  });

  it("a refused send shows the problem, keeps the text and leaves no message behind", async () => {
    const id = await makeThread("ask pick a branch");
    shell(id);
    await screen.findByText("Waiting for your answer.");
    await screen.findByText("Which branch?");
    failing = {
      key: "POST /agui/agents/coder",
      status: 409,
      detail: "the thread is finished (Done); start a new thread",
    };
    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "late" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    await screen.findByText("the thread is finished (Done); start a new thread");
    await waitFor(() =>
      expect((screen.getByLabelText("Message") as HTMLTextAreaElement).value).toBe("late"),
    );
    expect(within(log()).queryByText("late")).toBeNull();
  });

  it("an A2UI surface is drawn; its button sends the action (no message, no resume) and the scenario goes on", async () => {
    const id = await makeThread("ui pick one", "reviewer");
    shell(id);
    const ui = await screen.findByRole("region", { name: "Interface from reviewer" });
    await waitFor(() => expect(stateBadge().textContent).toBe("Waiting for you"));
    expect(within(ui).getByText("Pick one")).toBeTruthy();
    const goButton = await within(ui).findByRole("button", { name: "Go" });
    await waitFor(() => expect((goButton as HTMLButtonElement).disabled).toBe(false));
    // opening the thread sent nothing but the connect
    expect(calls.filter((c) => c.startsWith("POST"))).toEqual([]);
    fireEvent.click(goButton);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const transcript = within(log());
    await waitFor(() => expect(transcript.getAllByText("answered: ui-action go")).toHaveLength(1));
    expect(transcript.getByText("Chose")).toBeTruthy();
    // one POST, the action; the surface is still there, and its button is off now
    expect(calls.filter((c) => c.startsWith("POST /agui/agents"))).toEqual([
      "POST /agui/agents/reviewer 200",
    ]);
    expect(transcript.getAllByText("ui pick one")).toHaveLength(1);
    expect(
      (
        within(transcript.getByRole("region", { name: /^Interface from/ })).getByRole("button", {
          name: "Go",
        }) as HTMLButtonElement
      ).disabled,
    ).toBe(true);
    expect(screen.getByText(/This thread is finished/)).toBeTruthy();
  });

  it("a finished thread shows its surface read-only, and the button says why", async () => {
    const id = await makeThread("ui pick one", "reviewer");
    // answer it from outside, as another tab would
    const res = await realFetch(`${base}/agui/agents/reviewer`, {
      method: "POST",
      headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
      body: JSON.stringify({
        threadId: id,
        runId: "run-2",
        messages: [],
        forwardedProps: {
          a2uiAction: {
            userAction: { name: "go", surfaceId: "s1", sourceComponentId: "go", context: {} },
          },
        },
      }),
    });
    expect(res.status).toBe(200);
    await res.text();
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const ui = await screen.findByRole("region", { name: "Interface from reviewer" });
    expect((within(ui).getByRole("button", { name: "Go" }) as HTMLButtonElement).disabled).toBe(
      true,
    );
    expect(within(ui).getByText(/This thread is finished/)).toBeTruthy();
    expect(calls.filter((c) => c.startsWith("POST"))).toEqual([]);
  });

  it("a refused action shows the problem, keeps the transcript, and the button works again", async () => {
    const id = await makeThread("ui pick one", "reviewer");
    shell(id);
    const goButton = await screen.findByRole("button", { name: "Go" });
    await waitFor(() => expect((goButton as HTMLButtonElement).disabled).toBe(false));
    failing = {
      key: "POST /agui/agents/reviewer",
      status: 409,
      detail: "a run is already open on this thread; wait for it to finish",
    };
    fireEvent.click(goButton);
    await screen.findByText("a run is already open on this thread; wait for it to finish");
    // the message before the action is still there: an action carries none, so none was taken back
    expect(within(log()).getByText("ui pick one")).toBeTruthy();
    await waitFor(() =>
      expect((screen.getByRole("button", { name: "Go" }) as HTMLButtonElement).disabled).toBe(
        false,
      ),
    );
  });

  it("Cancel asks the orchestrator; the run ends cancelled and the runtime shows it", async () => {
    const id = await makeThread("slow work", "reviewer", true);
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Working"));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(stateBadge().textContent).toBe("Cancelled"));
    expect(calls).toContain(`POST /api/threads/${id}/cancel 202`);
    await waitFor(() => expect(within(log()).getAllByText(/Cancelled/).length).toBeGreaterThan(0));
    expect(screen.queryByRole("button", { name: "Cancel" })).toBeNull();
  });

  it("an unknown thread is not found, and its connect is not retried", async () => {
    shell("00000000-0000-7000-8000-00000000dead");
    await screen.findByText(/Thread not found/);
    await new Promise((r) => setTimeout(r, 50));
    expect(calls.filter((c) => c.includes("/connect"))).toEqual([
      "GET /agui/threads/00000000-0000-7000-8000-00000000dead/connect 404",
    ]);
  });
});

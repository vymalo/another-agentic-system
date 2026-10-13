// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { ApiThread } from "@/lib/api/types";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

/*
 * Deleting a thread (ADR 0043), against the mock orchestrator: the dialog says what is erased, a
 * thread that works is stopped first ("Stop and delete": cancel, wait for the thread to end, delete),
 * and every refusal is in words. The mock answers as the orchestrator does (409 `thread_active`, 404
 * the second time, 403 without `thread.delete`), so the flow runs for real; only what the mock cannot
 * be told to do (an agent that never stops) is faked at the fetch.
 */

let DeleteThreadDialog: typeof import("./delete-thread-dialog").DeleteThreadDialog;

const server = createMockServer({ stepMs: 5, keepaliveMs: 50 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
let cookie = "";
/** Every call the dialog made: `METHOD path status`. */
let calls: string[] = [];
/** Requests answered by the test instead of the mock: `METHOD path` to the status it gets. */
let faked: Record<string, number> = {};

beforeAll(async () => {
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
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
    const key = `${method} ${url.pathname.replace(/[0-9a-f-]{36}/, ":id")}`;
    const status = faked[key];
    if (status !== undefined) {
      calls.push(`${key} ${status}`);
      return new Response(status === 202 || status === 204 ? null : "{}", {
        status,
        headers: { "content-type": "application/problem+json" },
      });
    }
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    if (cookie) carried.headers.set("cookie", cookie);
    const res = await realFetch(carried);
    calls.push(`${key} ${res.status}`);
    return res;
  }) as typeof fetch;
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  ({ DeleteThreadDialog } = await import("./delete-thread-dialog"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

let people = 0;
async function aPerson(me: "user" | "read-only" = "user") {
  const session = `dialog-${++people}`;
  const user = `dialog${people}@example.com`;
  await realFetch(`${base}/__mock/config?me=${me}&user=${user}&session=${session}`, {
    method: "POST",
  });
  cookie = `mock-registry=${session}`;
}

afterEach(async () => {
  cleanup();
  vi.restoreAllMocks();
  calls = [];
  faked = {};
  cookie = "";
  await realFetch(`${base}/__mock/reset`, { method: "POST" });
});

/** A thread of the current person: run to its end, or (`slow`) still working. */
async function threadOf(text: string, agent = "reviewer"): Promise<ApiThread> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/${agent}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream", cookie },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
    }),
  });
  expect(res.status).toBe(200);
  if (text.startsWith("slow")) {
    // it never ends by itself: read up to RUN_STARTED and let go
    const reader = res.body?.getReader();
    const decoder = new TextDecoder();
    let seen = "";
    while (reader && !seen.includes("RUN_STARTED")) {
      const { value, done } = await reader.read();
      if (done) break;
      seen += decoder.decode(value);
    }
    await reader?.cancel();
  } else {
    await res.text();
  }
  const got = await realFetch(`${base}/api/threads/${threadId}`, { headers: { cookie } });
  return (await got.json()) as ApiThread;
}

const stateOf = async (id: string) =>
  (
    (await (
      await realFetch(`${base}/api/threads/${id}`, { headers: { cookie } })
    ).json()) as ApiThread
  ).state;
const exists = async (id: string) =>
  (await realFetch(`${base}/api/threads/${id}`, { headers: { cookie } })).status === 200;

const FAST = { pollMs: 10, giveUpMs: 3000 };

function open(thread: ApiThread, extra: { timing?: typeof FAST; onClose?: () => void } = {}) {
  const onErased = vi.fn();
  const onClose = extra.onClose ?? vi.fn();
  render(
    <DeleteThreadDialog
      thread={thread}
      onErased={onErased}
      onClose={onClose}
      timing={extra.timing ?? FAST}
    />,
  );
  return { onErased, onClose };
}

const dialog = () => screen.getByRole("alertdialog");

describe("the delete dialog says plainly what is erased", () => {
  it("names the chat and lists what goes with it, what stays, and that it cannot be undone", async () => {
    await aPerson();
    const thread = await threadOf("echo the words");
    open(thread);
    expect(dialog().getAttribute("aria-labelledby")).toBeTruthy();
    expect(screen.getByRole("alertdialog", { name: "Delete “echo the words”?" })).toBeTruthy();
    const text = dialog().textContent ?? "";
    expect(text).toMatch(/cannot be undone/i);
    expect(text).toMatch(/chats made from it by editing a message/i);
    expect(text).toMatch(/files/i);
    expect(text).toMatch(/forks .* kept/i);
    // not shared: no link to speak of
    expect(text).not.toMatch(/share link/i);
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop and delete" })).toBeNull();
  });

  it("speaks of the share link only when the chat has one", async () => {
    await aPerson();
    const thread = await threadOf("echo shared");
    open({ ...thread, share: { visibility: "public", effective: "internal" } });
    expect(dialog().textContent).toMatch(/share link.*stops working/i);
  });
});

describe("deleting a chat that is not working", () => {
  it("calls deleteThread once, says it was erased, and the chat is gone", async () => {
    await aPerson();
    const thread = await threadOf("echo bye");
    const { onErased } = open(thread);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(onErased).toHaveBeenCalledWith(thread.id));
    expect(calls.filter((c) => c.startsWith("DELETE"))).toEqual(["DELETE /api/threads/:id 204"]);
    expect(await exists(thread.id)).toBe(false);
  });

  it("disables the button while it deletes, so one click is one call", async () => {
    await aPerson();
    const thread = await threadOf("echo twice");
    const { onErased } = open(thread);
    const button = screen.getByRole("button", { name: "Delete" });
    fireEvent.click(button);
    fireEvent.click(button);
    await waitFor(() => expect(onErased).toHaveBeenCalledTimes(1));
    expect(calls.filter((c) => c.startsWith("DELETE"))).toHaveLength(1);
  });

  it("treats a 404 as already gone: the list drops it and no error is shown", async () => {
    await aPerson();
    const thread = await threadOf("echo gone");
    await realFetch(`${base}/api/threads/${thread.id}`, { method: "DELETE", headers: { cookie } });
    const { onErased } = open(thread);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(onErased).toHaveBeenCalledWith(thread.id));
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says in words that the roles do not let the person delete (403), and stays open to try again", async () => {
    await aPerson("read-only");
    const viewer = cookie;
    // a thread the viewer owns: made by the default person and handed over by the test
    cookie = "";
    const made = await threadOf("echo held");
    await realFetch(`${base}/__mock/owner?thread=${made.id}&owner=dialog${people}@example.com`, {
      method: "POST",
    });
    cookie = viewer;
    const { onErased } = open(made);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/your roles do not let you delete chats/i);
    expect(onErased).not.toHaveBeenCalled();
    expect((screen.getByRole("button", { name: "Delete" }) as HTMLButtonElement).disabled).toBe(
      false,
    );
    expect(await exists(made.id)).toBe(true);
  });

  it("says another failure in the server's words and keeps the chat", async () => {
    await aPerson();
    const thread = await threadOf("echo flaky");
    faked["DELETE /api/threads/:id"] = 503;
    const { onErased } = open(thread);
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/could not delete/i);
    expect(onErased).not.toHaveBeenCalled();
    // and a second try goes to the real thing
    delete faked["DELETE /api/threads/:id"];
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(onErased).toHaveBeenCalledWith(thread.id));
  });

  it("Cancel closes it and deletes nothing", async () => {
    await aPerson();
    const thread = await threadOf("echo keep");
    const { onClose, onErased } = open(thread);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalled();
    expect(onErased).not.toHaveBeenCalled();
    expect(calls.some((c) => c.startsWith("DELETE"))).toBe(false);
    expect(await exists(thread.id)).toBe(true);
  });
});

describe("deleting a chat that works: Stop and delete", () => {
  it("offers Stop and delete in place of Delete when the list says it works, and says why", async () => {
    await aPerson();
    const thread = await threadOf("slow task");
    // queued or working: both are a run that is open
    expect(["queued", "working"]).toContain(thread.state);
    open(thread);
    expect(screen.getByRole("button", { name: "Stop and delete" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Delete$/ })).toBeNull();
    expect(dialog().textContent).toMatch(/still working/i);
    // nothing is sent until the person chooses
    expect(calls.filter((c) => !c.startsWith("GET"))).toEqual([]);
  });

  it("stops it (cancel), waits for it to end, then deletes it", async () => {
    await aPerson();
    const thread = await threadOf("slow task");
    const { onErased } = open(thread);
    fireEvent.click(screen.getByRole("button", { name: "Stop and delete" }));
    await screen.findByRole("status");
    await waitFor(() => expect(onErased).toHaveBeenCalledWith(thread.id), { timeout: 5000 });
    const writes = calls.filter((c) => !c.startsWith("GET"));
    expect(writes).toEqual(["POST /api/threads/:id/cancel 202", "DELETE /api/threads/:id 204"]);
    expect(await exists(thread.id)).toBe(false);
  });

  it("turns a Delete the server refused with 409 thread_active into Stop and delete, with the chat untouched", async () => {
    await aPerson();
    const working = await threadOf("slow task");
    // the list was read before the agent started: it still says done
    open({ ...working, state: "done" });
    fireEvent.click(screen.getByRole("button", { name: "Delete" }));
    await screen.findByRole("button", { name: "Stop and delete" });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(dialog().textContent).toMatch(/working/i);
    expect(await exists(working.id)).toBe(true);
    expect(await stateOf(working.id)).toBe("working");
  });

  it("gives up with a clear error when the agent never stops, deletes nothing, and can try again", async () => {
    await aPerson();
    const thread = await threadOf("slow task");
    // the agent takes the cancel and goes on working
    faked["POST /api/threads/:id/cancel"] = 202;
    const { onErased } = open(thread, { timing: { pollMs: 10, giveUpMs: 120 } });
    fireEvent.click(screen.getByRole("button", { name: "Stop and delete" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/has not stopped/i);
    expect(onErased).not.toHaveBeenCalled();
    expect(calls.some((c) => c.startsWith("DELETE"))).toBe(false);
    expect(await exists(thread.id)).toBe(true);
    // the button is there again
    expect(
      (screen.getByRole("button", { name: "Stop and delete" }) as HTMLButtonElement).disabled,
    ).toBe(false);
  });

  it("says in words when the roles do not let the person stop it (403 on cancel) and deletes nothing", async () => {
    await aPerson();
    const thread = await threadOf("slow task");
    faked["POST /api/threads/:id/cancel"] = 403;
    const { onErased } = open(thread);
    fireEvent.click(screen.getByRole("button", { name: "Stop and delete" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toMatch(/cannot be stopped|do not let you stop/i);
    expect(onErased).not.toHaveBeenCalled();
    expect(calls.some((c) => c.startsWith("DELETE"))).toBe(false);
  });

  it("Cancel while it waits for the stop closes the dialog and never deletes", async () => {
    await aPerson();
    const thread = await threadOf("slow task");
    faked["POST /api/threads/:id/cancel"] = 202;
    const { onClose, onErased } = open(thread, { timing: { pollMs: 10, giveUpMs: 5000 } });
    fireEvent.click(screen.getByRole("button", { name: "Stop and delete" }));
    await screen.findByRole("status");
    fireEvent.click(within(dialog()).getByRole("button", { name: "Cancel" }));
    expect(onClose).toHaveBeenCalled();
    await new Promise((r) => setTimeout(r, 100));
    expect(onErased).not.toHaveBeenCalled();
    expect(calls.some((c) => c.startsWith("DELETE"))).toBe(false);
  });

  it("is done quietly when the chat is gone while it waits (404 on the read)", async () => {
    await aPerson();
    const thread = await threadOf("slow task");
    faked["POST /api/threads/:id/cancel"] = 202;
    const { onErased } = open(thread, { timing: { pollMs: 10, giveUpMs: 5000 } });
    fireEvent.click(screen.getByRole("button", { name: "Stop and delete" }));
    await screen.findByRole("status");
    // another tab stopped it and deleted it
    faked["GET /api/threads/:id"] = 404;
    await waitFor(() => expect(onErased).toHaveBeenCalledWith(thread.id), { timeout: 5000 });
    expect(screen.queryByRole("alert")).toBeNull();
    expect(calls.some((c) => c.startsWith("DELETE"))).toBe(false);
  });
});

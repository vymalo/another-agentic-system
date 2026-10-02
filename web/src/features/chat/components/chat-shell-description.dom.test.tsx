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

/*
 * A thread's description (ADR 0035) through the whole app: the mock's `describe` script has the
 * model describe the thread after its job ends, the open thread follows it live, a person edits it
 * (an empty one clears it), and `ui.showDescriptions` hides it everywhere.
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;
let resetUiConfig: () => void;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the app made: `METHOD path status`. */
let calls: string[] = [];
/** The bodies of the PATCH requests, in order. */
let patches: unknown[] = [];
let failing: { key: string; status: number; detail: string } | undefined;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 10_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollTo = () => {};
  Element.prototype.scrollIntoView = () => {};

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
    const key = `${method} ${url.pathname}`;
    if (method === "PATCH") patches.push(JSON.parse((await request?.clone().text()) ?? "null"));
    if (failing && key === failing.key) {
      calls.push(`${key} ${failing.status}`);
      return new Response(
        JSON.stringify({ title: "Failure", status: failing.status, detail: failing.detail }),
        { status: failing.status, headers: { "content-type": "application/problem+json" } },
      );
    }
    const res = await realFetch(abs(input) as RequestInfo, init);
    calls.push(`${key} ${res.status}`);
    return res;
  }) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
  ({ resetUiConfig } = await import("@/features/chat/hooks/use-ui-config"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
  resetUiConfig();
  calls = [];
  patches = [];
  failing = undefined;
});
afterEach(cleanup);

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

async function makeThread(text: string, agent = "reviewer"): Promise<string> {
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
  await res.text();
  return threadId;
}

const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
/** The description under the header: the line, or null when there is none. */
const line = () => document.querySelector("[data-slot='thread-description']");
/** The thread's row in the sidebar (every test of the file made threads with the same words). */
const rowOf = (id: string) =>
  waitFor(() => {
    const link = document.querySelector(`nav[aria-label='Threads'] a[href='/threads/${id}']`);
    expect(link).not.toBeNull();
    return link as HTMLElement;
  });
const field = () => screen.findByRole("textbox", { name: "Thread description" });

/** An item of the thread's overflow menu, opened from the keyboard. */
async function menuItem(name: string | RegExp): Promise<HTMLElement> {
  const trigger = await screen.findByRole("button", { name: "Thread options" });
  if (trigger.getAttribute("aria-expanded") !== "true") {
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
  }
  return screen.findByRole("menuitem", { name });
}

const MODEL = "The person wants a plan for a test.";

describe("a thread's description in the app", () => {
  it("the model's description appears under the header while the thread is open, and in the sidebar", async () => {
    const id = await makeThread("describe talk to me");
    shell(id);
    // the description may come while the thread is open or before it: either way, one line
    await waitFor(() => expect(line()?.textContent).toBe(MODEL));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    // the sidebar row reads it as the link's description
    const row = await rowOf(id);
    const described = row.getAttribute("aria-describedby") ?? "";
    await waitFor(() => expect(document.getElementById(described)?.textContent).toBe(MODEL));
  });

  it("a description that arrives after the thread is open is drawn with no reload", async () => {
    const id = await makeThread("talk to me");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(line()).toBeNull();
    // the orchestrator's model describes it later (a producer-initiated run on the stream)
    const written = await realFetch(`${base}/api/threads/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ description: "Written elsewhere." }),
    });
    expect(written.status).toBe(200);
    await waitFor(() => expect(line()?.textContent).toBe("Written elsewhere."));
    // and cleared elsewhere
    await realFetch(`${base}/api/threads/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ description: "" }),
    });
    await waitFor(() => expect(line()).toBeNull());
  });

  it("is plain text, whatever the model wrote", async () => {
    const id = await makeThread("talk to me");
    const text = "**bold** <img src=x onerror=alert(1)> [x](javascript:alert(1))";
    await realFetch(`${base}/api/threads/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ description: text }),
    });
    shell(id);
    await waitFor(() => expect(line()?.textContent).toBe(text));
    expect(line()?.querySelector("strong, img, a, script")).toBeNull();
  });

  it("Add description opens a field, Enter saves it through the API and the line says it", async () => {
    const id = await makeThread("talk to me");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    fireEvent.click(await menuItem("Add description"));
    const input = (await field()) as HTMLInputElement;
    expect(input.value).toBe("");
    await waitFor(() => expect(document.activeElement).toBe(input));
    fireEvent.change(input, { target: { value: "  Plan the login test.  " } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(calls).toContain(`PATCH /api/threads/${id} 200`));
    expect(patches).toEqual([{ description: "Plan the login test." }]);
    await waitFor(() => expect(line()?.textContent).toBe("Plan the login test."));
    expect(screen.queryByRole("textbox", { name: "Thread description" })).toBeNull();
    // the menu now offers to edit it
    expect(await menuItem("Edit description")).toBeTruthy();
  });

  it("an empty description clears it, and Escape gives an edit up and sends nothing", async () => {
    const id = await makeThread("describe talk to me");
    shell(id);
    await waitFor(() => expect(line()?.textContent).toBe(MODEL));

    fireEvent.click(await menuItem("Edit description"));
    let input = (await field()) as HTMLInputElement;
    expect(input.value).toBe(MODEL);
    fireEvent.change(input, { target: { value: "Something else" } });
    fireEvent.keyDown(input, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByRole("textbox", { name: "Thread description" })).toBeNull(),
    );
    expect(line()?.textContent).toBe(MODEL);
    expect(patches).toEqual([]);

    // the same words are no edit either
    fireEvent.click(await menuItem("Edit description"));
    input = (await field()) as HTMLInputElement;
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(screen.queryByRole("textbox", { name: "Thread description" })).toBeNull(),
    );
    expect(patches).toEqual([]);

    fireEvent.click(await menuItem("Edit description"));
    input = (await field()) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.blur(input);
    await waitFor(() => expect(line()).toBeNull());
    expect(patches).toEqual([{ description: "" }]);
    expect(await menuItem("Add description")).toBeTruthy();
  });

  it("a refused edit says why, keeps the field and the words, and works once the server does", async () => {
    const id = await makeThread("describe talk to me");
    shell(id);
    await waitFor(() => expect(line()?.textContent).toBe(MODEL));
    failing = { key: `PATCH /api/threads/${id}`, status: 503, detail: "storage is unavailable" };
    fireEvent.click(await menuItem("Edit description"));
    const input = (await field()) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "Mine." } });
    fireEvent.keyDown(input, { key: "Enter" });
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not save the description: storage is unavailable");
    expect(((await field()) as HTMLInputElement).value).toBe("Mine.");

    failing = undefined;
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() => expect(line()?.textContent).toBe("Mine."));
    expect(screen.queryByText(/Could not save the description/)).toBeNull();
  });

  it("the title's Rename is unchanged beside it", async () => {
    const id = await makeThread("describe talk to me");
    shell(id);
    await waitFor(() => expect(line()?.textContent).toBe(MODEL));
    fireEvent.click(await menuItem("Rename"));
    const title = await screen.findByRole("textbox", { name: "Thread title" });
    fireEvent.change(title, { target: { value: "Test plan" } });
    fireEvent.keyDown(title, { key: "Enter" });
    await waitFor(() => expect(patches).toEqual([{ title: "Test plan" }]));
    await waitFor(() =>
      expect(within(document.body).getByRole("heading", { level: 1 }).textContent).toBe(
        "Test plan",
      ),
    );
    expect(line()?.textContent).toBe(MODEL);
  });

  it("with ui.showDescriptions off there is no line, no hover card text and no menu item", async () => {
    const id = await makeThread("describe talk to me");
    await waitFor(async () => {
      const got = (await (await realFetch(`${base}/api/threads/${id}`)).json()) as {
        description?: string;
      };
      expect(got.description).toBe(MODEL);
    });
    await realFetch(`${base}/__mock/config?showDescriptions=false`, { method: "POST" });
    try {
      shell(id);
      await waitFor(() => expect(calls).toContain("GET /api/config 200"));
      await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
      const row = await rowOf(id);
      expect(row.getAttribute("aria-describedby")).toBeNull();
      expect(line()).toBeNull();
      expect(screen.queryByText(MODEL)).toBeNull();
      await screen.findByRole("button", { name: "Thread options" });
      const trigger = screen.getByRole("button", { name: "Thread options" });
      trigger.focus();
      fireEvent.keyDown(trigger, { key: "Enter" });
      await screen.findByRole("menuitem", { name: "Rename" });
      expect(screen.queryByRole("menuitem", { name: /description/ })).toBeNull();
    } finally {
      await realFetch(`${base}/__mock/config?showDescriptions=true`, { method: "POST" });
    }
  });
});

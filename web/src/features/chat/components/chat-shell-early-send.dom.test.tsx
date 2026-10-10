// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { cleanup, configure, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { createMockServer } from "../../../../mock/server";

/*
 * A message written before the newest turns of a thread opened at its end are in the runtime (ADR 0059), through the whole app.
 * The page says Done as soon as it has read the page of history; the transcript follows when the seed has been made and imported,
 * and the import replaces what the runtime holds. A message added meanwhile would be gone with it, and the answer to it would
 * have no question, so the box keeps the message and the page sends it when the conversation is on screen.
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

/** The seed is made when the test says: the page has read the history and the transcript is not in yet. */
const gate = vi.hoisted(() => ({
  open: Promise.resolve() as Promise<void>,
  release: () => {},
}));
vi.mock("@/features/chat/lib/agui/seed", async (importOriginal) => {
  const real = await importOriginal<typeof import("@/features/chat/lib/agui/seed")>();
  return {
    ...real,
    buildMessages: async (...args: Parameters<typeof real.buildMessages>) => {
      await gate.open;
      return real.buildMessages(...args);
    },
  };
});

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** The messages the app sent to an agent: `POST /agui/agents/{id}`, with the words. */
let sent: string[] = [];

const turns = () => document.querySelectorAll('[data-slot="user-message"]').length;
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const box = () => screen.getByLabelText("Message") as HTMLTextAreaElement;

beforeAll(async () => {
  configure({ asyncUtilTimeout: 20_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView = () => {};
  Element.prototype.scrollTo = () => {};

  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  // nosemgrep: opt.opengrep-rules.typescript.react.security.react-insecure-request -- loopback test server, never leaves the runner
  base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  const abs = (input: unknown) =>
    typeof input === "string" && input.startsWith("/") ? base + input : input;
  globalThis.Request = class extends RealRequest {
    constructor(input: RequestInfo | URL, init?: RequestInit) {
      super(abs(input) as RequestInfo, init);
    }
  } as typeof Request;
  globalThis.fetch = (async (input: RequestInfo | URL, init?: RequestInit) => {
    const request =
      input instanceof Request ? input : new RealRequest(abs(input) as RequestInfo, init);
    if (request.method === "POST" && new URL(request.url).pathname.startsWith("/agui/agents/")) {
      const body = (await request.clone().json()) as { messages: { content: string }[] };
      sent.push(...body.messages.map((m) => m.content));
    }
    return realFetch(request);
  }) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  sent = [];
  gate.open = new Promise<void>((r) => {
    gate.release = r;
  });
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
});
afterEach(() => {
  gate.release();
  cleanup();
});

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

async function longThread(turns: number): Promise<string> {
  const res = await realFetch(`${base}/__mock/long-thread?turns=${turns}`, { method: "POST" });
  return ((await res.json()) as { threadId: string }).threadId;
}

/** The page says Done, and the transcript is not in the runtime: the seed waits for the test. */
async function openedBeforeTheSeed(turnsOfHistory: number) {
  const id = await longThread(turnsOfHistory);
  shell(id);
  await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  // the agent the message is for is known
  await screen.findByRole("button", { name: /^Agent:/ });
  expect(turns()).toBe(0);
  return id;
}

const ways: [string, () => void][] = [
  ["the Send button", () => fireEvent.click(screen.getByRole("button", { name: "Send" }))],
  ["Enter", () => fireEvent.keyDown(box(), { key: "Enter" })],
];

describe("a message written before the transcript is in", { timeout: 60_000 }, () => {
  for (const [way, press] of ways) {
    it(`sent with ${way}, is held, goes out once the conversation is shown, and is in it`, async () => {
      await openedBeforeTheSeed(3);

      fireEvent.change(box(), { target: { value: "Fix the next thing" } });
      press();
      await new Promise((r) => setTimeout(r, 150));
      expect(sent).toEqual([]);
      expect(box().value).toBe("Fix the next thing");

      gate.release();
      await waitFor(() => expect(sent).toEqual(["Fix the next thing"]));
      // the three turns of the history and the new one, in this order: the import did not take the message
      await waitFor(() => expect(turns()).toBe(4));
      await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
      const said = [...document.querySelectorAll('[data-slot="user-message"]')].map(
        (m) => m.textContent ?? "",
      );
      expect(said.at(-1)).toContain("Fix the next thing");
      expect(box().value).toBe("");
      expect(sent).toEqual(["Fix the next thing"]);
    });
  }
});

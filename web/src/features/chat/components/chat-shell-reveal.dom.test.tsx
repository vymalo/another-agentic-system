// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import { cleanup, configure, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { createMockServer } from "../../../../mock/server";

/*
 * A thread that is opened is replayed from its log, and the transcript is not drawn until the replay is applied
 * (ADR 0059, slice 1): the person sees the skeleton, then the whole conversation, drawn at its end, and the page does
 * not scroll in between. Through the whole app, against the mock's long thread (`POST /__mock/long-thread`).
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;

/** Where the viewport was told to go, and whether the transcript was still held back when it was. */
const scrolls: { held: boolean; top: number | undefined; behavior: ScrollBehavior | undefined }[] =
  [];
const SCROLL_HEIGHT = 4321;

const wrapper = () => document.querySelector<HTMLElement>('[data-slot="aui_messages"]');
const held = () => wrapper()?.hasAttribute("data-held") ?? true;
const turns = () => document.querySelectorAll('[data-slot="user-message"]').length;
const skeleton = () => document.querySelector('[data-slot="aui_thread-history-skeleton"]');
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const viewport = () => document.querySelector<HTMLElement>('[data-slot="aui_thread-viewport"]');

beforeAll(async () => {
  configure({ asyncUtilTimeout: 20_000 });
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView = () => {};
  Element.prototype.scrollTo = ((options?: ScrollToOptions) => {
    scrolls.push({ held: held(), top: options?.top, behavior: options?.behavior });
  }) as typeof Element.prototype.scrollTo;
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get: () => SCROLL_HEIGHT,
  });

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
  globalThis.fetch = ((input: RequestInfo | URL, init?: RequestInit) =>
    realFetch(abs(input) as RequestInfo, init)) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});
beforeEach(() => {
  scrolls.length = 0;
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
});
afterEach(cleanup);

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

/** What the page looked like each time it changed: held or not, how many turns, the skeleton. */
function watch() {
  const seen: { held: boolean; turns: number; skeleton: boolean }[] = [];
  const record = () => seen.push({ held: held(), turns: turns(), skeleton: skeleton() !== null });
  const observer = new MutationObserver(record);
  observer.observe(document.body, { childList: true, subtree: true, attributes: true });
  return { seen, stop: () => observer.disconnect() };
}

describe("the transcript of an opened thread", { timeout: 60_000 }, () => {
  it("is not drawn, with the skeleton in its place, while the log replays, and is drawn once with every turn in it", async () => {
    const id = await longThread(12);
    const page = watch();
    shell(id);
    await waitFor(() => expect(turns()).toBe(12));
    await waitFor(() => expect(held()).toBe(false));
    page.stop();

    // not a turn of it was drawn while the log replayed, and the skeleton stood in its place
    const waiting = page.seen.filter((s) => s.held);
    expect(waiting.length).toBeGreaterThan(0);
    expect(waiting.every((s) => s.turns === 0 && s.skeleton)).toBe(true);
    // and it was never drawn with a part of the conversation in it
    const shown = page.seen.filter((s) => !s.held);
    expect(shown.length).toBeGreaterThan(0);
    expect(shown.every((s) => s.turns === 12 && !s.skeleton)).toBe(true);
    expect(screen.getByRole("log", { name: "Conversation" }).getAttribute("aria-busy")).toBe(
      "false",
    );
  });

  it("is scrolled to its end, instantly, when it is shown, and is smooth for what comes after", async () => {
    const id = await longThread(12);
    shell(id);
    await waitFor(() => expect(turns()).toBe(12));
    await waitFor(() => expect(held()).toBe(false));

    expect(scrolls.filter((s) => !s.held)).toContainEqual({
      held: false,
      top: SCROLL_HEIGHT,
      behavior: "instant",
    });
    // the viewport is smooth for what comes after (the button, a run that starts), not before
    expect(viewport()?.className).toContain("scroll-smooth");
  });

  it("is not drawn in a smooth viewport, and the log is busy", async () => {
    const id = await longThread(12);
    shell(id);
    await waitFor(() => expect(wrapper()).not.toBeNull());
    expect(held()).toBe(true);
    expect(viewport()?.className).not.toContain("scroll-smooth");
    expect(screen.getByRole("log", { name: "Conversation" }).getAttribute("aria-busy")).toBe(
      "true",
    );
    await waitFor(() => expect(held()).toBe(false));
  });

  it("is not held back again by a run that starts after it was shown", async () => {
    const id = await longThread(3);
    shell(id);
    await waitFor(() => expect(held()).toBe(false));
    const page = watch();

    fireEvent.change(screen.getByLabelText("Message"), { target: { value: "Fix the next thing" } });
    fireEvent.click(await screen.findByRole("button", { name: "Send" }));
    await waitFor(() => expect(turns()).toBe(4));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    page.stop();
    expect(page.seen.length).toBeGreaterThan(0);
    expect(page.seen.every((s) => !s.held && !s.skeleton)).toBe(true);
  });
});

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
import { OWN_CATALOG, UI_CATALOG_PROP } from "@/features/chat/lib/a2ui/catalog";
import { SessionBanner } from "@/features/session/components/session-banner";
import { resetSessionState, SIGNED_IN_CHANNEL, sessionStatus } from "@/lib/api/session-refresh";
import { uuidv7 } from "@/lib/uuid";
import { createMockServer } from "../../../../mock/server";

/*
 * Who the person is and what their roles let them do (ADR 0033), through the whole app: the real
 * mock orchestrator answers `GET /api/me` for the session the test is, and enforces what it says,
 * so what the page hides is also what the server would refuse. A session is a profile of the mock:
 * `user`, `admin` (a user who also holds `admin`: nobody reads another person's thread, ADR 0039),
 * `read-only`, `limited` (invokes the reviewer only) and `no-access`.
 */

const router = { push: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("./chat-shell").ChatShell;
let resetMe: () => void;
let resetUiConfig: () => void;
let resetRedirectPause: () => void;
let navigation: { go: (url: string) => void };

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the app made: `METHOD path?query status`. */
let calls: string[] = [];
/** The mock session the app's requests carry (the cookie its browser would). */
let cookie = "";
let failing: { key: string; status: number; detail: string; code?: string } | undefined;

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
    if (failing && key === failing.key) {
      calls.push(`${key} ${failing.status}`);
      return new Response(
        JSON.stringify({
          title: "Failure",
          status: failing.status,
          detail: failing.detail,
          ...(failing.code ? { code: failing.code } : {}),
        }),
        { status: failing.status, headers: { "content-type": "application/problem+json" } },
      );
    }
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    const sent = cookie;
    if (sent) carried.headers.set("cookie", sent);
    const res = await realFetch(carried);
    // an answer that comes after the test that asked is over is not this test's call
    if (sent === cookie) calls.push(`${key}${url.search} ${res.status}`);
    return res;
  }) as typeof fetch;
  ({ ChatShell } = await import("./chat-shell"));
  ({ resetMe } = await import("@/features/me/hooks/use-me"));
  ({ resetUiConfig } = await import("@/features/chat/hooks/use-ui-config"));
  ({ resetRedirectPause, navigation } = await import("@/lib/api/session"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

let sessions = 0;
type Profile = "user" | "admin" | "read-only" | "limited" | "no-access";
/** Makes the app's requests those of a session that is `profile`; returns its cookie, for the test's own. */
async function as(profile: Profile): Promise<string> {
  const session = `dom-roles-${++sessions}`;
  await realFetch(`${base}/__mock/config?me=${profile}&session=${session}`, { method: "POST" });
  cookie = `mock-registry=${session}`;
  return cookie;
}

beforeEach(async () => {
  await realFetch(`${base}/__mock/reset`, { method: "POST" });
  Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true, writable: true });
  window.localStorage.clear();
  window.sessionStorage.clear();
  window.history.pushState({}, "", "/");
  resetMe();
  resetUiConfig();
  resetRedirectPause();
  resetSessionState();
  calls = [];
  cookie = "";
  failing = undefined;
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

const shell = (threadId: string | null) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );

/** A thread made by the session `as` (no cookie: the default person, dev@example.com), run to its end. */
async function makeThread(
  text: string,
  { agent = "reviewer", as: who = "", catalog = false } = {},
): Promise<string> {
  const threadId = uuidv7();
  const res = await realFetch(`${base}/agui/agents/${agent}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      Accept: "text/event-stream",
      ...(who ? { cookie: who } : {}),
    },
    body: JSON.stringify({
      threadId,
      runId: "run-1",
      messages: [{ id: "m-1", role: "user", content: text }],
      forwardedProps: catalog ? { [UI_CATALOG_PROP]: OWN_CATALOG } : {},
    }),
  });
  expect(res.status).toBe(200);
  await res.text();
  return threadId;
}

const composer = () => screen.queryByRole("textbox", { name: "Message" });
const notice = () => document.querySelector("[data-slot='read-only']");
const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const rows = () =>
  [...document.querySelectorAll("nav[aria-label='Threads'] [data-slot='thread-row']")].map(
    (r) => r.textContent ?? "",
  );

async function menuItem(name: string | RegExp): Promise<HTMLElement> {
  const trigger = await screen.findByRole("button", { name: "Thread options" });
  if (trigger.getAttribute("aria-expanded") !== "true") {
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
  }
  return screen.findByRole("menuitem", { name });
}
const isDisabled = (el: HTMLElement) =>
  el.getAttribute("aria-disabled") === "true" || el.hasAttribute("data-disabled");

describe("GET /api/me", () => {
  it("is read once per page load, whatever asks", async () => {
    const id = await makeThread("echo once");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(calls.filter((c) => c.startsWith("GET /api/me"))).toEqual(["GET /api/me 200"]);
  });

  it("a person who may do everything sees the page as before roles", async () => {
    const id = await makeThread("echo mine");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(composer()).not.toBeNull();
    expect(notice()).toBeNull();
    expect(screen.queryByText("Read only")).toBeNull();
    expect((await menuItem("Rename")).hasAttribute("data-disabled")).toBe(false);
  });

  it("an orchestrator that cannot say shows everything, and the server decides", async () => {
    // a role that may not write: with `GET /api/me` failing the page does not know, so the box is
    // there, and the server's own words answer a send
    const id = await makeThread("echo viewed");
    await realFetch(`${base}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
      method: "POST",
    });
    await as("read-only");
    failing = { key: "GET /api/me", status: 404, detail: "not found" };
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(composer()).not.toBeNull();
    expect(notice()).toBeNull();
    fireEvent.change(composer() as HTMLElement, { target: { value: "echo more" } });
    fireEvent.click(screen.getByRole("button", { name: "Send" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("your roles do not grant");
    expect((composer() as HTMLTextAreaElement).value).toBe("echo more");
  });
});

describe("another person's thread", () => {
  it("does not exist for an administrator: not found, nothing of it, and no write asked", async () => {
    const theirs = await makeThread("echo theirs");
    await as("admin");
    shell(theirs);
    // the answer is the one for a thread nobody has (ADR 0039): no title, no conversation, no owner
    expect(await screen.findByText(/Thread not found/)).toBeTruthy();
    expect(screen.queryByText("echo theirs")).toBeNull();
    expect(composer()).toBeNull();
    expect(notice()).toBeNull();
    expect(screen.queryByText(/dev@example\.com/)).toBeNull();
    expect(calls.some((c) => /^(PATCH|POST) \/(api|agui)\/(threads|agents)/.test(c))).toBe(false);
  });

  it("an administrator's own thread is theirs, as a user's is", async () => {
    const admin = await as("admin");
    const own = await makeThread("echo own", { as: admin });
    shell(own);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(composer()).not.toBeNull();
    expect(notice()).toBeNull();
    expect(isDisabled(await menuItem("Rename"))).toBe(false);
  });
});

describe("a thread the person may read and not change", () => {
  it("a role without thread.write: no box, no write action, the export stays", async () => {
    const id = await makeThread("echo viewed");
    await realFetch(`${base}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
      method: "POST",
    });
    await as("read-only");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await waitFor(() => expect(notice()).not.toBeNull());
    expect(notice()?.textContent).toBe("Read only: your roles do not let you write in threads.");
    expect(notice()?.getAttribute("role")).toBe("status");
    expect(screen.getByText("Read only")).toBeTruthy();
    // no message box, no Send, no Stop
    expect(composer()).toBeNull();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    // the person's own words cannot be edited, and no turn can be forked
    expect(screen.queryByRole("button", { name: "Edit what you said" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Fork from here" })).toBeNull();
    // the conversation is there to read, and so is the export
    expect(screen.getAllByText("echo viewed").length).toBeGreaterThan(0);
    const rename = await menuItem("Rename");
    expect(isDisabled(rename)).toBe(true);
    expect(rename.getAttribute("title")).toContain("Read only: your roles do not let you write");
    expect(
      isDisabled(await screen.findByRole("menuitem", { name: /Add description|Edit description/ })),
    ).toBe(true);
    expect(isDisabled(await screen.findByRole("menuitem", { name: "Export JSON" }))).toBe(false);
    // nothing was written to the thread, and the page asked for nothing it may not
    expect(calls.some((c) => /^(PATCH|POST) \/(api|agui)\/(threads|agents)/.test(c))).toBe(false);
  });

  it("the agent menu does not offer to continue with another agent", async () => {
    const id = await makeThread("echo viewed");
    await realFetch(`${base}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
      method: "POST",
    });
    await as("read-only");
    shell(id);
    await waitFor(() => expect(notice()).not.toBeNull());
    fireEvent.keyDown(await screen.findByRole("button", { name: /^Agent:/ }), { key: "Enter" });
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByText(/Read only: your roles do not let you write/)).toBeTruthy();
    for (const item of within(menu).getAllByRole("menuitemradio")) {
      if (item.getAttribute("aria-checked") !== "true") expect(isDisabled(item)).toBe(true);
    }
  });

  it("the card's actions are off, and the card says why", async () => {
    const id = await makeThread("choices now", { catalog: true });
    await realFetch(`${base}/__mock/owner?thread=${id}&owner=viewer@example.com`, {
      method: "POST",
    });
    await as("read-only");
    shell(id);
    const send = await screen.findByRole("button", { name: "Send answers" });
    await waitFor(() => expect(notice()).not.toBeNull());
    expect((send as HTMLButtonElement).disabled).toBe(true);
    for (const radio of screen.getAllByRole("radio")) {
      expect(radio.matches(":disabled")).toBe(true);
    }
    expect(
      screen.getByText(/Read only: your roles do not let you write in threads\. You cannot act/),
    ).toBeTruthy();
    expect(composer()).toBeNull();
  });

  it("a thread of an agent the roles may not invoke is read-only too, and says which", async () => {
    const id = await makeThread("echo coder", { agent: "coder" });
    await realFetch(`${base}/__mock/owner?thread=${id}&owner=limited@example.com`, {
      method: "POST",
    });
    await as("limited");
    shell(id);
    await waitFor(() => expect(notice()).not.toBeNull());
    expect(notice()?.textContent).toBe("Read only: your roles do not let you use the coder agent.");
    expect(composer()).toBeNull();
    // the thread's own agent is still named in the top bar, whatever the roles say
    expect((await screen.findByRole("button", { name: /^Agent:/ })).textContent).toContain("Coder");
  });
});

describe("what a new chat offers", () => {
  it("the agent picker lists only the agents the roles may invoke", async () => {
    await as("limited");
    shell(null);
    const picker = await screen.findByRole("button", { name: /^Agent:/ });
    await waitFor(() => expect(picker.textContent).toContain("Reviewer"));
    fireEvent.keyDown(picker, { key: "Enter" });
    const menu = await screen.findByRole("menu");
    const group = within(menu).getByRole("group", { name: "Agents" });
    expect(
      within(group)
        .getAllByRole("menuitemradio")
        .map((i) => i.textContent),
    ).toEqual([expect.stringContaining("Reviewer")]);
    expect(composer()).not.toBeNull();
  });

  it("a role that cannot start a chat gets a line instead of the box", async () => {
    await as("read-only");
    shell(null);
    await waitFor(() => expect(notice()).not.toBeNull());
    expect(notice()?.textContent).toBe("Your roles do not let you start chats.");
    expect(composer()).toBeNull();
    expect(screen.queryByRole("list", { name: "Suggestions" })).toBeNull();
  });
});

describe("the thread list is the person's own, for every role", () => {
  it("asks without owner, and has no switch to anyone else's", async () => {
    await makeThread("echo plain");
    shell(null);
    await waitFor(() => expect(rows().length).toBeGreaterThan(0));
    expect(screen.queryByRole("group", { name: "Whose threads" })).toBeNull();
    expect(screen.queryByRole("button", { name: "All threads" })).toBeNull();
    expect(calls.some((c) => c.includes("owner="))).toBe(false);
  });

  it("is an administrator's own threads and nobody else's, with no switch and no owner lines", async () => {
    await makeThread("echo dev one");
    const admin = await as("admin");
    await makeThread("echo admin one", { as: admin });
    shell(null);
    await waitFor(() => expect(rows()).toHaveLength(1));
    expect(rows()[0]).toContain("echo admin one");
    expect(screen.queryByRole("group", { name: "Whose threads" })).toBeNull();
    expect(screen.queryByRole("button", { name: "All threads" })).toBeNull();
    expect(document.querySelector("[data-slot='thread-owner']")).toBeNull();
    expect(calls.some((c) => c.includes("owner="))).toBe(false);
  });

  it("forgets a choice an earlier version stored: nobody is shown everyone's", async () => {
    window.localStorage.setItem("another-agentic.thread-scope", "all");
    await makeThread("echo someone else's");
    await as("admin");
    shell(null);
    await waitFor(() => expect(calls.some((c) => c.startsWith("GET /api/threads"))).toBe(true));
    expect(screen.queryByRole("group", { name: "Whose threads" })).toBeNull();
    expect(calls.some((c) => c.includes("owner="))).toBe(false);
    await waitFor(() => expect(rows()).toHaveLength(0));
  });
});

describe("no access", () => {
  it("replaces the chat with who the person is signed in as, and asks for nothing else", async () => {
    await as("no-access");
    shell(null);
    expect(await screen.findByRole("heading", { level: 1, name: "No access" })).toBeTruthy();
    expect(screen.getByText("nobody@example.com")).toBeTruthy();
    expect(screen.getByText(/Nina Nobody/)).toBeTruthy();
    expect(composer()).toBeNull();
    expect(screen.queryByRole("navigation", { name: "Threads" })).toBeNull();
    // `GET /api/me` answered, and said so once
    expect(calls.filter((c) => c.startsWith("GET /api/me"))).toEqual(["GET /api/me 200"]);
  });
});

describe("a 401: the session has expired", () => {
  const expired = () => {
    failing = { key: "GET /api/agents", status: 401, detail: "missing X-Auth-Request-Email" };
  };
  /** The mock session `cookie` names, as the edge's `forward_auth` leaves it: the token ran out, or the person is not in. */
  const edgeState = (state: "stale" | "signed-out") =>
    realFetch(
      `${base}/__mock/config?session=${cookie.split("=")[1]}&${
        state === "stale" ? "stale=true" : "signedIn=false"
      }`,
      { method: "POST" },
    );
  const withBanner = () =>
    render(
      <TooltipProvider>
        <ChatShell threadId={null} />
        <SessionBanner />
      </TooltipProvider>,
    );

  it("is refreshed at the edge and the request goes again: the page is not left, the agents are listed", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    const go = vi.spyOn(navigation, "go").mockImplementation(() => {});
    await as("user");
    await edgeState("stale");
    shell(null);
    expect(await screen.findByRole("button", { name: /^Agent:/ })).toBeTruthy();
    // the page's first calls all meet the 401 together; the browser asks the edge ONCE (which refreshed the
    // token), and every one of those calls went again and was answered
    const refused = calls.filter((c) => c.endsWith(" 401")).map((c) => c.slice(0, -4));
    expect(refused.length).toBeGreaterThan(1);
    expect(calls.filter((c) => c.startsWith("GET /oauth2/userinfo"))).toEqual([
      "GET /oauth2/userinfo 200",
    ]);
    for (const call of refused) {
      expect(calls.indexOf(`${call} 200`), call).toBeGreaterThan(calls.indexOf(`${call} 401`));
    }
    expect(calls.filter((c) => c.endsWith(" 401")).length).toBe(refused.length);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(go).not.toHaveBeenCalled();
    expect(sessionStatus()).toBe("ok");
  });

  it("with no session says so, keeps the page, and goes on when the person has signed in in a popup", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    const go = vi.spyOn(navigation, "go").mockImplementation(() => {});
    const popup = { opener: {} as unknown, location: { href: "" } };
    const open = vi.spyOn(window, "open").mockReturnValue(popup as unknown as Window);
    window.history.pushState({}, "", "/threads/abc?tab=sources#m-3");
    await as("user");
    await edgeState("signed-out");
    withBanner();
    await waitFor(() => expect(sessionStatus()).toBe("ended"));
    const notice = document.querySelector('[data-slot="session-banner"]') as HTMLElement;
    expect(notice.textContent).toContain("Your session has ended");
    // held, not failed: no error line, and nothing navigated
    expect(screen.queryByRole("alert")).toBeNull();
    expect(go).not.toHaveBeenCalled();

    // the button opens the sign-in in a popup that ends at the page that closes itself
    fireEvent.click(within(notice).getByRole("button", { name: "Sign in" }));
    expect(open).toHaveBeenCalledTimes(1);
    expect(popup.location.href).toBe("/oauth2/start?rd=%2Fsigned-in");
    expect(go).not.toHaveBeenCalled();

    // the popup signs the session in (the issuer approves) and its last page says so
    await realFetch(`${base}/oauth2/start?rd=/signed-in`, {
      headers: { cookie },
      redirect: "manual",
    });
    const channel = new BroadcastChannel(SIGNED_IN_CHANNEL);
    channel.postMessage("signed-in");
    channel.close();
    expect(await screen.findByRole("button", { name: /^Agent:/ })).toBeTruthy();
    await waitFor(() => expect(document.querySelector('[data-slot="session-banner"]')).toBeNull());
    expect(sessionStatus()).toBe("ok");
    expect(go).not.toHaveBeenCalled();
  });

  it("without a sign-in path it is the error line it always was, and nothing navigates or asks the edge", async () => {
    const go = vi.spyOn(navigation, "go").mockImplementation(() => {});
    expired();
    shell(null);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("Could not load agents: missing X-Auth-Request-Email");
    expect(within(alert).getByRole("button", { name: "Retry" })).toBeTruthy();
    expect(go).not.toHaveBeenCalled();
    expect(calls.some((c) => c.includes("/oauth2/"))).toBe(false);
  });

  it("a path that is not of this origin is no sign-in", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "//evil.example/sign_in");
    const go = vi.spyOn(navigation, "go").mockImplementation(() => {});
    expired();
    shell(null);
    await screen.findByRole("alert");
    expect(go).not.toHaveBeenCalled();
    expect(calls.some((c) => c.includes("/oauth2/"))).toBe(false);
  });
});

describe("the other 403s are the server's words, where the action was", () => {
  it("a refused rename says why and keeps the field", async () => {
    const id = await makeThread("echo mine");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    failing = {
      key: `PATCH /api/threads/${id}`,
      status: 403,
      detail: "your roles do not grant thread.write",
      code: "forbidden",
    };
    fireEvent.click(await menuItem("Rename"));
    const title = await screen.findByRole("textbox", { name: "Thread title" });
    fireEvent.change(title, { target: { value: "Other" } });
    fireEvent.keyDown(title, { key: "Enter" });
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(
      "Could not rename the thread: your roles do not grant thread.write",
    );
  });

  it("a refused fork says why", async () => {
    const id = await makeThread("echo mine");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    failing = {
      key: `POST /api/threads/${id}/fork`,
      status: 403,
      detail: "your roles do not grant thread.write",
      code: "forbidden",
    };
    fireEvent.click(await screen.findByRole("button", { name: "Fork from here" }));
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain(
      "Could not fork the chat: your roles do not grant thread.write",
    );
  });
});

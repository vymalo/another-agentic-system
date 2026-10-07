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
 * Sharing a thread (ADR 0040), through the whole app against the real mock orchestrator: the owner's
 * menu, dialog, chip and sidebar mark, and the page of a link for a signed-in reader, for anybody, and
 * for a link that does not work. A session is a mock session: its cap on sharing (`sharing`) and who it
 * is (`me`), and whether it has an identity (`signedIn`).
 */

const router = { push: vi.fn(), replace: vi.fn() };
vi.mock("next/navigation", () => ({ useRouter: () => router, usePathname: () => "/" }));

let ChatShell: typeof import("@/features/chat/components/chat-shell").ChatShell;
let SharedChat: typeof import("./shared-chat").SharedChat;
let resetMe: () => void;
let resetUiConfig: () => void;
let resetRedirectPause: () => void;
let navigation: { go: (url: string) => void };

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
let calls: string[] = [];
let cookie = "";

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
    const carried = new RealRequest(abs(input) as RequestInfo, init);
    const sent = cookie;
    if (sent) carried.headers.set("cookie", sent);
    const res = await realFetch(carried);
    if (sent === cookie) calls.push(`${method} ${url.pathname} ${res.status}`);
    return res;
  }) as typeof fetch;
  ({ ChatShell } = await import("@/features/chat/components/chat-shell"));
  ({ SharedChat } = await import("./shared-chat"));
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
/** Makes the app's requests those of a session of its own; returns its cookie. */
async function as(
  me: "user" | "admin" | "read-only",
  sharing: "disabled" | "internal" | "public" = "public",
  signedIn = true,
): Promise<string> {
  const session = `dom-share-${++sessions}`;
  await realFetch(
    `${base}/__mock/config?me=${me}&sharing=${sharing}&signedIn=${signedIn}&session=${session}`,
    { method: "POST" },
  );
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
  router.push.mockClear();
  router.replace.mockClear();
  calls = [];
  cookie = "";
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

const shell = (threadId: string) =>
  render(
    <TooltipProvider>
      <ChatShell threadId={threadId} />
    </TooltipProvider>,
  );
const reader = (token: string) =>
  render(
    <TooltipProvider>
      <SharedChat token={token} />
    </TooltipProvider>,
  );

async function makeThread(text: string, { agent = "coder", as: who = "" } = {}): Promise<string> {
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
    }),
  });
  expect(res.status).toBe(200);
  await res.text();
  return threadId;
}

/** Shares a thread through the API as the owner's session does; returns the token. */
async function shareAs(
  who: string,
  id: string,
  visibility: "internal" | "public",
): Promise<string> {
  const res = await realFetch(`${base}/api/threads/${id}/share`, {
    method: "PUT",
    headers: { "Content-Type": "application/json", cookie: who },
    body: JSON.stringify({ visibility }),
  });
  expect(res.status).toBe(200);
  return ((await res.json()) as { url: string }).url.replace("/s/", "");
}

const stateBadge = () => screen.getByRole("status", { name: /^Thread state:/ });
const chip = () => document.querySelector("[data-slot='share-chip']");
/** Picks a choice and saves it, as a person does. */
async function choose(dialog: HTMLElement, name: string) {
  fireEvent.click(within(dialog).getByRole("radio", { name }));
  fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
}
async function openMenu(): Promise<HTMLElement> {
  const trigger = await screen.findByRole("button", { name: "Thread options" });
  if (trigger.getAttribute("aria-expanded") !== "true") {
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "Enter" });
  }
  return trigger;
}
const menuItems = async () => {
  await openMenu();
  return (await screen.findAllByRole("menuitem")).map((i) => i.textContent);
};
async function openDialog(): Promise<HTMLElement> {
  await openMenu();
  fireEvent.click(await screen.findByRole("menuitem", { name: /Share…/ }));
  return screen.findByRole("dialog", { name: "Share this conversation" });
}

describe("the owner's menu", () => {
  it("has Share… when the deployment lets the person share, and not when it does not", async () => {
    const mine = await as("user", "internal");
    const id = await makeThread("echo mine", { as: mine });
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(await menuItems()).toContain("Share…");
    cleanup();
    resetMe();

    await as("user", "disabled");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(await menuItems()).not.toContain("Share…");
  });

  it("keeps Share… for a thread that is shared, even where sharing is now off: taking it down needs only ownership", async () => {
    const mine = await as("user", "internal");
    const id = await makeThread("echo mine", { as: mine });
    await shareAs(mine, id, "internal");
    await as("user", "disabled");
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const dialog = await openDialog();
    // nothing above private can be chosen, and the link is not offered while the deployment has none
    expect(
      within(dialog).getByRole("radio", { name: "Signed-in people with the link" }),
    ).toHaveProperty("disabled", true);
    expect(within(dialog).queryByRole("button", { name: "Copy" })).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Stop sharing" }));
    await waitFor(() => expect(chip()).toBeNull());
    expect(calls).toContain(`DELETE /api/threads/${id}/share 204`);
  });
});

describe("the share dialog", () => {
  it("shares as signed-in people, shows the link, makes a new one, and stops", async () => {
    const mine = await as("user", "public");
    const id = await makeThread("echo mine", { as: mine });
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(chip()).toBeNull();
    const dialog = await openDialog();

    const radios = within(dialog).getAllByRole("radio");
    expect(radios.map((r) => r.getAttribute("aria-checked"))).toEqual(["true", "false", "false"]);
    // the public choice carries its warning, in words
    expect(dialog.textContent).toContain(
      "Anyone with this link can read this conversation, including what you pasted in it. Your e-mail is not shown.",
    );
    expect(within(dialog).queryByRole("button", { name: "Stop sharing" })).toBeNull();

    // a choice is only picked until it is saved: nothing is shared yet
    fireEvent.click(within(dialog).getByRole("radio", { name: "Signed-in people with the link" }));
    expect(chip()).toBeNull();
    expect(calls.some((c) => c.startsWith("PUT"))).toBe(false);
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    const field = (await within(dialog).findByLabelText("Link")) as HTMLInputElement;
    expect(field.value).toMatch(new RegExp(`^${window.location.origin}/s/[A-Za-z0-9_-]{43}$`));
    expect(chip()?.textContent).toBe("Shared · signed-in");

    const first = field.value;
    fireEvent.click(within(dialog).getByRole("button", { name: "New link" }));
    await waitFor(() =>
      expect((within(dialog).getByLabelText("Link") as HTMLInputElement).value).not.toBe(first),
    );
    expect(within(dialog).getByRole("status").textContent).toBe(
      "New link made. The old link no longer works.",
    );

    await choose(dialog, "Anyone with the link");
    await waitFor(() => expect(chip()?.textContent).toBe("Shared · public"));

    // Private is stopping, and the link goes
    await choose(dialog, "Private");
    await waitFor(() => expect(chip()).toBeNull());
    expect(within(dialog).queryByLabelText("Link")).toBeNull();
  });

  it("keeps the focus in the dialog when an action ends: the button that was pressed is disabled for the request, and Done takes the focus", async () => {
    const mine = await as("user", "public");
    const id = await makeThread("echo mine", { as: mine });
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const dialog = await openDialog();
    const focusedIs = (name: string) => () =>
      expect(document.activeElement).toBe(within(dialog).getByRole("button", { name }));

    // Save: after it the button is disabled (nothing is picked), and a disabled button cannot hold the focus
    fireEvent.click(within(dialog).getByRole("radio", { name: "Signed-in people with the link" }));
    const save = within(dialog).getByRole("button", { name: "Save" });
    save.focus();
    fireEvent.click(save);
    await waitFor(() => expect(chip()?.textContent).toBe("Shared · signed-in"));
    await waitFor(focusedIs("Done"));

    // New link: disabled for the request
    const newLink = within(dialog).getByRole("button", { name: "New link" });
    newLink.focus();
    fireEvent.click(newLink);
    await waitFor(() =>
      expect(within(dialog).getByRole("status").textContent).toBe(
        "New link made. The old link no longer works.",
      ),
    );
    await waitFor(focusedIs("Done"));

    // Stop sharing: the button is gone with the link
    const stop = within(dialog).getByRole("button", { name: "Stop sharing" });
    stop.focus();
    fireEvent.click(stop);
    await waitFor(() => expect(chip()).toBeNull());
    await waitFor(focusedIs("Done"));
  });

  it("disables each choice above the cap, with the reason", async () => {
    const mine = await as("user", "internal");
    const id = await makeThread("echo mine", { as: mine });
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const dialog = await openDialog();
    const anyone = within(dialog).getByRole("radio", { name: "Anyone with the link" });
    expect(anyone).toHaveProperty("disabled", true);
    expect(dialog.textContent).toContain("This deployment shares only with signed-in people.");
    expect(within(dialog).getByRole("radio", { name: "Private" })).toHaveProperty(
      "disabled",
      false,
    );
  });

  it("says the server's words when a change is refused, and changes nothing", async () => {
    const mine = await as("user", "public");
    const id = await makeThread("echo mine", { as: mine });
    shell(id);
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    const dialog = await openDialog();
    // the deployment lowers its cap after the page has read it
    await realFetch(`${base}/__mock/config?sharing=disabled&session=${mine.split("=")[1]}`, {
      method: "POST",
    });
    await choose(dialog, "Signed-in people with the link");
    const alert = await within(dialog).findByRole("alert");
    expect(alert.textContent).toContain("sharing is turned off on this deployment");
    expect(chip()).toBeNull();
  });
});

describe("the page of a link", () => {
  it("is the conversation, read-only: a banner, no box, no menu, no fork, no edit, no e-mail", async () => {
    const owner = await as("user", "public");
    const id = await makeThread("echo what I wrote", { as: owner });
    const token = await shareAs(owner, id, "internal");
    await as("admin", "public");
    reader(token);
    expect(await screen.findByText("Shared conversation, read only")).toBeTruthy();
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    expect(document.body.textContent).toContain("what I wrote");
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Thread options" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Fork from here" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Edit what you said" })).toBeNull();
    expect(screen.getByRole("button", { name: "Copy link" })).toBeTruthy();
    expect(document.body.textContent).not.toContain("dev@example.com");
    // a signed-in reader is read by the signed-in routes
    expect(calls).toContain(`GET /api/shared/${token} 200`);
    expect(calls.some((c) => c.includes("/api/public/"))).toBe(false);
    expect(calls.some((c) => /^POST \/agui\/agents/.test(c))).toBe(false);
    await waitFor(() => expect(calls).toContain(`GET /agui/shared/${token}/connect 200`));
  });

  it("on a 401 asks the public route, and reads as anybody", async () => {
    const owner = await as("user", "public");
    const id = await makeThread("echo for everybody", { as: owner });
    const token = await shareAs(owner, id, "public");
    await as("user", "public", false);
    reader(token);
    expect(await screen.findByText("Shared conversation, read only")).toBeTruthy();
    expect(calls.slice(0, 2)).toEqual([
      `GET /api/shared/${token} 401`,
      `GET /api/public/shared/${token} 200`,
    ]);
    await waitFor(() => expect(calls).toContain(`GET /agui/public/shared/${token}/connect 200`));
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
  });

  it("sends the owner to the thread itself", async () => {
    const owner = await as("user", "public");
    const id = await makeThread("echo mine", { as: owner });
    const token = await shareAs(owner, id, "internal");
    reader(token);
    await waitFor(() => expect(router.replace).toHaveBeenCalledWith(`/threads/${id}`));
  });

  it("is one neutral page for every link that does not work, and says nothing of the thread", async () => {
    const owner = await as("user", "public");
    const id = await makeThread("echo private words", { as: owner });
    const token = await shareAs(owner, id, "internal");
    // an internal link, read as anybody with no sign-in to go to
    await as("user", "public", false);
    const go = vi.spyOn(navigation, "go").mockImplementation(() => {});
    reader(token);
    expect(await screen.findByRole("heading", { name: "This link does not work" })).toBeTruthy();
    expect(document.body.textContent).not.toContain("private words");
    expect(go).not.toHaveBeenCalled();
    cleanup();

    // a link nobody holds, and one that cannot be a link, read by a signed-in person: the same page
    await as("admin", "public");
    for (const bad of ["A".repeat(43), "nope"]) {
      reader(bad);
      expect(await screen.findByRole("heading", { name: "This link does not work" })).toBeTruthy();
      cleanup();
    }
  });

  it("sends the browser to sign in, coming back to the link, when the link is not public and a sign-in is built in", async () => {
    vi.stubEnv("NEXT_PUBLIC_SIGN_IN_PATH", "/oauth2/start");
    const owner = await as("user", "public");
    const id = await makeThread("echo only for us", { as: owner });
    const token = await shareAs(owner, id, "internal");
    await as("user", "public", false);
    window.history.pushState({}, "", `/s/${token}`);
    const go = vi.spyOn(navigation, "go").mockImplementation(() => {});
    reader(token);
    await waitFor(() => expect(go).toHaveBeenCalledTimes(1));
    expect(go).toHaveBeenCalledWith(`/oauth2/start?rd=${encodeURIComponent(`/s/${token}`)}`);
    expect(document.body.textContent).not.toContain("only for us");
  });

  it("turns into the neutral page when the link is taken down while it is open", async () => {
    const owner = await as("user", "public");
    const id = await makeThread("echo going away", { as: owner });
    const token = await shareAs(owner, id, "internal");
    await as("admin", "public");
    reader(token);
    expect(await screen.findByText("Shared conversation, read only")).toBeTruthy();
    await waitFor(() => expect(stateBadge().textContent).toBe("Done"));
    await realFetch(`${base}/api/threads/${id}/share`, {
      method: "DELETE",
      headers: { cookie: owner },
    });
    expect(await screen.findByRole("heading", { name: "This link does not work" })).toBeTruthy();
  });
});

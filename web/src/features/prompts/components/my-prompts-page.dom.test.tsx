// @vitest-environment jsdom
import type { AddressInfo } from "node:net";
import {
  act,
  cleanup,
  configure,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { createMockServer } from "../../../../mock/server";

/*
 * The page "My prompts" in jsdom against the mock orchestrator, which holds the contract's own rules (docs/api/prompts-v1.md):
 * create, list, edit, delete and copy as a person does them, a shared prompt read only, the sharing control of slice P4
 * (Share, Change sharing, Stop sharing and the dialog's refusals), and nothing asked of `/api/prompts` where the
 * configuration has no library. The cookie of the mock's session is not sent from jsdom, so every test shares one library
 * and one profile: each starts by emptying it and by being a plain `user`.
 */
vi.mock("next/link", () => ({
  default: ({ href, children, ...rest }: { href: string; children: React.ReactNode }) => (
    <a href={href} {...rest}>
      {children}
    </a>
  ),
}));
vi.mock("@/features/session/components/account-menu", () => ({ PROMPTS_LABEL: "My prompts" }));

let MyPromptsPage: typeof import("./my-prompts-page").MyPromptsPage;
let resetUiConfig: typeof import("@/features/chat/hooks/use-ui-config").resetUiConfig;
let resetMe: typeof import("@/features/me/hooks/use-me").resetMe;

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the page made: `METHOD path`. */
let calls: string[] = [];
/** The body of every write the page made, as it was sent. */
let sent: { method: string; path: string; body: string }[] = [];
/** Answer `GET /api/config` with this instead of the mock's. */
let configOverride: object | undefined;
/** Answer a request with this instead of the mock's (the path and the method decide). */
let answerWith: ((method: string, path: string) => Response | undefined) | undefined;
/** Holds a request until its promise is settled (the path of the request decides). */
let hold: ((method: string, pathname: string) => Promise<void> | undefined) | undefined;

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
    calls.push(`${method} ${url.pathname}`);
    if (method !== "GET" && method !== "HEAD") {
      const body = request ? await request.clone().text() : String(init?.body ?? "");
      if (body !== "") sent.push({ method, path: url.pathname, body });
    }
    if (configOverride && method === "GET" && url.pathname === "/api/config") {
      return new Response(JSON.stringify(configOverride), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    const replaced = answerWith?.(method, url.pathname);
    if (replaced) return replaced;
    const gate = hold?.(method, url.pathname);
    if (gate) await gate;
    return realFetch(abs(input) as RequestInfo, init);
  }) as typeof fetch;
  ({ MyPromptsPage } = await import("./my-prompts-page"));
  ({ resetUiConfig } = await import("@/features/chat/hooks/use-ui-config"));
  ({ resetMe } = await import("@/features/me/hooks/use-me"));
});
afterAll(async () => {
  globalThis.fetch = realFetch;
  globalThis.Request = RealRequest;
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
});

const api = async (method: string, path: string, body?: unknown) =>
  realFetch(`${base}${path}`, {
    method,
    headers: { "Content-Type": "application/json" },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });

/** Who the mock's session is (`GET /api/me`); `resetMe` then makes the page read it again. */
const asProfile = async (profile: "user" | "prompt-sharer") => {
  const res = await api("POST", `/__mock/config?me=${profile}`);
  expect(res.status, await res.clone().text()).toBe(204);
};

beforeEach(async () => {
  calls = [];
  sent = [];
  configOverride = undefined;
  answerWith = undefined;
  hold = undefined;
  resetUiConfig();
  await asProfile("user");
  resetMe();
  // the mock keeps one library for this file: empty what the person owns (what others shared stays, and tests find it by name)
  const list = (await (await api("GET", "/api/prompts")).json()) as {
    items: { id: string; own: boolean; revision: string }[];
  };
  for (const p of list.items) if (p.own) await api("DELETE", `/api/prompts/${p.id}`);
  calls = [];
  sent = [];
});
afterEach(cleanup);

const shareWithEveryone = async (name: string, title: string, body = "Shared text") => {
  const res = await api("POST", "/__mock/prompts/shared", [{ name, title, body, audience: ["*"] }]);
  // the mock refuses a prompt its rules refuse, and says so: a seed that did not take must not pass quietly
  expect(res.status, await res.clone().text()).toBe(204);
};

const open = async () => {
  render(<MyPromptsPage />);
  await screen.findByRole("heading", { name: /^Your prompts/ });
};

const fill = (label: string | RegExp, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });

describe("My prompts, against the mock's own rules", () => {
  it("create, list, edit and delete as a person does them", async () => {
    await open();
    expect(screen.getByText("You have no prompts yet.")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "New prompt" }));
    fill("Title", "Review a change");
    // the name follows the title until it is written
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("review-a-change");
    fill("Text", "Review for {{focus}}.");
    fireEvent.click(screen.getByRole("button", { name: "Add a question" }));
    fill(/^Question$/, "What focus?");
    expect((screen.getByLabelText("Name in the text") as HTMLInputElement).value).toBe(
      "what-focus",
    );
    fill("Name in the text", "focus");
    fireEvent.change(screen.getByLabelText("Kind"), { target: { value: "text" } });
    fireEvent.click(screen.getByRole("button", { name: "Save prompt" }));

    await screen.findByText("Saved “Review a change”.");
    const row = (await screen.findByRole("heading", { name: "Review a change", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    expect(within(row).getByText("/review-a-change")).toBeTruthy();
    expect(within(row).getByText("Asks 1 question")).toBeTruthy();
    expect(screen.getByText("(1 of 100)")).toBeTruthy();
    const posted = calls.filter((c) => c === "POST /api/prompts");
    expect(posted).toHaveLength(1);

    // an edit renames it and changes its text; only what changed is sent
    fireEvent.click(within(row).getByRole("button", { name: "Edit “Review a change”" }));
    await screen.findByRole("heading", { name: "Edit “Review a change”" });
    fill("Title", "Review it");
    fill("Text", "Review for {{focus}} now.");
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText("Saved “Review it”.");
    const stored = (await (await api("GET", "/api/prompts")).json()) as {
      items: { id: string; title: string; revision: string }[];
    };
    const only = stored.items[0];
    expect(only?.title).toBe("Review it");
    const full = (await (await api("GET", `/api/prompts/${only?.id}`)).json()) as {
      body: string;
      variables: { id: string; type: string }[];
      name: string;
    };
    expect(full.body).toBe("Review for {{focus}} now.");
    expect(full.name).toBe("review-a-change");
    expect(full.variables).toEqual([{ id: "focus", question: "What focus?", type: "text" }]);

    // deleting asks first, then removes it
    const edited = (await screen.findByRole("heading", { name: "Review it", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    fireEvent.click(within(edited).getByRole("button", { name: "Delete “Review it”" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText(/This cannot be undone/)).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    // (a modal dialog hides the page from the accessibility tree while it is open, so the page is read by its text)
    await screen.findByText("You have no prompts yet.");
    expect(calls).toContain(`DELETE /api/prompts/${only?.id}`);
  });

  it("opening two prompts in quick succession shows the last one, whichever answer comes first", async () => {
    for (const [name, title] of [
      ["first", "First prompt"],
      ["second", "Second prompt"],
    ] as const) {
      const made = await api("POST", "/api/prompts", { name, title, body: "Text" });
      expect(made.status).toBe(201);
    }
    const list = (await (await api("GET", "/api/prompts")).json()) as {
      items: { id: string; title: string }[];
    };
    const idOf = (title: string) => list.items.find((p) => p.title === title)?.id;
    const gates = new Map<string, () => void>();
    hold = (method, pathname) => {
      if (method !== "GET" || !pathname.startsWith("/api/prompts/")) return undefined;
      return new Promise<void>((resolve) => gates.set(pathname, resolve));
    };
    await open();
    // both buttons are pressed before the page has drawn the first answer's wait
    const first = screen.getByRole("button", { name: "Edit “First prompt”" });
    const second = screen.getByRole("button", { name: "Edit “Second prompt”" });
    act(() => {
      first.click();
      second.click();
    });
    await waitFor(() => expect(gates.size).toBe(2));
    // the later opening answers first, then the earlier one
    gates.get(`/api/prompts/${idOf("Second prompt")}`)?.();
    await screen.findByRole("heading", { name: "Edit “Second prompt”" });
    gates.get(`/api/prompts/${idOf("First prompt")}`)?.();
    await new Promise((r) => setTimeout(r, 100));
    expect(screen.queryByRole("heading", { name: "Edit “First prompt”" })).toBeNull();
    expect(screen.getByRole("heading", { name: "Edit “Second prompt”" })).toBeTruthy();
  });

  it("a refused save puts the words at the field and the focus there", async () => {
    await open();
    fireEvent.click(screen.getByRole("button", { name: "New prompt" }));
    fill("Title", "Broken");
    fill("Name", "Not A Name");
    fill("Text", "Hello");
    fireEvent.click(screen.getByRole("button", { name: "Save prompt" }));
    const field = screen.getByLabelText("Name");
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(field.getAttribute("aria-invalid")).toBe("true");
    const described = document.getElementById(
      field.getAttribute("aria-describedby")?.split(" ").pop() ?? "",
    );
    expect(described?.textContent).toMatch(/1 to 32 lowercase letters/);
  });

  it("a shared prompt is read only, names no author, and copies into the person's own", async () => {
    await shareWithEveryone("deploy-notes", "Deploy notes", "Write the <b>notes</b> now");
    await open();
    const shared = (await screen.findByRole("heading", { name: "Deploy notes", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    expect(within(shared).getByText("Shared with you")).toBeTruthy();
    expect(within(shared).queryByRole("button", { name: /Edit|Delete/ })).toBeNull();
    expect(shared.textContent).not.toMatch(/@|owner|author|by /i);

    fireEvent.click(within(shared).getByRole("button", { name: "View “Deploy notes”" }));
    const body = await screen.findByText(/Write the <b>notes<\/b>/);
    // drawn as text: the tag is characters, not an element
    expect(body.querySelector("b")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Copy to my prompts" }));
    await screen.findByRole("heading", { name: "Copy to my prompts" });
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("Deploy notes");
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("deploy-notes");
    fireEvent.click(screen.getByRole("button", { name: "Save prompt" }));
    await screen.findByText("Saved “Deploy notes”.");
    const own = screen.getByRole("list", { name: "Your prompts" });
    expect(within(own).getByText("/deploy-notes")).toBeTruthy();
    // a copy is private: no audience went with it
    const listed = (await (await api("GET", "/api/prompts?scope=mine")).json()) as {
      items: { audience?: string[] }[];
    };
    expect(listed.items[0]?.audience ?? []).toEqual([]);
  });

  it("a name the person already uses is refused at the name when a shared prompt is copied", async () => {
    await shareWithEveryone("review", "Their review");
    await api("POST", "/api/prompts", { name: "review", title: "My review", body: "x" });
    await open();
    fireEvent.click(
      await screen.findByRole("button", { name: "Copy to my prompts: “Their review”" }),
    );
    await screen.findByRole("heading", { name: "Copy to my prompts" });
    fireEvent.click(screen.getByRole("button", { name: "Save prompt" }));
    await waitFor(() => expect(document.activeElement).toBe(screen.getByLabelText("Name")));
    expect(
      screen.getByText("You already have a prompt named /review. Choose another name."),
    ).toBeTruthy();
  });

  it("a stale edit keeps the typed text, and Load the current version starts again from the current one", async () => {
    const made = (await (
      await api("POST", "/api/prompts", { name: "plan", title: "Plan", body: "Plan it" })
    ).json()) as { id: string; revision: string };
    await open();
    fireEvent.click(await screen.findByRole("button", { name: "Edit “Plan”" }));
    await screen.findByRole("heading", { name: "Edit “Plan”" });
    // another client changes it behind the page's back
    await api("PATCH", `/api/prompts/${made.id}`, { revision: made.revision, title: "Plan v2" });
    fill("Title", "My typed title");
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText(/Your changes are still here/);
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("My typed title");
    // saving again keeps the old revision, so it is stale again and never overwrites
    fireEvent.click(screen.getByRole("button", { name: "Save changes" }));
    await screen.findByText(/Your changes are still here/);
    const still = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as { title: string };
    expect(still.title).toBe("Plan v2");

    fireEvent.click(screen.getByRole("button", { name: "Load the current version" }));
    await waitFor(() =>
      expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("Plan v2"),
    );
    expect(screen.queryByText(/Your changes are still here/)).toBeNull();
  });

  it("without prompts in the configuration nothing is asked of /api/prompts", async () => {
    configOverride = { ui: { showDescriptions: true } };
    render(<MyPromptsPage />);
    await screen.findByText("This deployment does not offer a prompt library.");
    expect(calls.filter((c) => c.includes("/api/prompts"))).toEqual([]);
    expect(screen.queryByRole("button", { name: "New prompt" })).toBeNull();
  });

  it("nothing is kept on the device", async () => {
    const set = vi.spyOn(Storage.prototype, "setItem");
    await open();
    fireEvent.click(screen.getByRole("button", { name: "New prompt" }));
    fill("Title", "Draft");
    fill("Text", "Hello");
    // the app's own hint that a session was had is not a prompt; anything else written would be
    expect(set.mock.calls.filter(([key]) => key !== "another-agentic.had-session")).toEqual([]);
    expect(sessionStorage.length).toBe(0);
    set.mockRestore();
  });
});

/** A prompt of the person's own, made behind the page's back, as another tab would. */
const make = async (body: unknown) => {
  const res = await api("POST", "/api/prompts", body);
  expect(res.status, await res.clone().text()).toBe(201);
  return (await res.json()) as { id: string; revision: string };
};

const patchOf = () => sent.filter((s) => s.method === "PATCH");

const audienceFieldOf = (dialog: HTMLElement) =>
  within(dialog).getByLabelText("Roles who can read it") as HTMLInputElement;

/** The page, as a holder of `prompts.share`, with one private prompt of theirs. */
const openAsSharer = async (prompt: unknown = { name: "plan", title: "Plan", body: "Plan it" }) => {
  await asProfile("prompt-sharer");
  resetMe();
  const made = await make(prompt);
  await open();
  return made;
};

describe("the sharing control (P4)", () => {
  it("offers no share control without prompts.share, and Stop sharing on what was shared before it was lost", async () => {
    await asProfile("prompt-sharer");
    resetMe();
    await make({ name: "priv", title: "Private one", body: "x" });
    await make({ name: "shared", title: "Shared one", body: "y", audience: ["user"] });
    // the roles change elsewhere: the page reads who the person is now
    await asProfile("user");
    resetMe();
    await open();

    // the frozen control is the proof that `getMe` was read: only then does a private prompt offer nothing
    const stop = await screen.findByRole("button", { name: "Stop sharing “Shared one”" });
    expect(screen.queryByRole("button", { name: "Share “Private one”" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Change sharing “Shared one”" })).toBeNull();
    fireEvent.click(stop);
    await screen.findByText("“Shared one” is no longer shared.");
    // the Stop control is gone, so the row's Edit takes the focus
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Edit “Shared one”" }),
      ),
    );

    const listed = (await (await api("GET", "/api/prompts?scope=mine")).json()) as {
      items: { title: string; audience?: string[] }[];
    };
    expect(listed.items.find((p) => p.title === "Shared one")?.audience ?? []).toEqual([]);
    // and a private prompt offers nothing to a person who may not share, though its row is still there with the control it
    // should have: a `null` check on a row that vanished proves nothing
    expect(screen.getByRole("button", { name: "Edit “Private one”" })).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: /^(Share|Change sharing|Stop sharing)/ }),
    ).toBeNull();
  });

  it("says Stopping… on the row whose Stop sharing is in flight", async () => {
    await asProfile("prompt-sharer");
    resetMe();
    await make({ name: "shared", title: "Shared one", body: "y", audience: ["user"] });
    await asProfile("user");
    resetMe();
    await open();

    let release!: () => void;
    hold = (method) =>
      method === "PATCH"
        ? new Promise<void>((resolve) => {
            release = resolve;
          })
        : undefined;
    const stop = await screen.findByRole("button", { name: "Stop sharing “Shared one”" });
    fireEvent.click(stop);
    await waitFor(() => expect(stop.textContent).toBe("Stopping…"));
    expect((stop as HTMLButtonElement).disabled).toBe(true);
    release();
    await screen.findByText("“Shared one” is no longer shared.");
  });

  it("a 404 on the row's Stop sharing says so and puts the focus on the list", async () => {
    await asProfile("prompt-sharer");
    resetMe();
    const made = await make({ name: "shared", title: "Shared one", body: "y", audience: ["user"] });
    await asProfile("user");
    resetMe();
    await open();
    const stop = await screen.findByRole("button", { name: "Stop sharing “Shared one”" });
    await api("DELETE", `/api/prompts/${made.id}`);
    fireEvent.click(stop);
    await screen.findByText("This prompt no longer exists.");
    await screen.findByText("You have no prompts yet.");
    await waitFor(() =>
      expect(document.activeElement).toBe(document.getElementById("my-prompts-list")),
    );
  });

  it("a stale Stop sharing says so, reads the list again, and a second press stops it", async () => {
    await asProfile("prompt-sharer");
    resetMe();
    const made = await make({
      name: "shared",
      title: "Shared one",
      body: "y",
      audience: ["user"],
    });
    await asProfile("user");
    resetMe();
    await open();
    const row = (await screen.findByRole("heading", { name: "Shared one", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    const stop = within(row).getByRole("button", { name: "Stop sharing “Shared one”" });

    // another tab (the owner, still holding prompts.share) changes the title: the row's revision is stale
    await asProfile("prompt-sharer");
    const other = await api("PATCH", `/api/prompts/${made.id}`, {
      revision: made.revision,
      title: "Shared one, renamed",
    });
    expect(other.status).toBe(200);
    await asProfile("user");
    fireEvent.click(stop);
    await screen.findByText(/It was changed since you opened this page/);

    const fresh = (
      await screen.findByRole("heading", { name: "Shared one, renamed", level: 3 })
    ).closest("li") as HTMLElement;
    const freshStop = within(fresh).getByRole("button", {
      name: "Stop sharing “Shared one, renamed”",
    });
    await waitFor(() => expect(document.activeElement).toBe(freshStop));
    fireEvent.click(freshStop);
    await screen.findByText("“Shared one, renamed” is no longer shared.");
    const held = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as {
      audience: string[];
    };
    expect(held.audience).toEqual([]);
  });

  it("offers Share on a private prompt, Change sharing on a shared one, and nothing on one shared with you", async () => {
    await shareWithEveryone("theirs", "Theirs");
    await asProfile("prompt-sharer");
    resetMe();
    await make({ name: "plan", title: "Plan", body: "Plan it" });
    await make({ name: "shared", title: "Shared one", body: "y", audience: ["user"] });
    await open();

    // the positive control is the proof that `getMe` was read
    await screen.findByRole("button", { name: "Share “Plan”" });
    expect(screen.getByRole("button", { name: "Change sharing “Shared one”" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: /^Stop sharing/ })).toBeNull();
    const shared = (await screen.findByRole("heading", { name: "Theirs", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    expect(
      within(shared).queryByRole("button", { name: /^(Share|Change sharing|Stop sharing)/ }),
    ).toBeNull();
  });

  it("the dialog sends the revision and the audience and nothing else, and the row says who can read it", async () => {
    const made = await openAsSharer();
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByRole("heading", { name: "Share “Plan”" })).toBeTruthy();
    expect(within(dialog).getByText("/plan")).toBeTruthy();
    const field = audienceFieldOf(dialog);
    expect(field.value).toBe("");
    expect(within(dialog).getByText("Only you can read it.")).toBeTruthy();
    expect(field.getAttribute("aria-describedby")).not.toMatch(/-comma/);

    fireEvent.change(field, { target: { value: " user , prompt-team, user " } });
    expect(
      within(dialog).getByText("Everyone with one of these roles can read it: user, prompt-team."),
    ).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText("“Plan” is shared with prompt-team, user.");
    // the focus returns to the row's control, which now says Change sharing
    await waitFor(() =>
      expect(document.activeElement).toBe(
        screen.getByRole("button", { name: "Change sharing “Plan”" }),
      ),
    );

    const patches = patchOf();
    expect(patches).toHaveLength(1);
    const body = JSON.parse(patches[0]?.body ?? "{}") as Record<string, unknown>;
    // the values as typed, once each, and nothing else: no title, no text, no questions
    expect(Object.keys(body).sort()).toEqual(["audience", "revision"]);
    expect(body).toEqual({ revision: made.revision, audience: ["user", "prompt-team"] });
    const row = (await screen.findByRole("heading", { name: "Plan", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    expect(within(row).getByText(/Shared with: prompt-team, user/)).toBeTruthy();
    // the server's order is the one shown, whatever order was typed
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("returns the focus to the share control after Cancel", async () => {
    await openAsSharer();
    const share = await screen.findByRole("button", { name: "Share “Plan”" });
    fireEvent.click(share);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(document.activeElement).toBe(share));
  });

  it("Everyone is * alone, and the row says so", async () => {
    await openAsSharer();
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);
    fireEvent.change(field, { target: { value: "*" } });
    expect(within(dialog).getByText("Everyone can read it.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText("“Plan” is shared with Everyone.");
    expect(JSON.parse(patchOf()[0]?.body ?? "{}")).toMatchObject({ audience: ["*"] });
    const row = (await screen.findByRole("heading", { name: "Plan", level: 3 })).closest(
      "li",
    ) as HTMLElement;
    expect(within(row).getByText(/Shared with: Everyone/)).toBeTruthy();
    expect(within(row).getByRole("button", { name: "Change sharing “Plan”" })).toBeTruthy();
  });

  it("bad_audience puts the words at the field and the focus there", async () => {
    await openAsSharer();
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);

    // `*` beside a role, then a role with a space: the server judges both, and neither is sent as valid
    const save = within(dialog).getByRole("button", { name: "Save" });
    for (const typed of ["* , user", "prompt team"]) {
      fireEvent.change(field, { target: { value: typed } });
      save.focus();
      fireEvent.click(save);
      await waitFor(() => expect(field.getAttribute("aria-invalid")).toBe("true"));
      expect(document.activeElement).toBe(field);
      const described = document.getElementById(
        field.getAttribute("aria-describedby")?.split(" ").pop() ?? "",
      );
      expect(described?.textContent).toMatch(/A role is 1 to 64 characters with no spaces/);
      // the typed roles stay
      expect(field.value).toBe(typed);
    }

    fireEvent.change(field, { target: { value: "prompt-team" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText("“Plan” is shared with prompt-team.");
  });

  it("a stale share keeps the roles typed, and Load the current version shows the current audience", async () => {
    const made = await openAsSharer();
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);
    fireEvent.change(field, { target: { value: "user" } });
    // another tab shares it with a team
    const other = await api("PATCH", `/api/prompts/${made.id}`, {
      revision: made.revision,
      audience: ["team"],
    });
    expect(other.status).toBe(200);

    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText(/This prompt was changed elsewhere since you opened it/);
    expect(field.value).toBe("user");
    // again: still stale, never an overwrite
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText(/Your changes are still here/);
    const held = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as {
      audience: string[];
    };
    expect(held.audience).toEqual(["team"]);

    fireEvent.click(within(dialog).getByRole("button", { name: "Load the current version" }));
    await waitFor(() => expect(field.value).toBe("team"));
    await waitFor(() => expect(document.activeElement).toBe(field));
    expect(screen.queryByText(/changed elsewhere since you opened it/)).toBeNull();
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText("“Plan” is shared with team.");
  });

  it("403 forbidden offers Stop sharing, the only write the person may still make", async () => {
    const made = await openAsSharer({
      name: "plan",
      title: "Plan",
      body: "Plan it",
      audience: ["user"],
    });
    fireEvent.click(await screen.findByRole("button", { name: "Change sharing “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);
    // the roles change behind the page's back: it still offers what it may no longer do
    await asProfile("user");
    fireEvent.change(field, { target: { value: "user, prompt-team" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText(/Your roles no longer let you share prompts/);
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeTruthy();

    fireEvent.click(within(dialog).getByRole("button", { name: "Stop sharing" }));
    await screen.findByText("“Plan” is no longer shared.");
    const held = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as {
      audience: string[];
    };
    expect(held.audience).toEqual([]);
  });

  it("a role the field cannot hold (one with a comma) is kept by a Save that changes nothing, and said there", async () => {
    const made = await openAsSharer({
      name: "plan",
      title: "Plan",
      body: "Plan it",
      audience: ["a,b", "user"],
    });
    fireEvent.click(await screen.findByRole("button", { name: "Change sharing “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);
    // the contract allows a comma in a role value (every byte 0x21 to 0x7E); the field separates roles with one
    expect(field.value).toBe("a,b, user");
    expect(within(dialog).getByText(/One of these roles holds a comma/)).toBeTruthy();
    expect(field.getAttribute("aria-describedby")).toMatch(/-comma/);

    // opening it and pressing Save sends the audience it has, not the two roles the field would be read as
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText("“Plan” is shared with a,b, user.");
    const body = JSON.parse(patchOf()[0]?.body ?? "{}") as Record<string, unknown>;
    expect(body).toEqual({ revision: made.revision, audience: ["a,b", "user"] });
    const held = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as {
      audience: string[];
    };
    expect(held.audience).toEqual(["a,b", "user"]);

    // once the field is changed, the roles are what was typed: the dialog said so
    fireEvent.click(await screen.findByRole("button", { name: "Change sharing “Plan”" }));
    const again = await screen.findByRole("dialog");
    const other = audienceFieldOf(again);
    fireEvent.change(other, { target: { value: "a,b, prompt-team" } });
    expect(within(again).queryByText(/One of these roles holds a comma/)).toBeNull();
    fireEvent.click(within(again).getByRole("button", { name: "Save" }));
    await screen.findByText("“Plan” is shared with a, b, prompt-team.");
    const split = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as {
      audience: string[];
    };
    expect(split.audience).toEqual(["a", "b", "prompt-team"]);
  });

  it("Load the current version updates a stored comma role before the next Save", async () => {
    const made = await openAsSharer({
      name: "comma",
      title: "Comma",
      body: "x",
      audience: ["a,b"],
    });
    fireEvent.click(await screen.findByRole("button", { name: "Change sharing “Comma”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);
    // another tab adds a role: the revision the dialog holds is stale
    const other = await api("PATCH", `/api/prompts/${made.id}`, {
      revision: made.revision,
      audience: ["a,b", "user"],
    });
    expect(other.status).toBe(200);

    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText(/changed elsewhere/);
    fireEvent.click(within(dialog).getByRole("button", { name: "Load the current version" }));
    await waitFor(() => expect(field.value).toBe("a,b, user"));
    expect(document.activeElement).toBe(field);
    expect(within(dialog).getByText(/One of these roles holds a comma/)).toBeTruthy();

    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(patchOf()).toHaveLength(2));
    const body = JSON.parse(patchOf()[1]?.body ?? "{}") as { audience: string[] };
    expect(body.audience).toEqual(["a,b", "user"]);
    const held = (await (await api("GET", `/api/prompts/${made.id}`)).json()) as {
      audience: string[];
    };
    expect(held.audience).toEqual(["a,b", "user"]);
  });

  it("disables the field while a save is in flight", async () => {
    await openAsSharer();
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    const field = audienceFieldOf(dialog);
    fireEvent.change(field, { target: { value: "user" } });

    let release!: () => void;
    hold = (method) =>
      method === "PATCH"
        ? new Promise<void>((resolve) => {
            release = resolve;
          })
        : undefined;
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(field.disabled).toBe(true));
    release();
    await screen.findByText("“Plan” is shared with user.");
  });

  it("a 404 says the prompt is gone, closes the dialog and reads the list again", async () => {
    const made = await openAsSharer();
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    await api("DELETE", `/api/prompts/${made.id}`);
    fireEvent.change(audienceFieldOf(dialog), { target: { value: "user" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));

    await screen.findByText("This prompt no longer exists.");
    expect(screen.queryByRole("dialog")).toBeNull();
    // the list was read again, and the row is gone
    await screen.findByText("You have no prompts yet.");
    await waitFor(() =>
      expect(document.activeElement).toBe(document.getElementById("my-prompts-list")),
    );
  });

  it("a 503 says the write may or may not have been made and keeps the dialog open", async () => {
    await openAsSharer();
    answerWith = (method) =>
      method === "PATCH"
        ? new Response(JSON.stringify({ title: "Unavailable", status: 503 }), {
            status: 503,
            headers: { "content-type": "application/problem+json" },
          })
        : undefined;
    fireEvent.click(await screen.findByRole("button", { name: "Share “Plan”" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(audienceFieldOf(dialog), { target: { value: "user" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await screen.findByText(
      /could not be saved just now, and the change may or may not have been made/,
    );
    expect(within(dialog).getByRole("button", { name: "Save" })).toBeTruthy();
  });
});

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
 * create, list, edit, delete and copy as a person does them, a shared prompt read only, and nothing asked of `/api/prompts` where
 * the configuration has no library. The cookie of the mock's session is not sent from jsdom, so every test shares one library:
 * each starts by emptying it.
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

const server = createMockServer({ stepMs: 5, keepaliveMs: 1000 });
let base = "";
const realFetch = globalThis.fetch;
const RealRequest = globalThis.Request;
/** Every request the page made: `METHOD path`. */
let calls: string[] = [];
/** Answer `GET /api/config` with this instead of the mock's. */
let configOverride: object | undefined;
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
    if (configOverride && method === "GET" && url.pathname === "/api/config") {
      return new Response(JSON.stringify(configOverride), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    }
    const gate = hold?.(method, url.pathname);
    if (gate) await gate;
    return realFetch(abs(input) as RequestInfo, init);
  }) as typeof fetch;
  ({ MyPromptsPage } = await import("./my-prompts-page"));
  ({ resetUiConfig } = await import("@/features/chat/hooks/use-ui-config"));
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

beforeEach(async () => {
  calls = [];
  configOverride = undefined;
  hold = undefined;
  resetUiConfig();
  // the mock keeps one library for this file: empty what the person owns (what others shared stays, and tests find it by name)
  const list = (await (await api("GET", "/api/prompts")).json()) as {
    items: { id: string; own: boolean; revision: string }[];
  };
  for (const p of list.items) if (p.own) await api("DELETE", `/api/prompts/${p.id}`);
  calls = [];
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

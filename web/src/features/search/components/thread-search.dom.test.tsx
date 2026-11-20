// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ApiThread, ApiThreadHit } from "@/lib/api/types";
import { useThreadSearch } from "../hooks/use-thread-search";
import type { SearchOption } from "../lib/options";
import { SearchBox, SearchResults } from "./thread-search";

/*
 * The box and its listbox (ADR 0068, web/DESIGN.md "Search"): roles, keys, highlighting from the offsets as text nodes, and the states in
 * plain words. The API client answers what each test sets; the hook, the box and the results are the real ones.
 */

let answer: () => Promise<unknown>;
vi.mock("@/lib/api/client", () => ({ api: { POST: () => answer() } }));
vi.mock("@/features/chat/hooks/use-ui-config", () => ({
  useSearchConfig: () => ({ maxQueryLength: 200, maxTerms: 8 }),
}));

const thread = (id: string, title: string, extra: Partial<ApiThread> = {}): ApiThread => ({
  id,
  owner: "dev@example.com",
  title,
  target: { agentId: "coder" },
  state: "done",
  createdAt: "2026-10-02T10:00:00Z",
  updatedAt: "2026-10-02T10:00:00Z",
  lastSeq: 9,
  ...extra,
});

const hits: ApiThreadHit[] = [
  {
    thread: thread("t-1", "Fix the login page"),
    rank: 2,
    messageHits: 3,
    hits: [
      {
        field: "title",
        snippet: {
          text: "Fix the login page",
          cutStart: false,
          cutEnd: false,
          matches: [{ start: 8, end: 13 }],
        },
      },
      {
        field: "message",
        seq: 7,
        role: "agent",
        at: "2026-10-02T10:00:00Z",
        snippet: {
          text: "the login form now validates <img src=x onerror=alert(1)> input",
          cutStart: true,
          cutEnd: true,
          matches: [{ start: 4, end: 9 }],
        },
      },
    ],
  },
  {
    thread: thread("t-2", "Plan the release", { archived: true }),
    rank: 0,
    messageHits: 1,
    hits: [
      {
        field: "message",
        seq: 3,
        role: "person",
        at: "2026-10-01T10:00:00Z",
        snippet: {
          text: "login again?",
          cutStart: false,
          cutEnd: false,
          matches: [{ start: 0, end: 5 }],
        },
      },
    ],
  },
];

const ok = (results: ApiThreadHit[], terms = ["login"]) => ({
  data: { query: { terms }, results },
  response: { status: 200, headers: new Headers() },
});

const opened: SearchOption[] = [];

function Harness() {
  const search = useThreadSearch();
  return (
    <TooltipProvider>
      <SearchBox search={search} onOpen={(o) => opened.push(o)} />
      {search.active ? <SearchResults search={search} onOpen={(o) => opened.push(o)} /> : null}
    </TooltipProvider>
  );
}

const box = () => screen.getByRole("combobox", { name: "Search chats" }) as HTMLInputElement;
const settle = () => act(() => vi.advanceTimersByTimeAsync(300));
const type = async (text: string) => {
  fireEvent.change(box(), { target: { value: text } });
  await settle();
};

beforeEach(() => {
  vi.useFakeTimers();
  opened.length = 0;
  answer = async () => ok(hits);
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };
  Element.prototype.scrollIntoView = vi.fn();
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("the box", () => {
  it("is a combobox in a search landmark, and the list appears only with hits", async () => {
    render(<Harness />);
    // the `<search>` element: its landmark role is the browser's (jsdom does not map it)
    expect(document.querySelector("search")).not.toBeNull();
    expect(box().getAttribute("aria-expanded")).toBe("false");
    await type("login");
    expect(box().getAttribute("aria-expanded")).toBe("true");
    const list = screen.getByRole("listbox", { name: "Search results" });
    expect(box().getAttribute("aria-controls")).toBe(list.id);
    expect(within(list).getAllByRole("option")).toHaveLength(3);
  });

  it("draws the matches as marks of text nodes, and the text of a hit is never markup", async () => {
    render(<Harness />);
    await type("login");
    const marks = [...document.querySelectorAll("mark")].map((m) => m.textContent);
    expect(marks).toEqual(["login", "login", "login"]);
    // the markup the message holds is on the page as words, and no element was made from it
    expect(document.querySelector("img")).toBeNull();
    expect(screen.getByRole("listbox").textContent).toContain("<img src=x onerror=alert(1)>");
  });

  it("marks the archived thread and says how many messages more match", async () => {
    render(<Harness />);
    await type("login");
    const list = screen.getByRole("listbox");
    expect(list.textContent).toContain("Archived");
    expect(list.textContent).toContain("and 2 more messages");
  });

  it("opens a message at its seq and a title at the thread", async () => {
    render(<Harness />);
    await type("login");
    const options = screen.getAllByRole("option");
    expect(options.map((o) => o.getAttribute("href"))).toEqual([
      "/threads/t-1",
      "/threads/t-1#m-7",
      "/threads/t-2#m-3",
    ]);
  });
});

describe("the keys", () => {
  it("walks the hits with the arrows, keeping the focus in the box, and Enter opens the one it is on", async () => {
    render(<Harness />);
    await type("login");
    const options = screen.getAllByRole("option");
    expect(box().getAttribute("aria-activedescendant")).toBeNull();
    fireEvent.keyDown(box(), { key: "ArrowDown" });
    expect(box().getAttribute("aria-activedescendant")).toBe(options[0]?.id);
    fireEvent.keyDown(box(), { key: "ArrowDown" });
    fireEvent.keyDown(box(), { key: "ArrowDown" });
    expect(box().getAttribute("aria-activedescendant")).toBe(options[2]?.id);
    expect(options[2]?.getAttribute("aria-selected")).toBe("true");
    fireEvent.keyDown(box(), { key: "ArrowDown" });
    expect(box().getAttribute("aria-activedescendant")).toBe(options[0]?.id);
    fireEvent.keyDown(box(), { key: "ArrowUp" });
    fireEvent.keyDown(box(), { key: "Enter" });
    expect(opened.map((o) => o.href)).toEqual(["/threads/t-2#m-3"]);
  });

  it("opens the first hit on Enter when none was walked to", async () => {
    render(<Harness />);
    await type("login");
    fireEvent.keyDown(box(), { key: "Enter" });
    expect(opened.map((o) => o.href)).toEqual(["/threads/t-1"]);
  });

  it("clears the box on the first Escape and lets go of the focus on the second", async () => {
    const before = document.createElement("button");
    document.body.append(before);
    render(<Harness />);
    await type("login");
    box().focus();
    fireEvent.keyDown(box(), { key: "Escape" });
    expect(box().value).toBe("");
    expect(screen.queryByRole("listbox")).toBeNull();
    fireEvent.keyDown(box(), { key: "Escape" });
    expect(document.activeElement).not.toBe(box());
    before.remove();
  });

  it("clicks open a hit but leave a modified click to the browser", async () => {
    render(<Harness />);
    await type("login");
    fireEvent.click(screen.getAllByRole("option")[1] as Element);
    expect(opened.map((o) => o.href)).toEqual(["/threads/t-1#m-7"]);
    fireEvent.click(screen.getAllByRole("option")[2] as Element, { ctrlKey: true });
    expect(opened).toHaveLength(1);
  });
});

describe("the states", () => {
  it("says that no chat matches, with the words searched for, and tells the screen reader", async () => {
    answer = async () => ok([]);
    render(<Harness />);
    await type("zebra");
    expect(screen.getAllByText("No chats match “zebra”.")).toHaveLength(2); // drawn, and said by the live region
    expect(document.querySelector('[role="status"][aria-live="polite"]')?.textContent).toBe(
      "No chats match “zebra”.",
    );
  });

  it("says what a query with no usable word is", async () => {
    answer = async () => ok([], []);
    render(<Harness />);
    await type("a");
    expect(screen.getAllByText(/at least two letters or digits/).length).toBeGreaterThan(0);
  });

  it("tells a screen reader how many chats it found", async () => {
    render(<Harness />);
    await type("login");
    expect(screen.getAllByRole("status").some((s) => s.textContent === "2 chats found")).toBe(true);
  });

  it("dims the last results while the next are fetched", async () => {
    render(<Harness />);
    await type("login");
    answer = () => new Promise(() => {});
    fireEvent.change(box(), { target: { value: "login p" } });
    const results = document.querySelector('[data-slot="search-results"]');
    expect(results?.className).toContain("opacity-60");
    expect(results?.getAttribute("aria-busy")).toBe("true");
  });

  it("is an alert with Retry when the search fails, and Retry tries again", async () => {
    answer = async () => ({ response: { status: 500, headers: new Headers() } });
    render(<Harness />);
    await type("login");
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Could not search your chats.");
    answer = async () => ok(hits);
    await act(async () => fireEvent.click(within(alert).getByRole("button", { name: "Retry" })));
    expect(screen.getAllByRole("option")).toHaveLength(3);
  });

  it("says a 503 is busy, and a 429 is too fast and will try again", async () => {
    answer = async () => ({ response: { status: 503, headers: new Headers() } });
    render(<Harness />);
    await type("login");
    expect(screen.getByRole("alert").textContent).toContain("Search is busy right now");
    answer = async () => ({
      response: { status: 429, headers: new Headers({ "Retry-After": "3" }) },
    });
    await type("login p");
    expect(screen.getByText(/searching too fast\. Trying again shortly/)).toBeTruthy();
  });
});

describe("the box, as the review found it", () => {
  it("is inert when the answer is no match: aria-expanded is false and nothing controls a list", async () => {
    answer = async () => ok([]);
    render(<Harness />);
    await type("zebra");
    expect(box().getAttribute("aria-expanded")).toBe("false");
    expect(box().getAttribute("aria-controls")).toBeNull();
  });

  it("is inert after an error and within the debounce of a new text: Enter opens nothing", async () => {
    render(<Harness />);
    await type("login");
    fireEvent.keyDown(box(), { key: "ArrowDown" });
    answer = async () => ({ response: { status: 500, headers: new Headers() } });
    await type("login x");
    expect(box().getAttribute("aria-expanded")).toBe("false");
    expect(box().getAttribute("aria-activedescendant")).toBeNull();
    fireEvent.keyDown(box(), { key: "Enter" });
    expect(opened).toHaveLength(0);
  });

  it("keeps the hits and puts the alert under them when Show more fails", async () => {
    answer = async () => ({ ...ok(hits), data: { ...ok(hits).data, next: "CUR" } });
    render(<Harness />);
    await type("login");
    answer = async () => ({ response: { status: 500, headers: new Headers() } });
    await act(async () => fireEvent.click(screen.getByRole("button", { name: "Show more chats" })));
    expect(screen.getAllByRole("option")).toHaveLength(3);
    expect(screen.getByRole("alert").textContent).toContain("Could not search your chats.");
  });

  it("does nothing on Enter or the arrows while an IME composes, and prevents the default when it acts", async () => {
    render(<Harness />);
    await type("login");
    fireEvent.keyDown(box(), { key: "Enter", isComposing: true });
    fireEvent.keyDown(box(), { key: "ArrowDown", isComposing: true });
    expect(opened).toHaveLength(0);
    expect(box().getAttribute("aria-activedescendant")).toBeNull();
    expect(fireEvent.keyDown(box(), { key: "ArrowDown" })).toBe(false);
    expect(fireEvent.keyDown(box(), { key: "Enter" })).toBe(false);
    expect(opened).toHaveLength(1);
  });
});

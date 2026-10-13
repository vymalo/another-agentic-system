// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";
import type { ApiMe, ApiThread } from "@/lib/api/types";
import { PROFILES } from "../../../../mock/fixtures";
import { ThreadSidebar } from "./thread-sidebar";

let showDescriptions = true;
vi.mock("@/features/chat/hooks/use-ui-config", () => ({
  useShowDescriptions: () => showDescriptions,
}));
const router = { push: vi.fn(), replace: vi.fn() };
vi.mock("next/navigation", () => ({
  usePathname: () => "/threads/t-1",
  useRouter: () => router,
}));
let me: { status: "ready"; me: ApiMe } | { status: "unknown"; me: null } = {
  status: "ready",
  me: PROFILES.user,
};
vi.mock("@/features/me/hooks/use-me", () => ({ useMe: () => me }));

afterEach(() => {
  cleanup();
  showDescriptions = true;
  me = { status: "ready", me: PROFILES.user };
  router.push.mockReset();
  router.replace.mockReset();
});

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const thread = (id: string, title: string, description?: string): ApiThread => ({
  id,
  owner: "dev@example.com",
  title,
  ...(description ? { description } : {}),
  target: { agentId: "coder" },
  state: "done",
  createdAt: "2026-10-02T10:00:00Z",
  updatedAt: "2026-10-02T10:00:00Z",
  lastSeq: 7,
});

const view = (threads: ApiThread[], extra: Partial<ThreadsView> = {}): ThreadsView => ({
  threads,
  loading: false,
  error: null,
  hasMore: false,
  loadMore: vi.fn(),
  refresh: vi.fn(),
  archived: {
    open: false,
    toggle: vi.fn(),
    threads: [],
    loading: false,
    error: null,
    hasMore: false,
    loadMore: vi.fn(),
  },
  arrange: vi.fn(async () => {}),
  forget: vi.fn(),
  notice: null,
  dismissNotice: vi.fn(),
  ...extra,
});

const sidebar = (threads: ApiThread[], extra: Partial<ThreadsView> = {}) => {
  const v = view(threads, extra);
  render(
    <TooltipProvider>
      <ThreadSidebar threads={v} open onCollapse={vi.fn()} />
    </TooltipProvider>,
  );
  return v;
};

const DESCRIPTION = "The person wants a plan for a test, and **nothing else**.";

describe("a thread's description in the sidebar", () => {
  it("is the link's description for a screen reader, and the name stays the title", () => {
    sidebar([thread("t-1", "Fix the login", DESCRIPTION), thread("t-2", "No words")]);
    const link = screen.getByRole("link", { name: "Fix the login" });
    const id = link.getAttribute("aria-describedby") ?? "";
    expect(document.getElementById(id)?.textContent).toBe(DESCRIPTION);
    expect(link.textContent).toBe("Fix the login");
    expect(
      screen.getByRole("link", { name: "No words" }).getAttribute("aria-describedby"),
    ).toBeNull();
  });

  it("opens as a hover card when the keyboard focuses the row, in plain text, and closes with Escape", async () => {
    sidebar([thread("t-1", "Fix the login", DESCRIPTION)]);
    const link = screen.getByRole("link", { name: "Fix the login" });
    expect(document.querySelector("[data-slot='thread-description-card']")).toBeNull();
    fireEvent.focus(link);
    const card = await waitFor(() => {
      const found = document.querySelector("[data-slot='thread-description-card']");
      expect(found).not.toBeNull();
      return found as HTMLElement;
    });
    expect(card.textContent).toContain("Fix the login");
    expect(card.textContent).toContain(DESCRIPTION);
    // the model wrote it: Markdown is not rendered
    expect(card.querySelector("strong, em, a, code")).toBeNull();
    fireEvent.blur(link);
    await waitFor(() =>
      expect(document.querySelector("[data-slot='thread-description-card']")).toBeNull(),
    );
  });

  it("opens when the pointer rests on the row", async () => {
    sidebar([thread("t-1", "Fix the login", DESCRIPTION)]);
    fireEvent.pointerEnter(screen.getByRole("link", { name: "Fix the login" }), {
      pointerType: "mouse",
    });
    await waitFor(() =>
      expect(document.querySelector("[data-slot='thread-description-card']")).not.toBeNull(),
    );
  });

  it("is not drawn at all when the configuration hides descriptions", () => {
    showDescriptions = false;
    sidebar([thread("t-1", "Fix the login", DESCRIPTION)]);
    const link = screen.getByRole("link", { name: "Fix the login" });
    expect(link.getAttribute("aria-describedby")).toBeNull();
    fireEvent.focus(link);
    expect(screen.queryByText(DESCRIPTION)).toBeNull();
  });
});

describe("a shared thread in the sidebar (ADR 0040)", () => {
  it("has the share's mark after its title, with words for a screen reader, and a private one has none", () => {
    sidebar([
      { ...thread("t-1", "Fix the login"), share: { visibility: "public", effective: "internal" } },
      thread("t-2", "No words"),
    ]);
    const shared = screen.getByRole("link", { name: "Fix the login, shared · signed-in" });
    expect(shared.querySelector("[data-slot='share-mark']")).not.toBeNull();
    // the title is still the first thing the row says
    expect(shared.textContent?.startsWith("Fix the login")).toBe(true);
    expect(
      screen.getByRole("link", { name: "No words" }).querySelector("[data-slot='share-mark']"),
    ).toBeNull();
  });
});

const nav = () => screen.getByRole("navigation", { name: "Threads" });
const headings = () =>
  within(nav())
    .getAllByRole("heading", { level: 2 })
    .map((h) => h.textContent);
const linkTitles = (root: HTMLElement = nav()) =>
  within(root)
    .queryAllByRole("link")
    .map((a) => a.textContent ?? "")
    .filter((t) => t !== "New chat");

const pinned = (t: ApiThread): ApiThread => ({ ...t, pinned: true });
const child = (t: ApiThread, under: string): ApiThread => ({ ...t, nestedUnder: under });
const archived = (t: ApiThread): ApiThread => ({ ...t, archived: true });

describe("the sections of the list (ADR 0042): Pinned, Chats, Archived", () => {
  it("draws Pinned, then Chats, each in the order the server gave, and no recency group", () => {
    sidebar([
      pinned(thread("p-1", "Pinned one")),
      pinned(thread("p-2", "Pinned two")),
      thread("c-1", "Chat one"),
      thread("c-2", "Chat two"),
    ]);
    expect(headings()).toEqual(["Pinned", "Chats", "Archived"]);
    const lists = within(nav()).getAllByRole("list");
    expect(linkTitles(lists[0])).toEqual(["Pinned one", "Pinned two"]);
    expect(linkTitles(lists[1])).toEqual(["Chat one", "Chat two"]);
    for (const old of ["Today", "Yesterday", "Previous 7 days", "Previous 30 days", "Older"]) {
      expect(screen.queryByText(old)).toBeNull();
    }
  });

  it("leaves Pinned out when nothing is pinned, and Chats out when everything is", () => {
    sidebar([thread("c-1", "Chat one")]);
    expect(headings()).toEqual(["Chats", "Archived"]);
    cleanup();
    sidebar([pinned(thread("p-1", "Pinned one"))]);
    expect(headings()).toEqual(["Pinned", "Archived"]);
  });

  it("nests a fork under its parent, indented, in the parent's section", () => {
    sidebar([
      pinned(thread("p-1", "Parent")),
      child(thread("k-2", "Newer fork"), "p-1"),
      child(thread("k-1", "Older fork"), "p-1"),
      thread("c-1", "Chat one"),
    ]);
    const parent = screen.getByRole("link", { name: "Parent" }).closest("li") as HTMLElement;
    const nested = within(parent).getByRole("list");
    expect(nested.getAttribute("data-slot")).toBe("thread-children");
    expect(linkTitles(nested)).toEqual(["Newer fork", "Older fork"]);
    // the block is in Pinned: Chats holds only the other chat
    const sectionOf = (name: string) =>
      screen.getByRole("heading", { name }).parentElement as HTMLElement;
    expect(sectionOf("Pinned").contains(nested)).toBe(true);
    expect(linkTitles(sectionOf("Chats"))).toEqual(["Chat one"]);
  });

  it("keeps the highlight on the open thread, a child included", () => {
    sidebar([thread("t-1", "Open one"), child(thread("t-9", "A fork"), "t-1")]);
    expect(screen.getByRole("link", { name: "Open one" }).getAttribute("aria-current")).toBe(
      "page",
    );
  });

  it("says Your chats will show up here when there are none, and still offers Archived", () => {
    sidebar([]);
    expect(screen.getByText("Your chats will show up here.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Archived" })).toBeTruthy();
  });

  it("pages with Load older", () => {
    const v = sidebar([thread("c-1", "Chat one")], { hasMore: true });
    fireEvent.click(screen.getByRole("button", { name: "Load older" }));
    expect(v.loadMore).toHaveBeenCalled();
  });
});

describe("the Archived section: collapsed, loaded on demand", () => {
  const section = (extra: Partial<ThreadsView["archived"]> = {}, rows: ApiThread[] = []) => ({
    archived: { ...view([]).archived, ...extra, threads: rows },
  });

  it("is a button at the bottom that says it is collapsed, and asks the list to open it", () => {
    const v = sidebar([thread("c-1", "Chat one")]);
    const toggle = screen.getByRole("button", { name: "Archived" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    // collapsed: nothing of it is drawn, and it comes after the chats
    expect(screen.queryByText("Nothing archived.")).toBeNull();
    expect(
      toggle.compareDocumentPosition(screen.getByRole("link", { name: "Chat one" })) &
        Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy();
    fireEvent.click(toggle);
    expect(v.archived.toggle).toHaveBeenCalled();
  });

  it("lists the archived chats once it is open, as the server gave them, with their own menu", () => {
    sidebar(
      [thread("c-1", "Chat one")],
      section({ open: true }, [
        archived(thread("a-1", "Old plan")),
        archived(thread("a-2", "Old fix")),
      ]),
    );
    const toggle = screen.getByRole("button", { name: "Archived" });
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const panel = document.getElementById(
      toggle.getAttribute("aria-controls") ?? "",
    ) as HTMLElement;
    expect(linkTitles(panel)).toEqual(["Old plan", "Old fix"]);
    expect(within(panel).getByRole("button", { name: "More actions for Old plan" })).toBeTruthy();
  });

  it("says Loading while it reads, Nothing archived when there is none, and the error with Retry", () => {
    sidebar([], section({ open: true, loading: true }));
    expect(screen.getByRole("status").textContent).toMatch(/loading archived/i);
    cleanup();
    sidebar([], section({ open: true }));
    expect(screen.getByText("Nothing archived.")).toBeTruthy();
    cleanup();
    sidebar([], section({ open: true, error: "storage is unavailable" }));
    expect(screen.getByRole("alert").textContent).toMatch(/could not load archived/i);
  });

  it("nests a child that is archived with its parent under it, and stands a child archived alone by itself", () => {
    sidebar(
      [],
      section({ open: true }, [
        archived(thread("a-1", "Archived parent")),
        child(thread("a-1k", "Its fork"), "a-1"),
        archived(child(thread("a-2k", "Fork archived alone"), "elsewhere")),
      ]),
    );
    const parent = screen
      .getByRole("link", { name: "Archived parent" })
      .closest("li") as HTMLElement;
    expect(linkTitles(within(parent).getByRole("list"))).toEqual(["Its fork"]);
    const alone = screen.getByRole("link", { name: "Fork archived alone" }).closest("li");
    expect(alone?.parentElement?.getAttribute("data-slot")).not.toBe("thread-children");
  });

  it("pages the archived ones too", () => {
    const v = sidebar(
      [],
      section({ open: true, hasMore: true }, [archived(thread("a-1", "Old plan"))]),
    );
    const buttons = screen.getAllByRole("button", { name: "Load older" });
    fireEvent.click(buttons[buttons.length - 1] as HTMLElement);
    expect(v.archived.loadMore).toHaveBeenCalled();
  });
});

/** Opens the menu of a row the way a keyboard does. */
async function openMenu(title: string): Promise<HTMLElement> {
  const trigger = screen.getByRole("button", { name: `More actions for ${title}` });
  trigger.focus();
  fireEvent.keyDown(trigger, { key: "Enter" });
  return screen.findByRole("menu");
}
const items = (menu: HTMLElement) =>
  within(menu)
    .getAllByRole("menuitem")
    .map((i) => i.textContent);

describe("the row menu: a vertical three-dot button, shown on hover and on focus", () => {
  it("is a button named for its row, drawn but kept out of sight until the row is hovered or focused", () => {
    sidebar([thread("c-1", "Chat one")]);
    const button = screen.getByRole("button", { name: "More actions for Chat one" });
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
    expect(button.getAttribute("aria-expanded")).toBe("false");
    // revealed by the row's hover, the keyboard's focus in the row, an open menu, and a touch screen
    for (const reveal of [
      "group-hover/row:opacity-100",
      "group-has-[:focus-visible]/row:opacity-100",
      "data-[state=open]:opacity-100",
      "pointer-coarse:opacity-100",
    ]) {
      expect(button.className).toContain(reveal);
    }
    expect(button.className).toContain("opacity-0");
    // the icon is decoration: the name is the button's
    expect(button.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("is not a stop of the title's link: the link and the button are two controls", () => {
    sidebar([thread("c-1", "Chat one")]);
    const link = screen.getByRole("link", { name: "Chat one" });
    const button = screen.getByRole("button", { name: "More actions for Chat one" });
    expect(link.contains(button)).toBe(false);
  });

  it("offers Pin, Archive and Delete on a chat of the list's own level", async () => {
    sidebar([thread("c-1", "Chat one")]);
    expect(items(await openMenu("Chat one"))).toEqual(["Pin", "Archive", "Delete"]);
  });

  it("offers Unpin on a pinned chat", async () => {
    sidebar([pinned(thread("p-1", "Pinned one"))]);
    expect(items(await openMenu("Pinned one"))).toEqual(["Unpin", "Archive", "Delete"]);
  });

  it("offers Eject from parent and Archive on a child, and no Pin", async () => {
    sidebar([thread("c-1", "Parent"), child(thread("k-1", "A fork"), "c-1")]);
    const menu = await openMenu("A fork");
    expect(items(menu)).toEqual(["Eject from parent", "Archive", "Delete"]);
    // why there is no Pin is in the item that takes its place
    expect(
      within(menu).getByRole("menuitem", { name: "Eject from parent" }).getAttribute("title"),
    ).toMatch(/cannot be pinned/i);
  });

  it("offers Unarchive and Delete on an archived chat", async () => {
    sidebar([], {
      archived: {
        ...view([]).archived,
        open: true,
        threads: [archived(thread("a-1", "Old plan"))],
      },
    });
    expect(items(await openMenu("Old plan"))).toEqual(["Unarchive", "Delete"]);
  });

  it("offers Unarchive, Eject from parent and Delete on a child archived alone", async () => {
    sidebar([], {
      archived: {
        ...view([]).archived,
        open: true,
        threads: [archived(child(thread("a-2k", "Fork archived alone"), "elsewhere"))],
      },
    });
    expect(items(await openMenu("Fork archived alone"))).toEqual([
      "Unarchive",
      "Eject from parent",
      "Delete",
    ]);
  });

  it("has no Delete for a person whose roles do not hold thread.delete, on any kind of row", async () => {
    me = { status: "ready", me: PROFILES["read-only"] };
    sidebar([thread("c-1", "Chat one"), child(thread("k-1", "A fork"), "c-1")]);
    expect(items(await openMenu("Chat one"))).toEqual(["Pin", "Archive"]);
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    expect(items(await openMenu("A fork"))).toEqual(["Eject from parent", "Archive"]);
  });

  it("offers Delete while the roles cannot be read: the orchestrator decides, as everywhere else", async () => {
    me = { status: "unknown", me: null };
    sidebar([thread("c-1", "Chat one")]);
    expect(items(await openMenu("Chat one"))).toContain("Delete");
  });

  it("one choice is one arrangement: Pin, Archive, Unpin, Eject, Unarchive", async () => {
    const parent = thread("c-1", "Parent");
    const fork = child(thread("k-1", "A fork"), "c-1");
    const v = sidebar([parent, fork]);
    const choose = async (title: string, name: string) => {
      const menu = await openMenu(title);
      fireEvent.click(within(menu).getByRole("menuitem", { name }));
      await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    };
    await choose("Parent", "Pin");
    expect(v.arrange).toHaveBeenLastCalledWith(parent, "pin");
    await choose("Parent", "Archive");
    expect(v.arrange).toHaveBeenLastCalledWith(parent, "archive");
    await choose("A fork", "Eject from parent");
    expect(v.arrange).toHaveBeenLastCalledWith(fork, "eject");
    await choose("A fork", "Archive");
    expect(v.arrange).toHaveBeenLastCalledWith(fork, "archive");
    expect(v.arrange).toHaveBeenCalledTimes(4);
  });

  it("closes with Escape and gives the focus back to the button that opened it", async () => {
    sidebar([thread("c-1", "Chat one")]);
    const button = screen.getByRole("button", { name: "More actions for Chat one" });
    await openMenu("Chat one");
    expect(button.getAttribute("aria-expanded")).toBe("true");
    fireEvent.keyDown(screen.getByRole("menu"), { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(button));
  });

  it("shows why an arrangement was refused, in words, with a way to dismiss it", () => {
    const v = sidebar([thread("c-1", "Chat one")], {
      notice: "Could not pin “Chat one”: your roles do not let you do that",
    });
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Could not pin “Chat one”");
    fireEvent.click(within(alert).getByRole("button", { name: "Dismiss" }));
    expect(v.dismissNotice).toHaveBeenCalled();
  });
});

describe("Delete opens the dialog that says what is erased", () => {
  it("names the chat, and Cancel closes it with the focus back on the row's button", async () => {
    sidebar([thread("c-1", "Chat one")]);
    const button = screen.getByRole("button", { name: "More actions for Chat one" });
    const menu = await openMenu("Chat one");
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Delete" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Delete “Chat one”?" });
    expect(dialog.textContent).toMatch(/cannot be undone/i);
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(button));
  });
});

// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ThreadsView } from "@/features/threads/hooks/use-threads";
import type { ApiThread } from "@/lib/api/types";
import { ThreadSidebar } from "./thread-sidebar";

let showDescriptions = true;
vi.mock("@/features/chat/hooks/use-ui-config", () => ({
  useShowDescriptions: () => showDescriptions,
}));
vi.mock("next/navigation", () => ({ usePathname: () => "/threads/t-1" }));

afterEach(() => {
  cleanup();
  showDescriptions = true;
});

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const thread = (id: string, title: string, description?: string): ApiThread => ({
  id,
  title,
  ...(description ? { description } : {}),
  target: { agentId: "coder" },
  state: "done",
  createdAt: "2026-10-02T10:00:00Z",
  updatedAt: "2026-10-02T10:00:00Z",
  lastSeq: 7,
});

const view = (threads: ApiThread[]): ThreadsView => ({
  threads,
  loading: false,
  error: null,
  hasMore: false,
  loadMore: vi.fn(),
  refresh: vi.fn(),
});

const sidebar = (threads: ApiThread[]) =>
  render(<ThreadSidebar threads={view(threads)} open onCollapse={vi.fn()} />);

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

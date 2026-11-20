// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { ThreadViewProvider } from "@/features/chat/components/thread-view";
import type { SessionCard as Card } from "../lib/cards";
import { SessionCard } from "./session-card";

afterEach(cleanup);

const CHILD = "11111111-1111-4111-8111-111111111111";
const content = (over: Record<string, unknown> = {}) => ({
  session: "s1",
  threadId: CHILD,
  agent: "planner",
  description: "plan the release",
  keepOpen: false,
  state: "working",
  round: 1,
  card: "session-7",
  home: true,
  ...over,
});

describe("the card of a subagent", () => {
  it("names the helper, what it was started for, its state and its round, with a link to open it", () => {
    render(<SessionCard data={content()} />);
    const card = screen.getByRole("article", { name: "Subagent planner" });
    expect(card.textContent).toContain("planner");
    expect(card.textContent).toContain("plan the release");
    expect(card.textContent).toContain("Working");
    expect(card.textContent).toContain("Round 1");
    const open = screen.getByRole("link", { name: /Open/ });
    expect(open.getAttribute("href")).toBe(`/threads/${CHILD}`);
    expect(card.textContent).not.toContain("Started by you");
  });

  it("says when a person started it", () => {
    render(<SessionCard data={content({ by: "person" })} />);
    expect(screen.getByText("Started by you")).toBeTruthy();
  });

  it("says a blocked round waits for the person", () => {
    render(<SessionCard data={content({ report: "blocked" })} />);
    expect(screen.getByText("Waiting for you")).toBeTruthy();
  });

  it("draws nothing outside its home run", () => {
    const { container } = render(<SessionCard data={content({ home: false })} />);
    expect(container.innerHTML).toBe("");
    cleanup();
    const again = render(<SessionCard data={{ ...content(), home: undefined }} />);
    expect(again.container.innerHTML).toBe("");
  });

  it("draws nothing for a card of a bad shape", () => {
    const { container } = render(<SessionCard data={content({ state: "weird" })} />);
    expect(container.innerHTML).toBe("");
  });

  it("reads the live state, not the state of its own run", () => {
    const live: Card = {
      session: "s1",
      threadId: CHILD,
      agent: "planner",
      description: "plan the release",
      keepOpen: false,
      state: "closed",
      round: 2,
      report: "done",
      closing: false,
      closeAsked: false,
    };
    render(
      <ThreadViewProvider
        value={{
          state: undefined,
          waiting: false,
          agentId: null,
          sessions: new Map([["session-7", live]]),
        }}
      >
        <SessionCard data={content()} />
      </ThreadViewProvider>,
    );
    expect(screen.getByText("Closed")).toBeTruthy();
    expect(screen.getByText("Round 2")).toBeTruthy();
    expect(screen.queryByText("Working")).toBeNull();
  });

  it("draws the agent's words as text, never as markup", () => {
    const { container } = render(
      <SessionCard
        data={content({
          agent: "<img src=x onerror=alert(1)>",
          description: "**bold** <b>x</b> [a](javascript:alert(1))",
        })}
      />,
    );
    expect(container.querySelector("img, b, strong, a[href^='javascript']")).toBeNull();
    expect(container.textContent).toContain("<b>x</b>");
    expect(container.textContent).toContain("**bold**");
  });

  it("offers no link for a thread id that is not a UUID", () => {
    for (const threadId of ["../../etc", "javascript:alert(1)", `${CHILD}/x`]) {
      const { container, unmount } = render(<SessionCard data={content({ threadId })} />);
      expect(container.querySelector("a")).toBeNull();
      expect(container.textContent).toContain("planner");
      unmount();
    }
  });

  it("offers no link once the card has no thread", () => {
    const { container } = render(<SessionCard data={content({ threadId: undefined })} />);
    expect(container.querySelector("a")).toBeNull();
  });
});

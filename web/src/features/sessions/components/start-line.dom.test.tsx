// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ThreadViewProvider } from "@/features/chat/components/thread-view";
import type { SessionStartCard } from "../lib/cards";
import { StartLine } from "./start-line";

afterEach(cleanup);

const CHILD = "11111111-1111-4111-8111-111111111111";
const content = (over: Record<string, unknown> = {}) => ({
  request: "r1",
  agent: "planner",
  description: "plan the release",
  keepOpen: false,
  state: "starting",
  card: "session-start-r1",
  home: true,
  ...over,
});

describe("the line of a start a person asked for", () => {
  it("says what was started and that it is starting", () => {
    render(<StartLine data={content()} />);
    const line = screen.getByTestId("start-line");
    expect(line.textContent).toBe("You started planner: plan the release Starting…");
  });

  it("links to the child once it opened", () => {
    render(<StartLine data={content({ state: "opened", threadId: CHILD, session: "s1" })} />);
    expect(screen.getByRole("link", { name: /Open/ }).getAttribute("href")).toBe(
      `/threads/${CHILD}`,
    );
    expect(screen.queryByText("Starting…")).toBeNull();
  });

  it("gives no link for a thread id that is not a UUID", () => {
    const { container } = render(
      <StartLine data={content({ state: "opened", threadId: "../x", session: "s1" })} />,
    );
    expect(container.querySelector("a")).toBeNull();
  });

  it("says why it did not start, in the agent's words as text, and offers to try again", () => {
    const retry = vi.fn();
    const { container } = render(
      <StartLine
        data={content({ state: "failed", reason: "<b>no capacity</b> **now**" })}
        onTryAgain={retry}
      />,
    );
    expect(container.textContent).toContain("Did not start: <b>no capacity</b> **now**");
    expect(container.querySelector("b, strong")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(retry).toHaveBeenCalledTimes(1);
  });

  it("offers no Try again without a callback", () => {
    render(<StartLine data={content({ state: "failed", reason: "no" })} />);
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("is only drawn in its home run", () => {
    const { container } = render(<StartLine data={content({ home: false })} />);
    expect(container.innerHTML).toBe("");
  });

  it("reads the live state of the request", () => {
    const live: SessionStartCard = {
      request: "r1",
      agent: "planner",
      description: "plan the release",
      keepOpen: false,
      state: "opened",
      threadId: CHILD,
      session: "s1",
    };
    render(
      <ThreadViewProvider
        value={{
          state: undefined,
          waiting: false,
          agentId: null,
          sessionStarts: new Map([["r1", live]]),
        }}
      >
        <StartLine data={content()} />
      </ThreadViewProvider>,
    );
    expect(screen.getByRole("link", { name: /Open/ })).toBeTruthy();
    expect(screen.queryByText("Starting…")).toBeNull();
  });
});

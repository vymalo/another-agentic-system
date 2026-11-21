import { describe, expect, it } from "vitest";
import type { SessionCard } from "./cards";
import { isHomeCard, sessionStateWords, threadHref } from "./words";

const card = (over: Partial<SessionCard> = {}): SessionCard => ({
  session: "s1",
  agent: "planner",
  description: "plan it",
  keepOpen: false,
  state: "working",
  closing: false,
  closeAsked: false,
  ...over,
});

describe("sessionStateWords", () => {
  it("says working", () => {
    expect(sessionStateWords(card())).toBe("Working");
  });
  it("says that a blocked round waits for the person", () => {
    expect(sessionStateWords(card({ state: "working", round: 1, report: "blocked" }))).toBe(
      "Waiting for you",
    );
  });
  it("says idle after a round that is done or canceled", () => {
    expect(sessionStateWords(card({ state: "idle", round: 1, report: "done" }))).toBe("Idle");
    expect(sessionStateWords(card({ state: "idle", round: 2, report: "canceled" }))).toBe("Idle");
    expect(sessionStateWords(card({ state: "idle" }))).toBe("Idle");
  });
  it("names the round that failed", () => {
    expect(sessionStateWords(card({ state: "idle", round: 3, report: "failed" }))).toBe(
      "Round 3 failed",
    );
    expect(sessionStateWords(card({ state: "idle", report: "failed" }))).toBe("Round failed");
  });
  it("says closed whatever the last round was", () => {
    expect(sessionStateWords(card({ state: "closed", round: 1, report: "failed" }))).toBe("Closed");
  });
});

describe("threadHref", () => {
  it("is a route for a UUID only", () => {
    const id = "11111111-1111-4111-8111-111111111111";
    expect(threadHref(id)).toBe(`/threads/${id}`);
    for (const bad of [undefined, "", "../x", "abc", `${id}/../x`, "javascript:alert(1)"]) {
      expect(threadHref(bad)).toBeUndefined();
    }
  });
});

describe("isHomeCard", () => {
  it("is true only for a card that says it is home", () => {
    expect(isHomeCard({ home: true })).toBe(true);
    expect(isHomeCard({ home: false })).toBe(false);
    expect(isHomeCard({})).toBe(false);
    expect(isHomeCard("home")).toBe(false);
    expect(isHomeCard(null)).toBe(false);
  });
});

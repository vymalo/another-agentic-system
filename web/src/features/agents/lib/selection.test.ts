import { describe, expect, it } from "vitest";
import type { ApiAgent } from "@/lib/api/types";
import {
  agentNamed,
  effectiveSelection,
  newChatWith,
  requestedAgent,
  selectedAgent,
} from "./selection";

const coder: ApiAgent = {
  id: "coder",
  name: "Coder",
  releases: {
    defaultChannel: "production",
    channels: { production: "coder-r47", staging: "coder-r51" },
    revisions: ["coder-r53", "coder-r51", "coder-r47"],
  },
};
const reviewer: ApiAgent = { id: "reviewer", name: "Reviewer" };
const list = [coder, reviewer];

describe("selectedAgent", () => {
  it("is the agent the selection names, else the first", () => {
    expect(selectedAgent(list, { agentId: "reviewer", release: null })?.id).toBe("reviewer");
    expect(selectedAgent(list, { agentId: "gone", release: null })?.id).toBe("coder");
    expect(selectedAgent(list, { agentId: null, release: null })?.id).toBe("coder");
    expect(selectedAgent([], { agentId: "coder", release: null })).toBeUndefined();
  });
});

describe("effectiveSelection", () => {
  it("defaults an agent with releases to its default channel", () => {
    expect(effectiveSelection(list, { agentId: "coder", release: null })).toEqual({
      agentId: "coder",
      release: "production",
    });
  });

  it("keeps a channel or a revision the agent still offers", () => {
    expect(effectiveSelection(list, { agentId: "coder", release: "staging" }).release).toBe(
      "staging",
    );
    expect(effectiveSelection(list, { agentId: "coder", release: "coder-r53" }).release).toBe(
      "coder-r53",
    );
  });

  it("falls back to the default channel for a release the card no longer lists", () => {
    expect(effectiveSelection(list, { agentId: "coder", release: "canary" }).release).toBe(
      "production",
    );
  });

  it("sends no release to an agent without any", () => {
    expect(effectiveSelection(list, { agentId: "reviewer", release: "staging" })).toEqual({
      agentId: "reviewer",
      release: null,
    });
  });

  it("falls back to the first agent when the chosen one is not listed", () => {
    expect(effectiveSelection(list, { agentId: "gone", release: "x" })).toEqual({
      agentId: "coder",
      release: "production",
    });
  });

  it("keeps the choice while the list is empty (not loaded yet)", () => {
    expect(effectiveSelection([], { agentId: "coder", release: "staging" })).toEqual({
      agentId: "coder",
      release: null,
    });
    expect(effectiveSelection([], { agentId: null, release: null }).agentId).toBeNull();
  });
});

describe("requestedAgent", () => {
  it("reads ?agent= when the list has that agent", () => {
    expect(requestedAgent(list, "?agent=reviewer")).toBe("reviewer");
    expect(requestedAgent(list, "agent=coder")).toBe("coder");
  });

  it("ignores an unknown agent, an empty value and no query", () => {
    expect(requestedAgent(list, "?agent=nobody")).toBeNull();
    expect(requestedAgent(list, "?agent=")).toBeNull();
    expect(requestedAgent(list, "")).toBeNull();
    expect(requestedAgent(list, "?other=coder")).toBeNull();
  });

  it("is what newChatWith builds", () => {
    expect(requestedAgent(list, newChatWith("reviewer").slice(1))).toBe("reviewer");
    expect(newChatWith("a b")).toBe("/?agent=a%20b");
  });
});

// An agent that was renamed (ADR 0049): the list names it by its id and says what it was called.
describe("an agent that answers to another name", () => {
  const adam: ApiAgent = { id: "adam", name: "Adam", aliases: ["coder"] };
  const renamed = [adam, reviewer];

  it("is the agent the selection names by its old name (a choice made before the rename)", () => {
    expect(selectedAgent(renamed, { agentId: "coder", release: null })?.id).toBe("adam");
    expect(effectiveSelection(renamed, { agentId: "coder", release: null }).agentId).toBe("adam");
  });

  it("is the agent a link names by its old name, opened under its id", () => {
    expect(requestedAgent(renamed, "?agent=coder")).toBe("adam");
    expect(requestedAgent(renamed, "?agent=adam")).toBe("adam");
    expect(requestedAgent(renamed, "?agent=gone")).toBeNull();
  });

  it("is the agent a thread made under the old name (or a mention in its log) names", () => {
    expect(agentNamed(renamed, "coder")?.name).toBe("Adam");
    expect(agentNamed(renamed, "adam")?.name).toBe("Adam");
    expect(agentNamed(renamed, "gone")).toBeUndefined();
  });

  it("is found by its id before any alias: an alias never shadows an agent", () => {
    const odd: ApiAgent = { id: "other", name: "Other", aliases: ["reviewer"] };
    expect(selectedAgent([odd, reviewer], { agentId: "reviewer", release: null })?.id).toBe(
      "reviewer",
    );
  });
});

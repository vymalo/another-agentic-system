// @vitest-environment jsdom
import { cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { COMMANDS_URI } from "../lib/commands";
import { useCommands } from "./use-commands";

/*
 * The commands of the current agent (ADR 0079): read from the capabilities document's key for the
 * extension, and empty for a card that lists nothing, lists it badly, or could not be read.
 */

afterEach(cleanup);

const ALL = {
  commands: [
    { name: "review", description: "Reviews a change.", argumentHint: "<repo>" },
    { name: "explain", description: "Explains a file." },
  ],
};

/** The capabilities of an agent whose card lists `entry` under the extension (`undefined`: it lists nothing). */
const ready = (entry?: unknown): AgentCapabilities => ({
  status: "ready",
  supports: (uri) => uri === COMMANDS_URI && entry !== undefined,
  entry: (uri) => (uri === COMMANDS_URI ? entry : undefined),
  refresh: () => {},
});
const unreadable: AgentCapabilities = {
  status: "unreadable",
  supports: () => null,
  refresh: () => {},
};
const unknown: AgentCapabilities = { status: "unknown", supports: () => null, refresh: () => {} };

type Props = { capabilities: AgentCapabilities | undefined };
const mount = (initial: Props) =>
  renderHook((p: Props) => useCommands(p.capabilities), { initialProps: initial });

describe("useCommands", () => {
  it("offers the card's commands, sorted by name", () => {
    const { result } = mount({ capabilities: ready(ALL) });
    expect(result.current.map((c) => c.name)).toEqual(["explain", "review"]);
    expect(result.current.find((c) => c.name === "review")?.argumentHint).toBe("<repo>");
  });

  it("offers nothing for a card that lists no command, lists it badly, or cannot be read", () => {
    for (const capabilities of [
      ready(undefined),
      ready({ commands: [] }),
      ready({ commands: [{ name: "two words", description: "d" }] }),
      ready({ commands: [{ name: "review" }] }),
      unreadable,
      unknown,
      undefined,
    ]) {
      const { result } = mount({ capabilities });
      expect(result.current).toEqual([]);
    }
  });

  it("follows the agent: another card between two renders is another list", () => {
    const { result, rerender } = mount({ capabilities: ready(ALL) });
    expect(result.current.map((c) => c.name)).toEqual(["explain", "review"]);
    // the agent was redeployed with other commands
    rerender({
      capabilities: ready({ commands: [{ name: "test", description: "Runs the tests." }] }),
    });
    expect(result.current.map((c) => c.name)).toEqual(["test"]);
    // and the card stops listing the extension at all
    rerender({ capabilities: ready(undefined) });
    expect(result.current).toEqual([]);
  });
});

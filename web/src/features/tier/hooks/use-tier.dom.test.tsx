// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import { readPick, writePick } from "../lib/pick";
import { ACCESS_TIER_URI, type Tier } from "../lib/tier";
import { useTier } from "./use-tier";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

const ALL = { tiers: ["plan", "edits", "auto", "bypass"], default: "auto" };

/** The capabilities of an agent whose card lists `entry` under the extension (`undefined`: it lists nothing). */
const ready = (entry?: unknown): AgentCapabilities => ({
  status: "ready",
  supports: (uri) => uri === ACCESS_TIER_URI && entry !== undefined,
  entry: (uri) => (uri === ACCESS_TIER_URI ? entry : undefined),
  refresh: () => {},
});
const unreadable: AgentCapabilities = {
  status: "unreadable",
  supports: () => null,
  refresh: () => {},
};
const unknown: AgentCapabilities = { status: "unknown", supports: () => null, refresh: () => {} };

type Props = { threadId: string | null; capabilities: AgentCapabilities; max?: Tier };
const mount = (initial: Props) =>
  renderHook((p: Props) => useTier({ max: undefined, ...p }), { initialProps: initial });

describe("useTier: the chip exists only for an agent whose card lists the extension", () => {
  it("has no offer, no chip and nothing to send for a card that lists nothing, badly, or cannot be read", () => {
    for (const capabilities of [
      ready(undefined),
      ready({ tiers: [], default: "plan" }),
      ready({ tiers: ["plan"], default: "auto" }),
      unreadable,
      unknown,
    ]) {
      const { result } = mount({ threadId: "t-1", capabilities });
      expect(result.current.offer).toBeNull();
      expect(result.current.shown).toBeNull();
      expect(result.current.send).toBeUndefined();
      expect(result.current.offered).toEqual([]);
    }
  });

  it("shows the card's default and sends nothing until the person picks", () => {
    const { result } = mount({ threadId: "t-1", capabilities: ready(ALL) });
    expect(result.current.offered).toEqual(["plan", "edits", "auto", "bypass"]);
    expect(result.current.shown).toEqual({ tier: "auto", picked: false });
    expect(result.current.send).toBeUndefined();
  });

  it("is capped by the person's role: only the tiers up to the cap are offered", () => {
    const { result } = mount({ threadId: "t-1", capabilities: ready(ALL), max: "edits" });
    expect(result.current.offered).toEqual(["plan", "edits"]);
  });
});

describe("useTier: a pick", () => {
  it("is sent from then on, shown as picked, and remembered for the thread", () => {
    const { result } = mount({ threadId: "t-1", capabilities: ready(ALL) });
    act(() => result.current.pick("plan"));
    expect(result.current.send).toBe("plan");
    expect(result.current.shown).toEqual({ tier: "plan", picked: true });
    expect(readPick("t-1")).toBe("plan");
    expect(readPick("t-2")).toBeUndefined();
  });

  it("comes back with the thread: a page that opens it again has the pick", () => {
    writePick("t-1", "bypass");
    const { result } = mount({ threadId: "t-1", capabilities: ready(ALL) });
    expect(result.current.send).toBe("bypass");
    expect(result.current.shown).toEqual({ tier: "bypass", picked: true });
  });

  it("on a new chat is kept in memory and becomes the new thread's once it is created", () => {
    const { result } = mount({ threadId: null, capabilities: ready(ALL) });
    act(() => result.current.pick("edits"));
    expect(result.current.send).toBe("edits");
    expect(window.localStorage.length).toBe(0);
    act(() => result.current.created("t-new"));
    expect(readPick("t-new")).toBe("edits");
  });

  it("created with no pick writes nothing", () => {
    const { result } = mount({ threadId: null, capabilities: ready(ALL) });
    act(() => result.current.created("t-new"));
    expect(readPick("t-new")).toBeUndefined();
  });
});

describe("useTier: a pick that is not offered any more is forgotten, and said once", () => {
  it("when the agent stops listing the tier", () => {
    writePick("t-1", "bypass");
    const { result, rerender } = mount({ threadId: "t-1", capabilities: ready(ALL) });
    expect(result.current.send).toBe("bypass");
    rerender({
      threadId: "t-1",
      capabilities: ready({ tiers: ["plan", "edits"], default: "edits" }),
    });
    expect(result.current.send).toBeUndefined();
    expect(result.current.forgotten).toBe("bypass");
    expect(result.current.shown).toEqual({ tier: "edits", picked: false });
    expect(readPick("t-1")).toBeUndefined();
    act(() => result.current.dismiss());
    expect(result.current.forgotten).toBeNull();
  });

  it("when the person's cap goes below it", () => {
    writePick("t-1", "auto");
    const { result, rerender } = mount({ threadId: "t-1", capabilities: ready(ALL) });
    rerender({ threadId: "t-1", capabilities: ready(ALL), max: "edits" });
    expect(result.current.send).toBeUndefined();
    expect(result.current.forgotten).toBe("auto");
  });

  it("says nothing for an agent that lists no tier at all (there is no chip to say it in)", () => {
    writePick("t-1", "plan");
    const { result } = mount({ threadId: "t-1", capabilities: ready(undefined) });
    expect(result.current.send).toBeUndefined();
    expect(result.current.forgotten).toBeNull();
  });

  it("waits for a card that has not been read, and a card that cannot be read sends nothing", () => {
    writePick("t-1", "plan");
    const { result, rerender } = mount({ threadId: "t-1", capabilities: unknown });
    expect(result.current.send).toBeUndefined();
    // the pick is still remembered, and applies once the card says it is offered
    expect(readPick("t-1")).toBe("plan");
    rerender({ threadId: "t-1", capabilities: ready(ALL) });
    expect(result.current.send).toBe("plan");
    rerender({ threadId: "t-1", capabilities: unreadable });
    expect(result.current.send).toBeUndefined();
    expect(readPick("t-1")).toBe("plan");
  });
});

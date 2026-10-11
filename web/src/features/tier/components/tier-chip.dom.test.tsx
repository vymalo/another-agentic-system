// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AgentCapabilities } from "@/features/agents/hooks/use-agent-capabilities";
import type { TierView } from "../hooks/use-tier";
import { shownTier, TIERS, type Tier, type TierOffer } from "../lib/tier";
import { TierChip, TierNotice } from "./tier-chip";

afterEach(cleanup);

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const capabilities = (refresh = vi.fn()): AgentCapabilities => ({
  status: "ready",
  supports: () => true,
  refresh,
});

/** A view of an agent that honours `tiers` (default `auto`), the person's cap `max`, and their `pick`. */
function view(
  over: { tiers?: Tier[]; max?: Tier; pick?: Tier; forgotten?: Tier | null } = {},
): TierView & { picked: ReturnType<typeof vi.fn> } {
  const offer: TierOffer = { tiers: over.tiers ?? [...TIERS], default: "auto" };
  const limit = over.max === undefined ? TIERS.length : TIERS.indexOf(over.max) + 1;
  const offered = offer.tiers.filter((t) => TIERS.indexOf(t) < limit);
  const picked = vi.fn();
  return {
    offer,
    offered,
    shown: shownTier(offer, offered, over.pick),
    send: over.pick,
    pick: picked,
    forgotten: over.forgotten ?? null,
    dismiss: vi.fn(),
    created: vi.fn(),
    picked,
  };
}

const chip = () => screen.getByRole("button", { name: /^Access tier:/ });
function open() {
  chip().focus();
  fireEvent.keyDown(chip(), { key: "Enter" });
  return screen.findByRole("menu");
}

describe("TierChip", () => {
  it("is nothing without an offer: no control, no placeholder", () => {
    const none: TierView = { ...view(), offer: null, offered: [], shown: null, send: undefined };
    const { container } = render(
      <TierChip view={none} capabilities={capabilities()} agentName="Adam" />,
    );
    expect(container.textContent).toBe("");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("shows the agent's default and says so until the person picks", () => {
    render(<TierChip view={view()} capabilities={capabilities()} agentName="Adam" />);
    const button = chip();
    expect(button.getAttribute("aria-label")).toBe("Access tier: Auto (the agent's default)");
    expect(button.getAttribute("data-tier")).toBe("auto");
    expect(button.getAttribute("data-picked")).toBe("false");
    expect(button.textContent).toContain("Auto");
    expect(button.textContent).toContain("default");
    expect(button.getAttribute("aria-haspopup")).toBe("menu");
  });

  it("shows a capped person the tier their message runs under, and says why", async () => {
    // the cap is `edits` and the agent works under `auto`: the orchestrator runs the message at `edits`
    render(
      <TierChip view={view({ max: "edits" })} capabilities={capabilities()} agentName="Adam" />,
    );
    expect(chip().getAttribute("aria-label")).toBe(
      "Access tier: Edits (the most your role allows)",
    );
    expect(chip().getAttribute("data-tier")).toBe("edits");
    expect(chip().getAttribute("data-picked")).toBe("false");
    const menu = await open();
    const said = (tier: Tier) =>
      within(menu)
        .getAllByRole("menuitemradio")
        .find((i) => i.getAttribute("data-tier") === tier)?.textContent ?? "";
    // the default marked is the one that runs, not the agent's own, which this person may not use
    expect(said("edits")).toContain("default");
    expect(said("plan")).not.toContain("default");
    expect(screen.getByText(/the most your role allows\./).textContent).toBe(
      "Until you pick one, Adam works under Edits, the most your role allows.",
    );
  });

  it("is nothing for a person who may use no tier of the agent", () => {
    const none: TierView = { ...view({ tiers: ["auto", "bypass"], max: "edits" }) };
    const { container } = render(
      <TierChip view={none} capabilities={capabilities()} agentName="Adam" />,
    );
    expect(none.shown).toBeNull();
    expect(container.textContent).toBe("");
  });

  it("shows the pick, without the word default, once there is one", () => {
    render(
      <TierChip view={view({ pick: "plan" })} capabilities={capabilities()} agentName="Adam" />,
    );
    expect(chip().getAttribute("aria-label")).toBe("Access tier: Plan");
    expect(chip().getAttribute("data-picked")).toBe("true");
    expect(chip().textContent).not.toContain("default");
  });

  it("lists the tiers the agent honours, each with its icon and its line, the default marked", async () => {
    render(<TierChip view={view()} capabilities={capabilities()} agentName="Adam" />);
    const menu = await open();
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items.map((i) => i.getAttribute("data-tier"))).toEqual([...TIERS]);
    const said = (tier: Tier) =>
      within(menu)
        .getAllByRole("menuitemradio")
        .find((i) => i.getAttribute("data-tier") === tier)?.textContent ?? "";
    expect(said("plan")).toContain("Read and propose");
    expect(said("edits")).toContain("Change files, open PRs");
    expect(said("auto")).toContain("Merge and create too");
    expect(said("bypass")).toContain("Anything, never asks");
    // the default is marked in words, the others are not
    expect(said("auto")).toContain("default");
    expect(said("plan")).not.toContain("default");
    // icons are decoration: the name of each item is its words
    for (const item of items)
      expect(item.querySelector("svg")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("offers only what the person's role allows", async () => {
    render(
      <TierChip view={view({ max: "edits" })} capabilities={capabilities()} agentName="Adam" />,
    );
    const menu = await open();
    expect(
      within(menu)
        .getAllByRole("menuitemradio")
        .map((i) => i.getAttribute("data-tier")),
    ).toEqual(["plan", "edits"]);
  });

  it("picks a tier: choosing an item hands that tier to the view", async () => {
    const v = view();
    render(<TierChip view={v} capabilities={capabilities()} agentName="Adam" />);
    const menu = await open();
    const bypass = within(menu).getByRole("menuitemradio", { name: /Bypass/ });
    fireEvent.click(bypass);
    expect(v.picked).toHaveBeenCalledWith("bypass");
  });

  it("marks bypass by its icon and its words as well as its colour, in the menu and on the chip", async () => {
    const { rerender } = render(
      <TierChip view={view({ pick: "bypass" })} capabilities={capabilities()} agentName="Adam" />,
    );
    // the chip: the name says Bypass, the shape is the warning triangle, the colour is the warning one
    expect(chip().getAttribute("aria-label")).toBe("Access tier: Bypass");
    expect(chip().className).toContain("text-warning");
    expect(chip().querySelector("svg.lucide-triangle-alert")).not.toBeNull();
    rerender(<TierChip view={view()} capabilities={capabilities()} agentName="Adam" />);
    const menu = await open();
    const item = within(menu).getByRole("menuitemradio", { name: /Bypass/ });
    expect(item.querySelector("svg.lucide-triangle-alert")).not.toBeNull();
    expect(item.textContent).toContain("never asks");
    // and the other tiers are not warned about
    const plan = within(menu).getByRole("menuitemradio", { name: /Plan/ });
    expect(plan.querySelector("svg.lucide-triangle-alert")).toBeNull();
  });

  it("reads the card again each time the menu opens", async () => {
    const refresh = vi.fn();
    render(<TierChip view={view()} capabilities={capabilities(refresh)} agentName="Adam" />);
    expect(refresh).not.toHaveBeenCalled();
    await open();
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

describe("TierNotice", () => {
  it("says a pick that was forgotten, and the default that applies", () => {
    const v = view({ forgotten: "bypass" });
    render(<TierNotice view={v} agentName="Adam" />);
    expect(screen.getByRole("status").textContent).toContain(
      "Your earlier choice, Bypass, is not offered any more. Adam works under its default, Auto.",
    );
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }));
    expect(v.dismiss).toHaveBeenCalled();
  });

  it("says the tier a capped person's message runs under instead", () => {
    render(<TierNotice view={view({ forgotten: "bypass", max: "edits" })} agentName="Adam" />);
    expect(screen.getByRole("status").textContent).toContain(
      "Your earlier choice, Bypass, is not offered any more. Adam works under Edits, the most your role allows.",
    );
  });

  it("is nothing when nothing was forgotten", () => {
    const { container } = render(<TierNotice view={view()} agentName="Adam" />);
    expect(container.textContent).toBe("");
  });
});

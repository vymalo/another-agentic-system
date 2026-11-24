// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PickChoice, PickView } from "../hooks/use-pick";
import { type PickListing, parseListing, pickProps, type ShownPick, shownPick } from "../lib/pick";
import { PickChips } from "./pick-chip";

afterEach(cleanup);

// Radix measures its popper content; jsdom has no layout
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

const listing = {
  models: [
    { id: "alias-a", label: "Alias A" },
    { id: "alias-b", label: "Alias B" },
  ],
  modelDefault: "alias-a",
  efforts: ["low", "high"],
  effortDefault: "low",
};

/** The view of a card whose listing is `value`, for the person's `choice`. */
function view(
  over: { listing?: unknown; choice?: PickChoice } = {},
): PickView & { picked: ReturnType<typeof vi.fn> } {
  const raw = "listing" in over ? over.listing : listing;
  const parsed: PickListing | null = parseListing(raw);
  const choice = over.choice ?? {};
  const shown = parsed === null ? null : shownPick(parsed, choice);
  const gated: ShownPick = {};
  if (shown?.model !== undefined) gated.model = shown.model;
  if (shown?.effort !== undefined) gated.effort = shown.effort;
  const picked = vi.fn();
  return { listing: parsed, shown: gated, send: pickProps(choice), pick: picked, picked };
}

const modelChip = () => screen.queryByRole("button", { name: /^Model:/ });
const effortChip = () => screen.queryByRole("button", { name: /^Reasoning effort:/ });
function open(chip: HTMLElement) {
  chip.focus();
  fireEvent.keyDown(chip, { key: "Enter" });
  return screen.findByRole("menu");
}

describe("PickChips", () => {
  it("draws one chip per axis the card lists, at the agent's default, and says default", () => {
    render(<PickChips view={view()} agentName="Adam" />);
    const model = modelChip();
    const effort = effortChip();
    expect(model?.getAttribute("aria-label")).toBe("Model: Alias A (the agent's default)");
    expect(model?.getAttribute("data-axis")).toBe("model");
    expect(model?.getAttribute("data-picked")).toBe("false");
    expect(model?.textContent).toContain("default");
    expect(effort?.getAttribute("aria-label")).toBe("Reasoning effort: Low (the agent's default)");
    expect(effort?.getAttribute("data-axis")).toBe("effort");
    expect(effort?.getAttribute("data-picked")).toBe("false");
    expect(effort?.textContent).toContain("default");
  });

  it("draws no chip for an axis the live card does not list", () => {
    render(
      <PickChips
        view={view({ listing: { efforts: ["low", "high"], effortDefault: "low" } })}
        agentName="Adam"
      />,
    );
    expect(modelChip()).toBeNull();
    expect(effortChip()).not.toBeNull();
  });

  it("draws nothing when the card says nothing", () => {
    const { container } = render(<PickChips view={view({ listing: {} })} agentName="Adam" />);
    expect(container.textContent).toBe("");
    expect(screen.queryByRole("button")).toBeNull();
  });

  it("draws nothing when the agent list carries no listing at all", () => {
    const { container } = render(<PickChips view={view({ listing: null })} agentName="Adam" />);
    expect(container.textContent).toBe("");
  });

  it("lists the models the agent offers, the default marked, and hands a pick to the view", async () => {
    const v = view();
    render(<PickChips view={v} agentName="Adam" />);
    const menu = await open(modelChip() as HTMLElement);
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items.map((item) => item.getAttribute("data-model")).filter(Boolean)).toEqual([
      "alias-a",
      "alias-b",
    ]);
    expect(within(menu).getByRole("menuitemradio", { name: /^Alias A/ }).textContent).toContain(
      "default",
    );
    expect(within(menu).getByRole("menuitemradio", { name: /^Alias B/ }).textContent).not.toContain(
      "default",
    );
    fireEvent.click(within(menu).getByRole("menuitemradio", { name: /^Alias B/ }));
    expect(v.picked).toHaveBeenCalledWith({ model: "alias-b" });
  });

  it("lists the reasoning levels the agent offers and hands a pick to the view", async () => {
    const v = view();
    render(<PickChips view={v} agentName="Adam" />);
    const menu = await open(effortChip() as HTMLElement);
    const items = within(menu).getAllByRole("menuitemradio");
    expect(items.map((item) => item.getAttribute("data-effort")).filter(Boolean)).toEqual([
      "low",
      "high",
    ]);
    expect(within(menu).getByRole("menuitemradio", { name: /^Low/ }).textContent).toContain(
      "default",
    );
    fireEvent.click(within(menu).getByRole("menuitemradio", { name: /^High/ }));
    expect(v.picked).toHaveBeenCalledWith({ effort: "high" });
  });

  it("draws the chip for an axis with choices and no default, at Agent default", async () => {
    const v = view({ listing: { models: listing.models, efforts: listing.efforts } });
    render(<PickChips view={v} agentName="Adam" />);
    expect(modelChip()?.getAttribute("aria-label")).toBe("Model: Agent default");
    expect(modelChip()?.getAttribute("data-picked")).toBe("false");
    expect(modelChip()?.textContent).toContain("Agent default");
    expect(effortChip()?.getAttribute("aria-label")).toBe("Reasoning effort: Agent default");
    expect(effortChip()?.textContent).toContain("Agent default");
    const menu = await open(modelChip() as HTMLElement);
    const entry = within(menu).getByRole("menuitemradio", { name: /Agent default/ });
    // the first entry is selected while the person has picked nothing, and says who chooses
    expect(entry.getAttribute("data-agent-default")).toBe("true");
    expect(entry.getAttribute("aria-checked")).toBe("true");
    expect(entry.textContent).toContain("the agent chooses");
    fireEvent.click(within(menu).getByRole("menuitemradio", { name: /^Alias B/ }));
    expect(v.picked).toHaveBeenCalledWith({ model: "alias-b" });
  });

  it("choosing Agent default hands the view an empty pick for that axis", async () => {
    const v = view({ choice: { model: "alias-b", effort: "high" } });
    render(<PickChips view={v} agentName="Adam" />);
    const menu = await open(modelChip() as HTMLElement);
    fireEvent.click(within(menu).getByRole("menuitemradio", { name: /Agent default/ }));
    expect(v.picked).toHaveBeenLastCalledWith({ model: undefined });
    const efforts = await open(effortChip() as HTMLElement);
    fireEvent.click(within(efforts).getByRole("menuitemradio", { name: /Agent default/ }));
    expect(v.picked).toHaveBeenLastCalledWith({ effort: undefined });
  });

  it("shows the person's pick, without the word default, once there is one", () => {
    render(
      <PickChips view={view({ choice: { model: "alias-b", effort: "high" } })} agentName="Adam" />,
    );
    expect(modelChip()?.getAttribute("aria-label")).toBe("Model: Alias B");
    expect(modelChip()?.getAttribute("data-picked")).toBe("true");
    expect(modelChip()?.textContent).not.toContain("default");
    expect(effortChip()?.getAttribute("aria-label")).toBe("Reasoning effort: High");
    expect(effortChip()?.getAttribute("data-picked")).toBe("true");
    expect(effortChip()?.textContent).not.toContain("default");
  });

  it("reads the agent list again each time a menu opens", async () => {
    const refresh = vi.fn();
    render(<PickChips view={view()} refresh={refresh} agentName="Adam" />);
    expect(refresh).not.toHaveBeenCalled();
    await open(modelChip() as HTMLElement);
    expect(refresh).toHaveBeenCalledTimes(1);
  });
});

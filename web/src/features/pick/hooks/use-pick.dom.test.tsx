// @vitest-environment jsdom
import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { usePick } from "./use-pick";

afterEach(cleanup);

/** A listing the agent list carries: two neutral aliases and two levels, each with a default. */
const listing = {
  models: [
    { id: "alias-a", label: "Alias A" },
    { id: "alias-b", label: "Alias B" },
  ],
  modelDefault: "alias-a",
  efforts: ["low", "high"],
  effortDefault: "low",
};

type Props = { listing: unknown };
const mount = (initial: Props) => renderHook((p: Props) => usePick(p), { initialProps: initial });

describe("usePick: a chip exists per axis the live card lists", () => {
  it("draws both chips for a card that lists both axes, at the agent's defaults, sending nothing", () => {
    const { result } = mount({ listing });
    expect(result.current.shown).toEqual({
      model: { id: "alias-a", label: "Alias A", picked: false },
      effort: { level: "low", picked: false },
    });
    expect(result.current.send).toEqual({});
  });

  it("draws no model chip for a card that lists reasoning/v1 only", () => {
    const { result } = mount({
      listing: { efforts: ["low", "high"], effortDefault: "low" },
    });
    expect(result.current.shown).toEqual({ effort: { level: "low", picked: false } });
    expect(result.current.shown?.model).toBeUndefined();
  });

  it("draws no effort chip for a card that lists model-pick/v1 only", () => {
    const { result } = mount({
      listing: { models: [{ id: "alias-a", label: "Alias A" }], modelDefault: "alias-a" },
    });
    expect(result.current.shown).toEqual({
      model: { id: "alias-a", label: "Alias A", picked: false },
    });
    expect(result.current.shown?.effort).toBeUndefined();
  });

  it("draws nothing when the agent list carries no pick listing", () => {
    for (const value of [undefined, null, "alias-a", { models: "alias-a" }]) {
      const { result } = mount({ listing: value });
      expect(result.current.listing, JSON.stringify(value) ?? "undefined").toBeNull();
      expect(result.current.shown).toBeNull();
    }
  });

  it("draws nothing for a listing that names no axis", () => {
    const { result } = mount({ listing: {} });
    expect(result.current.listing).toEqual({ models: [], efforts: [] });
    expect(result.current.shown).toEqual({});
    expect(result.current.send).toEqual({});
  });

  it("draws the chip for a card that names choices but no default, at its agent-default entry", () => {
    const { result } = mount({
      listing: { models: listing.models },
    });
    expect(result.current.listing?.models).toHaveLength(2);
    expect(result.current.shown).toEqual({ model: { picked: false } });
    expect(result.current.send).toEqual({});
  });

  it("picking the agent default clears the choice: the next message sends nothing on that axis", () => {
    const { result } = mount({ listing: { models: listing.models } });
    act(() => result.current.pick({ model: "alias-b" }));
    expect(result.current.send).toEqual({ model: "alias-b" });
    expect(result.current.shown).toEqual({
      model: { id: "alias-b", label: "Alias B", picked: true },
    });
    act(() => result.current.pick({ model: undefined }));
    expect(result.current.send).toEqual({});
    expect(result.current.shown).toEqual({ model: { picked: false } });
  });
});

describe("usePick: a pick is shown, and sent with the messages from then on", () => {
  it("sends the model the person picked, and only that axis", () => {
    const { result } = mount({ listing });
    act(() => result.current.pick({ model: "alias-b" }));
    expect(result.current.shown).toEqual({
      model: { id: "alias-b", label: "Alias B", picked: true },
      effort: { level: "low", picked: false },
    });
    expect(result.current.send).toEqual({ model: "alias-b" });
  });

  it("sends the effort the person picked, and keeps the model they picked before", () => {
    const { result } = mount({ listing });
    act(() => result.current.pick({ model: "alias-b" }));
    act(() => result.current.pick({ effort: "high" }));
    expect(result.current.shown).toEqual({
      model: { id: "alias-b", label: "Alias B", picked: true },
      effort: { level: "high", picked: true },
    });
    expect(result.current.send).toEqual({ model: "alias-b", effort: "high" });
  });

  it("does not send a pick on an axis the card does not list", () => {
    const { result } = mount({ listing: { efforts: ["low", "high"], effortDefault: "low" } });
    act(() => result.current.pick({ model: "alias-b", effort: "high" }));
    expect(result.current.send).toEqual({ effort: "high" });
  });
});

describe("usePick: a choice the card no longer offers is forgotten", () => {
  it("drops a model the agent list no longer carries, and shows the default again", () => {
    const { result, rerender } = mount({ listing });
    act(() => result.current.pick({ model: "alias-b" }));
    expect(result.current.send).toEqual({ model: "alias-b" });
    rerender({ listing: { ...listing, models: [{ id: "alias-a", label: "Alias A" }] } });
    expect(result.current.send).toEqual({});
    expect(result.current.shown).toEqual({
      model: { id: "alias-a", label: "Alias A", picked: false },
      effort: { level: "low", picked: false },
    });
  });

  it("drops an effort when the card stops listing reasoning/v1", () => {
    const { result, rerender } = mount({ listing });
    act(() => result.current.pick({ effort: "high" }));
    rerender({ listing: { models: listing.models, modelDefault: "alias-a" } });
    expect(result.current.send).toEqual({});
    expect(result.current.shown?.effort).toBeUndefined();
  });

  it("sends nothing once the agent list carries no listing at all", () => {
    const { result, rerender } = mount({ listing });
    act(() => result.current.pick({ model: "alias-b", effort: "high" }));
    rerender({ listing: undefined });
    expect(result.current.shown).toBeNull();
    expect(result.current.send).toEqual({});
  });
});

import { describe, expect, it, vi } from "vitest";
import { SentTiers } from "./sent";

describe("the tiers of the messages this page sent", () => {
  it("keeps a tier by the message's id and lets it go when the send is refused", () => {
    const sent = new SentTiers();
    sent.remember("m-1", "plan");
    expect(sent.of("m-1")).toBe("plan");
    expect(sent.of("m-2")).toBeUndefined();
    sent.forget("m-1");
    expect(sent.of("m-1")).toBeUndefined();
  });

  it("tells subscribers when a tier changes, and not when forgetting what it never had", () => {
    const sent = new SentTiers();
    const listener = vi.fn();
    const unsubscribe = sent.subscribe(listener);
    const before = sent.getVersion();
    sent.forget("nothing");
    expect(listener).not.toHaveBeenCalled();
    expect(sent.getVersion()).toBe(before);
    sent.remember("m-1", "bypass");
    expect(listener).toHaveBeenCalledTimes(1);
    expect(sent.getVersion()).toBeGreaterThan(before);
    unsubscribe();
    sent.forget("m-1");
    expect(listener).toHaveBeenCalledTimes(1);
  });
});

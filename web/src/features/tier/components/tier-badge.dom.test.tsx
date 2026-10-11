// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { TierBadge } from "./tier-badge";

afterEach(cleanup);

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

describe("TierBadge", () => {
  it("says the tier a message went out under, in words a screen reader hears too", () => {
    const { container } = render(<TierBadge tier="edits" />);
    const badge = container.querySelector('[data-slot="tier-badge"]');
    expect(badge?.getAttribute("data-tier")).toBe("edits");
    expect(badge?.textContent).toBe("Sent under the access tier Edits");
    expect(badge?.querySelector(".sr-only")?.textContent).toBe("Sent under the access tier ");
  });

  it("marks bypass by its icon and its weight as well as its colour", () => {
    const { container } = render(<TierBadge tier="bypass" />);
    const badge = container.querySelector('[data-slot="tier-badge"]');
    expect(badge?.className).toContain("text-warning");
    expect(badge?.querySelector("svg.lucide-triangle-alert")).not.toBeNull();
    cleanup();
    const plain = render(<TierBadge tier="plan" />).container.querySelector(
      '[data-slot="tier-badge"]',
    );
    expect(plain?.className).not.toContain("text-warning");
    expect(screen.queryByText("Bypass")).toBeNull();
  });
});

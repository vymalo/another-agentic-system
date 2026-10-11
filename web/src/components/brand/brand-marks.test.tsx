// @vitest-environment jsdom
import { cleanup, render } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BRAND_MARKS, isBrandMark } from "./brand-marks";
import { GitHubMark } from "./github-mark";
import { OpenCodeMark } from "./opencode-mark";

/*
 * The third-party marks the page bundles (third-party-notices.md, ADR 0049's amendment of
 * 2026-10-11): each is one drawing of its own, in the colour of the text around it, and says what
 * it is only when a caller names it.
 */

afterEach(cleanup);

describe.each([
  ["GitHub", "github", GitHubMark],
  ["OpenCode", "opencode", OpenCodeMark],
] as const)("the %s mark", (name, brand, Mark) => {
  const svg = () => document.querySelector("svg") as SVGSVGElement;

  it("is one svg that draws only paths, in the colour of the text around it", () => {
    render(<Mark className="size-3" />);
    expect(document.querySelectorAll("svg")).toHaveLength(1);
    expect(svg().getAttribute("data-brand")).toBe(brand);
    expect(svg().getAttribute("class")).toBe("size-3");
    expect(svg().getAttribute("fill")).toBe("currentColor");
    const tags = new Set([...svg().querySelectorAll("*")].map((n) => n.tagName.toLowerCase()));
    expect([...tags].every((t) => t === "path")).toBe(true);
    for (const path of svg().querySelectorAll("path")) {
      // the drawing is the brand's, not a colour of ours: nothing but the inherited colour
      expect(path.getAttribute("fill") ?? "currentColor").toBe("currentColor");
      expect(path.getAttribute("d")).toMatch(/^[MmLlHhVvCcSsQqTtAaZz0-9\s.,-]+$/);
    }
    expect(svg().outerHTML).not.toMatch(
      /<script|<image|<use|<foreignObject|href=|\son\w+=|style=/i,
    );
  });

  it("is decorative until it is given a name", () => {
    render(<Mark />);
    expect(svg().getAttribute("aria-hidden")).toBe("true");
    expect(svg().getAttribute("role")).toBeNull();
    expect(svg().querySelector("title")).toBeNull();
    cleanup();
    render(<Mark title={name} />);
    expect(svg().getAttribute("role")).toBe("img");
    expect(svg().getAttribute("aria-label")).toBe(name);
    expect(svg().getAttribute("aria-hidden")).toBeNull();
    expect(svg().querySelector("title")?.textContent).toBe(name);
  });

  it("is known as a brand mark", () => {
    expect(isBrandMark(Mark)).toBe(true);
    expect(BRAND_MARKS).toContain(Mark);
  });
});

describe("a mark that is not a brand's", () => {
  it("is not one", () => {
    expect(isBrandMark(() => null)).toBe(false);
  });
});

import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AGENT_TINTS, AgentAvatar, initialOf, tintOf } from "./agent-avatar";

/** WCAG 2.x relative luminance and contrast ratio of two #rrggbb colours. */
function contrast(a: string, b: string): number {
  const lum = (hex: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const c = Number.parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * (r ?? 0) + 0.7152 * (g ?? 0) + 0.0722 * (bl ?? 0);
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return ((hi ?? 0) + 0.05) / ((lo ?? 0) + 0.05);
}

describe("AgentAvatar", () => {
  it("shows the first letter of the name, in capitals", () => {
    const html = renderToStaticMarkup(<AgentAvatar agentId="coder" name="coder" />);
    expect(html).toContain(">C<");
    expect(renderToStaticMarkup(<AgentAvatar agentId="r" name="  reviewer" />)).toContain(">R<");
  });

  it("skips leading punctuation, keeps a whole emoji, and never shows nothing", () => {
    expect(initialOf("(beta) coder")).toBe("B");
    expect(initialOf("42nd agent")).toBe("4");
    expect(initialOf("\u{1F43C} panda")).toBe("P");
    expect(initialOf("\u{1F43C}\u{1F43C}")).toBe("\u{1F43C}");
    expect(initialOf("   ")).toBe("?");
    expect(initialOf("")).toBe("?");
  });

  it("is decorative: hidden from assistive technology, the name is text beside it", () => {
    const html = renderToStaticMarkup(<AgentAvatar agentId="coder" name="coder" />);
    expect(html).toContain('aria-hidden="true"');
    expect(html).not.toContain("role=");
    expect(html).not.toContain("aria-label");
  });

  it("gives the same agent the same tint, whatever its name says", () => {
    expect(tintOf("coder")).toBe(tintOf("coder"));
    const a = renderToStaticMarkup(<AgentAvatar agentId="coder" name="coder" />);
    const b = renderToStaticMarkup(<AgentAvatar agentId="coder" name="Coder (renamed)" />);
    expect(a.match(/data-tint="(\w+)"/)?.[1]).toBe(b.match(/data-tint="(\w+)"/)?.[1]);
  });

  it("pins the tints of the agents of the dev stack, so a hash change is deliberate", () => {
    expect(["coder", "reviewer", "researcher", "verifier"].map((id) => tintOf(id).name)).toEqual([
      "sage",
      "plum",
      "teal",
      "sand",
    ]);
  });

  it("uses all six tints over a spread of ids", () => {
    const seen = new Set(Array.from({ length: 64 }, (_, i) => tintOf(`agent-${i}`).name));
    expect(seen.size).toBe(AGENT_TINTS.length);
  });

  it("keeps every letter at 4.5:1 or better in both schemes", () => {
    for (const tint of AGENT_TINTS) {
      expect(contrast(tint.light[0], tint.light[1]), `${tint.name} light`).toBeGreaterThanOrEqual(
        4.5,
      );
      expect(contrast(tint.dark[0], tint.dark[1]), `${tint.name} dark`).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("scales the circle and the letter with size", () => {
    const html = renderToStaticMarkup(<AgentAvatar agentId="coder" name="coder" size={20} />);
    expect(html).toContain("width:20px");
    expect(html).toContain("height:20px");
    expect(html).toContain("font-size:9px");
  });
});

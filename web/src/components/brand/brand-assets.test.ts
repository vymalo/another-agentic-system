import { readFileSync, statSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import manifest from "@/app/manifest";

/** The brand's files (web/DESIGN.md, "Brand"): what they may contain, what they must be. */
const root = path.resolve(".");
const read = (file: string) => readFileSync(path.join(root, file));

const BRAND_FILLS = ["#161d17", "#3f7341", "#faf7ef"];
const SVGS = ["public/brand/panda.svg", "src/app/icon.svg"];
const MAX_SVG_BYTES = 16 * 1024;

describe.each(SVGS)("%s", (file) => {
  const svg = read(file).toString("utf8");

  it("uses only the three brand fills", () => {
    const fills = [...svg.matchAll(/\bfill="([^"]*)"/g)].map((m) => (m[1] ?? "").toLowerCase());
    expect(fills.length).toBeGreaterThan(0);
    expect([...new Set(fills)].sort()).toEqual(BRAND_FILLS);
    // a colour that is not a fill attribute would be a fourth colour
    expect(svg).not.toMatch(/\b(stroke|stop-color|style)=/);
    expect(svg).not.toMatch(/gradient/i);
  });

  it("draws everything itself: no image, script, link, or handler", () => {
    expect(svg).not.toMatch(/<image\b|<script\b|<foreignObject\b|<use\b/i);
    expect(svg).not.toMatch(/\son\w+=|href=|xlink:/i);
  });

  it(`stays within ${MAX_SVG_BYTES / 1024} KB`, () => {
    expect(statSync(path.join(root, file)).size).toBeLessThanOrEqual(MAX_SVG_BYTES);
  });
});

/** Width and height from a PNG's IHDR. */
function pngSize(buf: Buffer): [number, number] {
  expect(buf.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
  return [buf.readUInt32BE(16), buf.readUInt32BE(20)];
}

describe("the raster icons", () => {
  it.each([
    ["public/brand/icon-192.png", 192],
    ["public/brand/icon-512.png", 512],
    ["public/brand/icon-maskable-512.png", 512],
    ["src/app/apple-icon.png", 180],
  ])("%s is a %i px square PNG", (file, size) => {
    expect(pngSize(read(file))).toEqual([size, size]);
  });

  it("favicon.ico holds 16, 32 and 48 px PNG images", () => {
    const ico = read("src/app/favicon.ico");
    expect(ico.readUInt16LE(0)).toBe(0);
    expect(ico.readUInt16LE(2)).toBe(1); // an icon
    const count = ico.readUInt16LE(4);
    const sizes = Array.from({ length: count }, (_, i) => {
      const at = 6 + 16 * i;
      const [length, offset] = [ico.readUInt32LE(at + 8), ico.readUInt32LE(at + 12)];
      const [w] = pngSize(ico.subarray(offset, offset + length));
      expect(ico.readUInt8(at)).toBe(w);
      return w;
    });
    expect(sizes).toEqual([16, 32, 48]);
  });
});

describe("the web app manifest", () => {
  const m = manifest();

  it("is named for the product, with the brand's colours", () => {
    expect(m.name).toBe("another·agentic");
    expect(m.short_name).toBe("agentic");
    expect(m.theme_color).toBe("#3F7341");
    expect(m.background_color).toBe("#FFFFFF");
  });

  it("points at icons that exist, one of them maskable", () => {
    const icons = m.icons ?? [];
    expect(icons.some((i) => i.purpose === "maskable")).toBe(true);
    for (const icon of icons) {
      const [w, h] = pngSize(read(`public${icon.src}`));
      expect(icon.sizes).toBe(`${w}x${h}`);
    }
  });
});

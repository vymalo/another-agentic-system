// Regenerates the icons from the one brand drawing, public/brand/panda.svg (web/DESIGN.md, "Brand"):
//
//   src/app/icon.svg                  the same drawing framed closer (below), the tab icon
//   src/app/favicon.ico               16, 32 and 48 px of that framing
//   src/app/apple-icon.png            180 px, the whole mark on a full square of the brand green
//   public/brand/icon-{192,512}.png   the manifest icons: the mark on a transparent square
//   public/brand/icon-maskable-512.png   the mark inside the maskable safe zone, on the green
//
//   pnpm brand:icons
//
// It renders with the Chromium that Playwright already pins for the tests, so it needs no other
// tool (no rsvg-convert, no ImageMagick). The ICO is written here: a header and PNG images.
//
// Why a framing: at 16 px the whole mark (the body, the bamboo) is a smudge. The tab icon keeps the
// drawing and crops it to a circle round the face, 1.25 times closer, which reads as a panda at 16 px.
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "@playwright/test";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const GREEN = "#3F7341";

const mark = readFileSync(path.join(root, "public/brand/panda.svg"), "utf8");

const n = (v) => Number(v.toFixed(1)); // 51.2, not 51.199999999999996

/** The mark cropped to a circle round the face: a smaller viewBox and a circular clip in it. */
function framed(svg) {
  const view = 512 / 1.25; // the square that shows, 1.25 times closer
  const [cx, cy] = [256, 0.4 * 512]; // the face; the crop circle stays inside the mark's own disc
  const opening = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512"';
  const defs = "</title><defs>";
  const disc = '</defs><g clip-path="url(#disc)">';
  const close = "</g></svg>";
  for (const part of [opening, defs, disc, close]) {
    if (!svg.includes(part)) throw new Error(`public/brand/panda.svg changed shape: no ${part}`);
  }
  return svg
    .replace(
      opening,
      `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${n(cx - view / 2)} 0 ${n(view)} ${n(view)}"`,
    )
    .replace(
      defs,
      `</title><defs><clipPath id="crop"><circle cx="${n(cx)}" cy="${n(cy)}" r="${n(view / 2)}"/></clipPath>`,
    )
    .replace(disc, `</defs><g clip-path="url(#crop)"><g clip-path="url(#disc)">`)
    .replace(close, "</g></g></svg>");
}

const icon = framed(mark);
const urlOf = (svg) => `data:image/svg+xml;base64,${Buffer.from(svg).toString("base64")}`;

const browser = await chromium.launch();
const page = await browser.newPage();

/** `svg` as a PNG: `scale` of the square, centred, over `background` (transparent when none). */
async function render(svg, size, { scale = 1, background = null } = {}) {
  await page.setViewportSize({ width: size, height: size });
  const inner = Math.round(size * scale);
  await page.setContent(
    `<!doctype html><style>html,body{margin:0;height:100%;background:${background ?? "transparent"}}
     body{display:grid;place-items:center}img{display:block;width:${inner}px;height:${inner}px}</style>
     <img alt="" src="${urlOf(svg)}">`,
  );
  await page.waitForFunction(() => document.images[0].complete);
  return page.screenshot({ omitBackground: background === null, type: "png" });
}

const out = (file, data) => writeFileSync(path.join(root, file), data);

out("src/app/icon.svg", icon);

out("public/brand/icon-192.png", await render(mark, 192));
out("public/brand/icon-512.png", await render(mark, 512));
// a launcher crops a maskable icon to a circle of at most 80% of its width: the disc sits inside it
out(
  "public/brand/icon-maskable-512.png",
  await render(mark, 512, { scale: 0.8, background: GREEN }),
);
// iOS fills transparency with black: the green fills the square, the disc as large as it goes
out("src/app/apple-icon.png", await render(mark, 180, { background: GREEN }));

// favicon.ico: three sizes, each a PNG image
const sizes = [16, 32, 48];
const images = [];
for (const size of sizes) images.push(await render(icon, size));
const head = Buffer.alloc(6 + 16 * sizes.length);
head.writeUInt16LE(1, 2); // type: icon
head.writeUInt16LE(sizes.length, 4);
let offset = head.length;
sizes.forEach((size, i) => {
  const at = 6 + 16 * i;
  head.writeUInt8(size, at); // width
  head.writeUInt8(size, at + 1); // height
  head.writeUInt16LE(1, at + 4); // colour planes
  head.writeUInt16LE(32, at + 6); // bits per pixel
  head.writeUInt32LE(images[i].length, at + 8);
  head.writeUInt32LE(offset, at + 12);
  offset += images[i].length;
});
out("src/app/favicon.ico", Buffer.concat([head, ...images]));

await browser.close();

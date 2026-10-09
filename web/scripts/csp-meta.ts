/*
 * After `next build`: writes the meta half of the content security policy into every page of the export (ADR 0047,
 * Amendment 2026-10-09; `src/lib/csp.ts` says why there are two halves): `script-src 'self'` and the SHA-256 of each inline
 * script of that page, as the first element of `<head>`. A page that has no `<head>`, or a policy already, fails the build:
 * a page without its hashes would run under the header's `'unsafe-inline'` alone.
 *
 *   tsx scripts/csp-meta.ts [dir]    # dir: the export, `NEXT_DIST_DIR` or `out`
 */
import { createHash } from "node:crypto";
import { readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { inlineScripts, metaPolicy, metaTag } from "../src/lib/csp";

const dir = path.resolve(process.argv[2] ?? process.env.NEXT_DIST_DIR ?? "out");

async function pages(root: string): Promise<string[]> {
  const entries = await readdir(root, { withFileTypes: true, recursive: true });
  return entries
    .filter((e) => e.isFile() && e.name.endsWith(".html"))
    .map((e) => path.join(e.parentPath, e.name));
}

const sha256 = (text: string) => createHash("sha256").update(text, "utf8").digest("base64");

let count = 0;
for (const file of await pages(dir)) {
  const html = await readFile(file, "utf8");
  if (/http-equiv=["']?Content-Security-Policy/i.test(html)) {
    throw new Error(`${path.relative(dir, file)} has a policy already`);
  }
  const head = /<head\b[^>]*>/i.exec(html);
  if (!head) throw new Error(`${path.relative(dir, file)} has no <head>`);
  const tag = metaTag(metaPolicy(inlineScripts(html).map(sha256)));
  const at = head.index + head[0].length;
  await writeFile(file, `${html.slice(0, at)}${tag}${html.slice(at)}`);
  count += 1;
}
if (count === 0) throw new Error(`no page in ${dir}`);
console.log(
  `csp-meta: ${count} pages of ${path.relative(process.cwd(), dir) || "."} carry their script hashes`,
);

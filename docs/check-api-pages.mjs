// After the build, for api/chat-api.yaml:
//  1. every operationId has a built page under api/operations/, and the page holds its operation's path;
//  2. every relative `](...)` target in the file resolves to a file under docs/.
// usage: node check-api-pages.mjs [spec] [built operations dir]
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const here = dirname(fileURLToPath(import.meta.url));
const spec = process.argv[2] ?? resolve(here, "api/chat-api.yaml");
const dir = process.argv[3] ?? resolve(here, ".vitepress/dist/api/operations");
const text = readFileSync(spec, "utf8");
const doc = parse(text);
const problems = [];

// 1. pages
const verbs = ["get", "put", "post", "delete", "patch", "head", "options", "trace"];
const ops = Object.entries(doc.paths).flatMap(([path, item]) => verbs.filter((v) => item[v]).map((v) => ({ id: item[v].operationId, path })));
const built = readdirSync(dir).filter((f) => f.endsWith(".html")).map((f) => f.replace(/\.html$/, ""));
console.log(`operations in the spec: ${ops.length}, operation pages built: ${built.length}`);
const missing = ops.filter((o) => !built.includes(o.id));
if (missing.length || ops.length !== built.length) problems.push(`no page for: ${missing.map((o) => o.id).join(", ") || "(count differs)"}`);
for (const o of ops) {
  if (!built.includes(o.id)) continue;
  if (!readFileSync(resolve(dir, `${o.id}.html`), "utf8").includes(o.path)) problems.push(`${o.id}.html does not contain its path ${o.path}`);
}

// 2. links
const docs = resolve(here);
const apiDir = resolve(here, "api");
let links = 0;
for (const m of text.matchAll(/\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g)) {
  const target = m[1];
  if (/^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) continue;
  links++;
  const file = resolve(apiDir, decodeURIComponent(target.split(/[?#]/)[0]));
  if (relative(docs, file).startsWith("..")) problems.push(`link ${target} climbs out of docs/`);
  else if (!existsSync(file)) problems.push(`link ${target} has no file ${relative(docs, file)}`);
}
console.log(`relative links checked: ${links}`);

if (problems.length) {
  for (const p of problems) console.error(p);
  process.exit(1);
}

// After the build: every operationId of api/chat-api.yaml has a built page under api/operations/.
// usage: node check-api-pages.mjs [spec] [built operations dir]
import { readFileSync, readdirSync } from "node:fs";
import { parse } from "yaml";

const spec = process.argv[2] ?? new URL("./api/chat-api.yaml", import.meta.url).pathname;
const dir = process.argv[3] ?? new URL("./.vitepress/dist/api/operations", import.meta.url).pathname;
const doc = parse(readFileSync(spec, "utf8"));
const verbs = ["get", "put", "post", "delete", "patch", "head", "options", "trace"];
const ids = Object.values(doc.paths).flatMap((item) => verbs.filter((v) => item[v]).map((v) => item[v].operationId));
const built = readdirSync(dir).filter((f) => f.endsWith(".html")).map((f) => f.replace(/\.html$/, ""));
const missing = ids.filter((id) => !built.includes(id));
console.log(`operations in the spec: ${ids.length}, operation pages built: ${built.length}`);
if (missing.length || ids.length !== built.length) {
  console.error(`no page for: ${missing.join(", ") || "(count differs)"}`);
  process.exit(1);
}

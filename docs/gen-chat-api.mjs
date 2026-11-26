// Writes api/chat-api.md (not committed) from api/chat-api.yaml: the operations of the chat API as a table.
import { readFileSync, writeFileSync } from "node:fs";
import { parse } from "yaml";

const doc = parse(readFileSync(new URL("./api/chat-api.yaml", import.meta.url), "utf8"));
const cell = (s) => String(s ?? "").replace(/\|/g, "\\|").replace(/\s+/g, " ").trim();
const rows = [];
for (const [path, item] of Object.entries(doc.paths ?? {}))
  for (const [method, op] of Object.entries(item))
    if (["get", "put", "post", "delete", "patch"].includes(method))
      rows.push(`| \`${method.toUpperCase()}\` | \`${path}\` | \`${cell(op.operationId)}\` | ${cell(op.summary)} |`);
writeFileSync(
  new URL("./api/chat-api.md", import.meta.url),
  `# Chat API (OpenAPI)\n\n${cell(doc.info?.title)}, version ${cell(doc.info?.version)}. The contract is [chat-api.yaml](chat-api.yaml) (OpenAPI 3.1). Its operations, read from it at build time:\n\n| Method | Path | Operation | Summary |\n|---|---|---|---|\n${rows.join("\n")}\n`,
);

// The chat API contract, read from the one file that is the contract (api/chat-api.yaml).
// Node side only (the sidebar and the dynamic routes); the pages read the same file in openapi/client.ts.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { DEFAULT_BASE, rewriteDocLinks } from "./shared";

const file = fileURLToPath(new URL("../../api/chat-api.yaml", import.meta.url));

export function loadSpec(): any {
  return parse(rewriteDocLinks(readFileSync(file, "utf8"), process.env.DOCS_BASE ?? DEFAULT_BASE));
}

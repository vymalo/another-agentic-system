// The chat API contract, read from the one file that is the contract (api/chat-api.yaml).
// Node side (the dynamic routes) and client side (the theme) both read it here.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { DEFAULT_TAG, rewriteDocLinks } from "./shared";

export { DEFAULT_TAG };

const file = join(dirname(fileURLToPath(import.meta.url)), "../../api/chat-api.yaml");

export function loadSpec(): any {
  return parse(rewriteDocLinks(readFileSync(file, "utf8"), process.env.DOCS_BASE ?? "/another-agentic-system/"));
}

// The chat API contract, read from the one file that is the contract (api/chat-api.yaml).
// Node side (the dynamic routes) and client side (the theme) both read it here.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

export const DEFAULT_TAG = "Chat";

const file = join(dirname(fileURLToPath(import.meta.url)), "../../api/chat-api.yaml");

export function loadSpec(): any {
  return parse(readFileSync(file, "utf8"));
}

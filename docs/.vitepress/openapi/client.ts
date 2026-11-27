// Client side of the API pages. Only api/reference.md, api/tags/ and api/operations/ import this, so the contract
// (a few hundred KB of YAML) is in their chunks and not in the theme that every page loads.
import { useOpenapi } from "vitepress-openapi/client";
import raw from "../../api/chat-api.yaml?raw";
import { rewriteDocLinks } from "./shared";

// Links in descriptions are rewritten to the site's routes under its base (Vite's BASE_URL is the config's `base`).
const spec = rewriteDocLinks(raw, import.meta.env.BASE_URL);

// Makes the contract the global spec of the library's components. The theme holds the rest of its configuration.
export function useChatApiSpec() {
  useOpenapi({ spec });
}

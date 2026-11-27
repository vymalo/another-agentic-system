import type { Theme } from "vitepress";
import Md3Theme from "@vaam-apps/vitepress-md3";
import { theme as openapiTheme, useOpenapi } from "vitepress-openapi/client";
import "vitepress-openapi/dist/style.css";
// The contract is the one file; `?raw` hands its text to the library, which parses it.
import raw from "../../api/chat-api.yaml?raw";
import { DEFAULT_TAG, rewriteDocLinks } from "../openapi/shared";

// Links in descriptions are rewritten to the site's routes under its base (Vite's BASE_URL is the config's `base`).
const spec = rewriteDocLinks(raw, import.meta.env.BASE_URL);

const theme: Theme = {
  ...Md3Theme,
  async enhanceApp(ctx) {
    await Md3Theme.enhanceApp?.(ctx);
    useOpenapi({
      spec,
      // No "Try it out": the docs site is not a client of any deployment, and the spec names no server.
      config: { spec: { defaultTag: DEFAULT_TAG }, operation: { hiddenSlots: ["playground"] } },
    });
    await openapiTheme.enhanceApp(ctx);
  },
};

export default theme;

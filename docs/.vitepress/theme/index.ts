import type { Theme } from "vitepress";
import Md3Theme from "@vaam-apps/vitepress-md3";
import { theme as openapiTheme, useOpenapi } from "vitepress-openapi/client";
import "vitepress-openapi/dist/style.css";
import { DEFAULT_TAG } from "../openapi/shared";
import "./extras.css";

// Only the configuration of the library is here; the contract itself is loaded by the API pages (openapi/client.ts).
const theme: Theme = {
  ...Md3Theme,
  async enhanceApp(ctx) {
    await Md3Theme.enhanceApp?.(ctx);
    useOpenapi({
      // No "Try it out": the docs site is not a client of any deployment, and the spec names no server.
      config: { spec: { defaultTag: DEFAULT_TAG }, operation: { hiddenSlots: ["playground"] } },
    });
    await openapiTheme.enhanceApp(ctx);
  },
};

export default theme;

import type { Theme } from "vitepress";
import Md3Theme from "@vaam-apps/vitepress-md3";
import { theme as openapiTheme, useOpenapi } from "vitepress-openapi/client";
import "vitepress-openapi/dist/style.css";
import { Layout } from "./mermaid";
import "virtual:group-icons.css";
import "./extras.css";

// Only the configuration of the library is here; the contract itself is loaded by the API pages (openapi/client.ts).
const theme: Theme = {
  ...Md3Theme,
  Layout,
  async enhanceApp(ctx) {
    await Md3Theme.enhanceApp?.(ctx);
    useOpenapi({
      // No "Try it out": the docs site is not a client of any deployment, and the spec names no server.
      config: { operation: { hiddenSlots: ["playground"] } },
    });
    await openapiTheme.enhanceApp(ctx);
  },
};

export default theme;

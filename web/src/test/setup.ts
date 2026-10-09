import { beforeEach } from "vitest";
import { setBrowserAuth } from "@/lib/auth/config";
import { setRuntimeConfig } from "@/lib/runtime-config";

// Every test starts as an edge deployment (the cookie of oauth2-proxy): the page asks
// `GET /api/public/auth` once per load, and a test that is about browser mode says so itself.
// The runtime configuration is the web's own origin (`/config.json` says nothing), unless a test says so.
beforeEach(() => {
  setRuntimeConfig({});
  setBrowserAuth(null);
});

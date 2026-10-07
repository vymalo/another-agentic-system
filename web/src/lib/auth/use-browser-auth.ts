"use client";

import { useEffect, useSyncExternalStore } from "react";
import { authReady, browserAuth, subscribeAuthConfig } from "./config";
import type { BrowserAuthConfig } from "./types";

/**
 * The deployment's kind, for a component: `undefined` while it is being asked, `null` for an edge
 * (the cookie, as before), the configuration for browser mode (ADR 0054).
 */
export function useBrowserAuth(): BrowserAuthConfig | null | undefined {
  const cfg = useSyncExternalStore(subscribeAuthConfig, browserAuth, () => undefined);
  useEffect(() => {
    void authReady();
  }, []);
  return cfg;
}

"use client";

import { useSyncExternalStore } from "react";
import {
  currentTheme,
  getChoice,
  setChoice,
  subscribeTheme,
  type Theme,
  type ThemeChoice,
} from "@/lib/theme";

/** The person's choice (System until they chose), and the way to change it. The server's render is System. */
export function useThemeChoice(): [ThemeChoice, (value: ThemeChoice) => void] {
  const value = useSyncExternalStore(subscribeTheme, getChoice, () => "system" as const);
  return [value, setChoice];
}

/** The theme the PAGE shows (`<html data-theme>`): what a drawing of ours must match, never the device's scheme. */
export function usePageTheme(): Theme {
  return useSyncExternalStore(subscribeTheme, currentTheme, () => "light" as const);
}

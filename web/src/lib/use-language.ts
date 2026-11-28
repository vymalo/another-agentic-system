"use client";

import { useSyncExternalStore } from "react";
import { getLanguage, setLanguage, subscribeLanguage } from "@/lib/language";

/**
 * The language the person chose on this device (nothing until they chose), and the way to change it. The
 * server's render has no choice: the head script has already put the stored one on `<html>`.
 */
export function useLanguage(): [string | null, (tag: string) => void] {
  const value = useSyncExternalStore(subscribeLanguage, getLanguage, () => null);
  return [value, setLanguage];
}

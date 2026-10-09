"use client";

import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";

/*
 * The static export has one page per kind of address (ADR 0047, decision 1): `/threads/_` and `/s/_`,
 * which the static server answers for any `/threads/<id>` and `/s/<token>` (`Caddyfile`), and the desktop
 * app maps the same way (`apps/tauri/src-tauri/src/assets.rs`). The id is in the address, read on the
 * client after the first render: the page was rendered at build time with `_`, and a first render that
 * differed from it would not hydrate.
 */

/** The segment after `prefix` in a path of that shape (`/threads/<id>`), else `null`. */
export function segmentAfter(prefix: string, pathname: string): string | null {
  if (!pathname.startsWith(prefix)) return null;
  const rest = pathname.slice(prefix.length);
  if (!rest || rest.includes("/")) return null;
  try {
    return decodeURIComponent(rest);
  } catch {
    return null;
  }
}

/**
 * The segment of the address after `prefix`: `undefined` until the page has read it (the first render,
 * which must equal the exported one), then the segment or `null`. A navigation to another id of the same
 * kind reads it again.
 */
export function useAddressSegment(prefix: string): string | null | undefined {
  const pathname = usePathname();
  const [segment, setSegment] = useState<string | null | undefined>(undefined);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the router's pathname is the trigger, the address the truth
  useEffect(() => {
    setSegment(segmentAfter(prefix, window.location.pathname));
  }, [prefix, pathname]);
  return segment;
}

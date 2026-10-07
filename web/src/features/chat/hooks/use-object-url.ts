"use client";

import { useEffect, useState } from "react";
import { fetchFileBlob } from "../lib/file-access";

export type ObjectUrl =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "error" }
  | { state: "ready"; url: string };

/**
 * A kept file as an object URL (ADR 0054, decision 9): fetched with DPoP when `enabled`, revoked
 * when the component goes or the file changes, and never made for a file that is only a link.
 */
export function useObjectUrl(href: string, enabled: boolean): ObjectUrl {
  const [value, setValue] = useState<ObjectUrl>({ state: enabled ? "loading" : "idle" });
  useEffect(() => {
    if (!enabled) {
      setValue({ state: "idle" });
      return;
    }
    const controller = new AbortController();
    let url: string | undefined;
    setValue({ state: "loading" });
    fetchFileBlob(href, controller.signal).then(
      (blob) => {
        if (controller.signal.aborted) return;
        url = URL.createObjectURL(blob);
        setValue({ state: "ready", url });
      },
      () => {
        if (!controller.signal.aborted) setValue({ state: "error" });
      },
    );
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [href, enabled]);
  return value;
}

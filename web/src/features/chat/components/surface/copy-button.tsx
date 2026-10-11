"use client";

import { CheckIcon, CopyIcon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";

/*
 * A button that copies its text to the clipboard (the catalog's Code and Terminal). It says what it did in
 * words, in a polite live region, and changes its icon too: "Copied", or "Copy failed" when the browser
 * refuses (no clipboard API, not a secure page). The words are the button's name, so it needs no aria-label.
 */

type State = "idle" | "copied" | "failed";

export function CopyButton({ text, label = "Copy" }: { text: string; label?: string }) {
  const [state, setState] = useState<State>("idle");

  useEffect(() => {
    if (state === "idle") return;
    const timer = window.setTimeout(() => setState("idle"), 2000);
    return () => window.clearTimeout(timer);
  }, [state]);

  const copy = () => {
    // no clipboard (an insecure page) and a refused write both say "Copy failed", never a crash
    let written: Promise<void>;
    try {
      written = navigator.clipboard?.writeText(text) ?? Promise.reject(new Error("no clipboard"));
    } catch (error) {
      written = Promise.reject(error);
    }
    written.then(
      () => setState("copied"),
      () => setState("failed"),
    );
  };

  const Icon = state === "copied" ? CheckIcon : state === "failed" ? TriangleAlertIcon : CopyIcon;
  return (
    <span className="inline-flex shrink-0 items-center gap-2">
      <Button
        type="button"
        variant="ghost"
        size="sm"
        className="h-7 px-2 text-xs"
        onClick={copy}
        data-slot="copy-button"
      >
        <Icon aria-hidden="true" className="size-3.5" />
        {state === "copied" ? "Copied" : state === "failed" ? "Copy failed" : label}
      </Button>
      <span role="status" className="sr-only">
        {state === "copied"
          ? "Copied to the clipboard"
          : state === "failed"
            ? "The text could not be copied to the clipboard"
            : ""}
      </span>
    </span>
  );
}

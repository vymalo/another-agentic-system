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
    navigator.clipboard.writeText(text).then(
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
        {state === "copied" ? "Copied to the clipboard" : state === "failed" ? "Copy failed" : ""}
      </span>
    </span>
  );
}

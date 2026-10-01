"use client";

import { PanelRightCloseIcon, PanelRightOpenIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { usePanel } from "../hooks/use-panel";

/**
 * The header's button for the right-hand panel. One button, one name ("Thread details"): the
 * state is `aria-expanded`, as for any disclosure. It exists only where a panel does (a thread).
 */
export function PanelToggle() {
  const panel = usePanel();
  if (!panel) return null;
  const Icon = panel.open ? PanelRightCloseIcon : PanelRightOpenIcon;
  return (
    <Button
      ref={panel.toggleRef}
      type="button"
      variant="ghost"
      size="icon"
      aria-label="Thread details"
      aria-expanded={panel.open}
      aria-controls={panel.panelId}
      aria-keyshortcuts="Control+Shift+Period Meta+Shift+Period"
      title="Thread details (Ctrl or ⌘ + Shift + .)"
      onClick={panel.toggle}
      className="size-9 rounded-full text-muted-foreground hover:text-foreground"
    >
      <Icon aria-hidden="true" className="size-4.5" />
    </Button>
  );
}

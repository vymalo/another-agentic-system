"use client";

import { type FocusEvent, type ReactElement, type ReactNode, useState } from "react";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";

/**
 * Focus that the page moved to a control after a click (a sidebar that closes hands the focus to the
 * button that opens it) is no reason to explain that control: only focus a keyboard would show a ring on
 * opens the hint. Where the browser cannot say, the focus opens it.
 */
function explainsOnlyKeyboardFocus(event: FocusEvent<HTMLElement>) {
  try {
    if (!event.currentTarget.matches(":focus-visible")) event.preventDefault();
  } catch {
    // no `:focus-visible` in this engine
  }
}

/**
 * The words of a control that is drawn as an icon, in the tooltip every icon of the chat uses (a pointer
 * rests on it, the keyboard focuses it). The tooltip is a hint for the eye: the control's name is its own
 * `aria-label`, and the hint says the same words (or more, such as a shortcut), so a screen reader loses
 * nothing when it does not open. It works on a `button` and on an `a` (`children` is the one element that
 * gets the hint) and, for a state that is not a control, on a `span`. `suppressed` keeps it shut while the
 * control's own menu or popover is open, which is where the person's eyes are.
 */
export function Hint({
  label,
  side = "bottom",
  suppressed = false,
  children,
}: {
  label: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
  suppressed?: boolean;
  children: ReactElement;
}) {
  const [open, setOpen] = useState(false);
  return (
    <TooltipProvider delayDuration={0}>
      <Tooltip open={open && !suppressed} onOpenChange={setOpen}>
        <TooltipTrigger asChild onFocus={explainsOnlyKeyboardFocus}>
          {children}
        </TooltipTrigger>
        <TooltipContent side={side}>{label}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}

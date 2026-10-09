"use client";

import { Slot } from "radix-ui";
import { type ComponentPropsWithRef, forwardRef } from "react";
import { Hint } from "@/components/hint";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export type TooltipIconButtonProps = ComponentPropsWithRef<typeof Button> & {
  tooltip: string;
  side?: "top" | "bottom" | "left" | "right";
};

/** An icon button whose name is its tooltip (`Hint`: the same hint every icon of the chat has). */
export const TooltipIconButton = forwardRef<HTMLButtonElement, TooltipIconButtonProps>(
  ({ children, tooltip, side = "bottom", className, ...rest }, ref) => {
    return (
      <Hint label={tooltip} side={side}>
        <Button
          variant="ghost"
          size="icon"
          {...rest}
          className={cn("aui-button-icon size-6 p-1 active:scale-90", className)}
          ref={ref}
        >
          <Slot.Slottable>{children}</Slot.Slottable>
          <span className="aui-sr-only sr-only">{tooltip}</span>
        </Button>
      </Hint>
    );
  },
);

TooltipIconButton.displayName = "TooltipIconButton";

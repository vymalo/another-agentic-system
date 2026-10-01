import { useId } from "react";
import { cn } from "@/lib/utils";

/**
 * The app's mark and an agent's avatar: a four-pointed sparkle on a soft round tile. The one
 * gradient of the design (web/DESIGN.md). Decorative: whoever uses it says the name in text.
 */
export function BrandMark({ className }: { className?: string }) {
  const fill = useId();
  return (
    <span
      aria-hidden="true"
      data-slot="brand-mark"
      className={cn(
        "inline-flex size-7 shrink-0 items-center justify-center rounded-full bg-[linear-gradient(135deg,#dbe7fd,#efe3fb)] dark:bg-[linear-gradient(135deg,#1e2a44,#2d2240)]",
        className,
      )}
    >
      <svg viewBox="0 0 24 24" className="size-[62%]" role="presentation">
        <defs>
          <linearGradient id={fill} x1="0" y1="0" x2="1" y2="1">
            <stop offset="0" stopColor="#4285f4" />
            <stop offset="0.55" stopColor="#9b72cb" />
            <stop offset="1" stopColor="#d96570" />
          </linearGradient>
        </defs>
        <path
          fill={`url(#${fill})`}
          d="M12 1.5c.5 4.9 2.3 8.1 5.2 9.4 1.1.5 2.5.9 4.3 1.1-4.9.5-8.1 2.3-9.4 5.2-.5 1.1-.9 2.5-1.1 4.3-.5-4.9-2.3-8.1-5.2-9.4-1.1-.5-2.5-.9-4.3-1.1 4.9-.5 8.1-2.3 9.4-5.2.5-1.1.9-2.5 1.1-4.3Z"
        />
      </svg>
    </span>
  );
}

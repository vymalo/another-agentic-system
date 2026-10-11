import type { ComponentType } from "react";

/** What the page draws in a small round slot: a lucide icon, or the mark of a brand. */
export type Glyph = ComponentType<{ className?: string; strokeWidth?: number }>;

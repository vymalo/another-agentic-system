import type { ReactNode } from "react";
import { PandaMark } from "@/components/brand/panda-mark";

/** A tab with nothing in it yet: the mark, what the tab is for, and why it is empty. */
export function EmptyPanel({ title, children }: { title: string; children: ReactNode }) {
  return (
    <div
      data-slot="panel-empty"
      className="flex h-full flex-col items-center justify-center gap-3 px-8 py-10 text-center"
    >
      <PandaMark size={96} />
      <p className="text-[0.9375rem] font-medium">{title}</p>
      <p className="max-w-64 text-sm text-balance text-muted-foreground">{children}</p>
    </div>
  );
}

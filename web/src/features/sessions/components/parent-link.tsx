"use client";

import { ArrowLeftIcon } from "lucide-react";
import Link from "next/link";
import type { ApiThread } from "@/lib/api/types";
import { useParentTitle } from "../hooks/use-parent-title";
import { threadHref } from "../lib/words";

/**
 * "← Back to *parent title*", a link to the parent thread. The route is made only of a UUID: a parent
 * that is not one is plain words with no link. The title is the parent's own text, drawn as text.
 */
export function ParentLink({
  parent,
  rows,
  className,
}: {
  parent: string | undefined;
  rows: readonly ApiThread[];
  className?: string;
}) {
  const title = useParentTitle(parent, rows);
  const href = threadHref(parent);
  if (!href) {
    return <span className={className}>Part of another chat</span>;
  }
  return (
    <Link
      href={href}
      data-slot="parent-link"
      className={`inline-flex min-w-0 max-w-full items-center gap-1 font-medium text-foreground underline-offset-4 hover:underline ${className ?? ""}`}
    >
      <ArrowLeftIcon aria-hidden="true" className="size-3.5 shrink-0" />
      <span className="truncate">
        Back to <span data-slot="parent-title">{title}</span>
      </span>
    </Link>
  );
}

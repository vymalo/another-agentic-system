"use client";

import type { ReactNode } from "react";

/*
 * What a question shows (catalog version 8, docs/api/ui-catalog-v1.md, "Version 8"): components of the
 * surface drawn with the question, between its words and its control, inside its group. They are read, not
 * answered: nothing in them takes part in the form. So that a form that has stopped taking input does not
 * switch off the Copy button of a shown code block, the group is not the disabled `fieldset` when it shows
 * something: the controls are in an inner `fieldset` (no role, no name) that is.
 */

/** The shown components of one question, in the order the question lists them. Nothing without any. */
export function Shown({ nodes }: { nodes: readonly ReactNode[] | undefined }): ReactNode {
  if (!nodes || nodes.length === 0) return null;
  return (
    <div data-slot="choices-shown" className="flex min-w-0 flex-col gap-2">
      {nodes.map((node, i) => (
        // the nodes are the surface's own components, written once: the place is the identity
        // biome-ignore lint/suspicious/noArrayIndexKey: see above
        <div key={i} className="min-w-0">
          {node}
        </div>
      ))}
    </div>
  );
}

/** Whether a question has anything to show. */
export const hasShown = (nodes: readonly ReactNode[] | undefined): boolean =>
  nodes !== undefined && nodes.length > 0;

/** The props that make the container of a typed question a group named by its words, when it shows something. */
export const groupOf = (
  nodes: readonly ReactNode[] | undefined,
  labelId: string,
): { role?: "group"; "aria-labelledby"?: string } =>
  hasShown(nodes) ? { role: "group", "aria-labelledby": labelId } : {};

/**
 * The `disabled` of the question's own group: it is the group's unless the question shows something, in
 * which case the controls carry it (see {@link Controls}).
 */
export const groupDisabled = (
  enabled: boolean,
  nodes: readonly ReactNode[] | undefined,
): boolean | undefined => (hasShown(nodes) ? undefined : !enabled);

/** The controls of a question that shows something, switched off together when the form is not taking input. */
export function Controls({
  enabled,
  nodes,
  children,
}: {
  enabled: boolean;
  nodes: readonly ReactNode[] | undefined;
  children: ReactNode;
}): ReactNode {
  if (!hasShown(nodes)) return children;
  return (
    <fieldset
      role="none"
      disabled={!enabled}
      className="m-0 flex min-w-0 flex-col gap-2 border-0 p-0"
    >
      {children}
    </fieldset>
  );
}

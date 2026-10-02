import { act, waitFor } from "@testing-library/react";
import { expect } from "vitest";
import { OWN_CATALOG } from "@/features/chat/lib/a2ui/catalog";
import { surface } from "@/features/chat/lib/a2ui/testing";
import { type GoldenFrame, loadGolden, THREAD_ID } from "@/features/chat/lib/agui/testing";
import { mountSurfaces } from "../surface/testing";

/** Test support for the file card and the catalog's Image: the `file` golden, played in the real app. */

/** The `file` golden (ADR 0032): a PNG, `chart.png`, kept by the artifact store. */
export const SHA = "4ff6ab670a58c14270e034e2090d9a432caa263a14e0a25785386b0c12f880b5";
export const HREF = `/api/threads/${THREAD_ID}/artifacts/${SHA}`;

export type Mounted = ReturnType<typeof mountSurfaces>;

export async function play(m: Mounted, frames: GoldenFrame[]) {
  await act(async () => {
    m.stream.frames(frames);
  });
  const last = frames.filter((f) => f.id !== undefined).at(-1)?.id;
  await waitFor(() => expect(m.agent.getSnapshot().lastSeq).toBe(last));
}

export const AGENT = { "vymalo.actor": { type: "agent", name: "plain" } };
export const artifactFrame = (content: Record<string, unknown>, id: number): GoldenFrame => ({
  id,
  event: {
    type: "ACTIVITY_SNAPSHOT",
    messageId: `evt-${id}`,
    activityType: "vymalo.artifact",
    content: { at: "2027-01-15T08:00:03Z", ...content },
    subagentRunId: "sub-2",
    metadata: AGENT,
  },
});

/** The golden's frames with `extra` after the file's artifact (id 3), numbered from 4. */
export function goldenWith(extra: GoldenFrame[]): GoldenFrame[] {
  const frames = loadGolden("file");
  const at = frames.findIndex((f) => f.id === 3);
  return [...frames.slice(0, at + 1), ...extra, ...frames.slice(at + 1)];
}

export const imageSurface = (components: Record<string, unknown>[]): GoldenFrame => ({
  event: {
    type: "ACTIVITY_SNAPSHOT",
    messageId: "a2ui-9",
    activityType: "a2ui-surface",
    replace: true,
    content: {
      a2ui_operations: surface(components, undefined, "v0.9.1", OWN_CATALOG.catalogId),
    },
    subagentRunId: "sub-2",
    metadata: AGENT,
  },
});

export const mount = () =>
  mountSurfaces({}, {}, undefined, { catalogVersion: OWN_CATALOG.version });

export const card = () => document.querySelector('[data-slot="file-card"]') as HTMLElement;

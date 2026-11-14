import path from "node:path";
import type { components } from "../src/lib/api/schema";

type ThreadExport = components["schemas"]["ThreadExport"];

/*
 * The conformance vectors of docs/api/thread-markdown-v1.md: each `<name>.export.json` (what `GET /api/threads/{id}/export` answers)
 * with the `<name>.md` a client must write from it, byte for byte. A run's own ids and times are fixed before the comparison.
 */

/** The thread's own id in every vector. A second id (a fork's parent) is `…0002`, and so on in the order they appear. */
export const GOLDEN_ID = "00000000-0000-4000-8000-000000000001";
export const GOLDEN_AT = "2026-11-14T10:00:00.000Z";
export const GOLDEN_DIR = path.resolve(
  import.meta.dirname,
  "../../docs/api/examples/thread-markdown",
);

/**
 * Whether the vectors are rewritten (`UPDATE_GOLDEN=1`). Never in CI: a run that writes the expected file and then compares the
 * code with it proves nothing, so a CI run that asks for it fails.
 */
export function updateGolden(env: Record<string, string | undefined>): boolean {
  if (env.UPDATE_GOLDEN !== "1") return false;
  if (env.CI)
    throw new Error(
      "UPDATE_GOLDEN is refused when CI is set: the golden files are written by a person and reviewed",
    );
  return true;
}

/** An export with what a run changes fixed: every UUID by the order it appears, every timestamp, the build versions. */
export function normalizeExport(doc: ThreadExport): ThreadExport {
  const ids = new Map<string, string>();
  const text = JSON.stringify(doc)
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g, (id) => {
      if (!ids.has(id))
        ids.set(id, `00000000-0000-4000-8000-${String(ids.size + 1).padStart(12, "0")}`);
      return ids.get(id) as string;
    })
    .replace(/\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z/g, GOLDEN_AT);
  return JSON.parse(text) as ThreadExport;
}

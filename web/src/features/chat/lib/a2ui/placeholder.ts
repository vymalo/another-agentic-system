/*
 * What a surface of the UI catalog is as one line of text: the placeholder Copy puts on the clipboard and Export Markdown
 * writes where a surface was (docs/api/thread-markdown-v1.md, "Surfaces"). The catalog defines no text fallback for a
 * component, so the line only names what was there, and a person who pastes it knows that something was drawn.
 *
 * Pure. It reads the raw operations of a surface (the agent's, untrusted) the way `choicesOf` does: a later definition of a
 * component replaces an earlier one, and a `createSurface` starts the surface over.
 */

/** The words every placeholder starts with: the page quotes them, a test holds the page to them. */
export const INTERFACE_WORDS = "[Interface";

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The components that only lay out others: they are not what the surface is about. */
const LAYOUT: ReadonlySet<string> = new Set(["Column", "Stack"]);

/**
 * The component types of each surface the operations are about, in the order each type was first defined, by surface id.
 * A surface the operations only delete has none.
 */
export function surfaceComponents(operations: unknown): Map<string, string[]> {
  const surfaces = new Map<string, Map<string, string>>();
  if (!Array.isArray(operations)) return new Map();
  for (const op of operations) {
    if (!isRecord(op)) continue;
    const create = op.createSurface;
    if (isRecord(create) && str(create.surfaceId) !== undefined) {
      surfaces.set(create.surfaceId as string, new Map());
    }
    const gone = op.deleteSurface;
    if (isRecord(gone) && str(gone.surfaceId) !== undefined)
      surfaces.delete(gone.surfaceId as string);
    const update = op.updateComponents;
    const id = isRecord(update) ? str(update.surfaceId) : undefined;
    if (!isRecord(update) || id === undefined || !Array.isArray(update.components)) continue;
    const components = surfaces.get(id) ?? new Map<string, string>();
    surfaces.set(id, components);
    for (const c of update.components) {
      const key = isRecord(c) ? str(c.id) : undefined;
      const type = isRecord(c) ? str(c.component) : undefined;
      if (key !== undefined && type !== undefined) components.set(key, type);
    }
  }
  const out = new Map<string, string[]>();
  for (const [id, components] of surfaces) out.set(id, [...new Set(components.values())]);
  return out;
}

/**
 * `[Interface: Choices]`, `[Interface: Cards, Mermaid]`: the component types of the surface except the ones that only lay
 * out (`Column`, `Stack`) and `Text`, in the order they were first defined; `[Interface: Text]` for a surface of layout and text
 * only, and `[Interface]` when the operations define no component. Each type is the catalog's own word, so it is plain text, but
 * only letters and digits are kept: the operations are the agent's.
 */
export function placeholderOf(types: readonly string[]): string {
  const clean = types.map((t) => t.replace(/[^A-Za-z0-9]/g, "")).filter((t) => t !== "");
  const about = clean.filter((t) => !LAYOUT.has(t) && t !== "Text");
  const rest = clean.filter((t) => !LAYOUT.has(t));
  const named = [...new Set(about.length > 0 ? about : rest.length > 0 ? rest : clean)];
  return named.length > 0 ? `${INTERFACE_WORDS}: ${named.join(", ")}]` : `${INTERFACE_WORDS}]`;
}

/** The id of the surface an operation is about, when it names one. */
function surfaceOf(op: unknown): string | undefined {
  if (!isRecord(op)) return undefined;
  for (const key of ["createSurface", "updateComponents", "updateDataModel", "deleteSurface"]) {
    const body = op[key];
    const id = isRecord(body) ? str(body.surfaceId) : undefined;
    if (id !== undefined) return id;
  }
  return undefined;
}

/**
 * The operations of a thread filed by the surface they are about, in one pass: reading a surface later costs its own
 * operations and not the whole log (a thread of thousands of turns reads each operation once).
 */
export class SurfaceLog {
  private readonly byId = new Map<string, unknown[]>();

  /** Files the operations; returns the ids of the surfaces they are about, each once, in the order first named. */
  feed(operations: unknown): string[] {
    const named: string[] = [];
    if (!Array.isArray(operations)) return named;
    for (const op of operations) {
      const id = surfaceOf(op);
      if (id === undefined) continue;
      const list = this.byId.get(id) ?? [];
      this.byId.set(id, list);
      list.push(op);
      if (!named.includes(id)) named.push(id);
    }
    return named;
  }

  /** The operations filed for a surface so far. */
  operations(id: string): unknown[] {
    return this.byId.get(id) ?? [];
  }

  /** The component types of the surface as the operations so far leave it; none for one that was deleted. */
  types(id: string): string[] {
    return surfaceComponents(this.operations(id)).get(id) ?? [];
  }
}

/** One placeholder for each surface the operations are about, in the order the surfaces were first seen. */
export function surfacePlaceholders(operations: unknown): string[] {
  const surfaces = surfaceComponents(operations);
  return [...surfaces.values()].map(placeholderOf);
}

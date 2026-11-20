/**
 * A2UI 1.0's four operations, applied to the surfaces of one activity (ADR 0075, docs/api/ui-catalog-v2.md
 * section 8). Our own reducer: the pinned `@assistant-ui/react-generative-ui` reads `"v0.9"`, `"v0.9.1"` and
 * the draft `"v1.0"`, and no release reads the released `"1.0"` (0.0.21 and 0.0.24: `dist/a2ui/reducer.js`,
 * 241 lines each; 0.1.11: a re-export of `@assistant-ui/generative-ui` 0.0.12, whose reducer is 236 lines and
 * also takes no `"1.0"`). So the web applies the operations itself and hands the surfaces it built to the
 * package's converter, which is untouched.
 *
 * One dialect, strict. Refused, never guessed at: an operation whose `version` is not `"1.0"`; a `createSurface`
 * that names no catalog, or names one already live; an `updateDataModel` with no `value` (`contents` and `data`
 * were the older spellings); a `path` that is not a JSON Pointer. A refused operation changes nothing and is
 * reported as one warning: the validator (`prepare.ts`) refuses the whole surface on the first one, so nothing
 * half applied is ever drawn.
 *
 * The split with the validator: a binding (`@path`), a call (`@call`) and an `@` key are the validator's, which
 * reads what the renderer can lower and refuses the rest. Nothing here looks at them; a component is kept as
 * the agent wrote it, so what the validator checks is exactly what was sent.
 *
 * Pure: no React, no I/O, no clock.
 */

import { A2UI_VERSION } from "./limits";

type Rec = Record<string, unknown>;

/** One surface, as the converter reads it (`convertSurfaceToUISpec` takes exactly this shape). */
export type SurfaceState = {
  /** The catalog the surface named in its `createSurface`: A2UI 1.0 makes it optional, this app does not. */
  readonly catalogId: string;
  readonly components: Map<string, Rec>;
  dataModel: unknown;
};

/** The surfaces of one activity, by their `surfaceId`. */
export type A2uiState = ReadonlyMap<string, SurfaceState>;

export type A2uiOperationResult = {
  readonly state: A2uiState;
  readonly warnings: string[];
};

const OPERATIONS = new Set([
  "createSurface",
  "updateComponents",
  "updateDataModel",
  "deleteSurface",
]);

/** A JSON Pointer's array index, canonical only: `0`, or no leading zero. */
const isArrayIndex = (segment: string): boolean => segment === "0" || /^[1-9]\d*$/.test(segment);

/**
 * The ceiling of an index that may be written into a list. Far above the renderer's displayed-item limit, so a
 * pointer cannot make the reducer allocate a huge array.
 */
const MAX_INDEX = 10_000;

const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);

/** The segments of a JSON Pointer (RFC 6901): `""` and `"/"` are the whole document, else it starts with `/`. */
function segmentsOf(path: string): string[] | undefined {
  if (path === "" || path === "/") return [];
  if (!path.startsWith("/")) return undefined;
  return path
    .slice(1)
    .split("/")
    .map((segment) => segment.replaceAll("~1", "/").replaceAll("~0", "~"));
}

const INVALID = Symbol("invalidPointer");

/**
 * `value` written at `segments`, from `index` down, into a copy of `current`: a missing object is made, a
 * missing list when the segment is an index or `-`, and `-` appends. `INVALID` when the path runs through a
 * value that is not a container, or names an index absurdly far out.
 */
function assign(
  current: unknown,
  segments: readonly string[],
  index: number,
  value: unknown,
): unknown {
  const segment = segments[index] as string;
  const last = index + 1 === segments.length;
  const container =
    current === undefined || current === null
      ? isArrayIndex(segment) || segment === "-"
        ? []
        : {}
      : current;

  if (Array.isArray(container)) {
    if (segment !== "-" && !isArrayIndex(segment)) return INVALID;
    const at = segment === "-" ? container.length : Number(segment);
    if (at > MAX_INDEX) return INVALID;
    const next = last ? value : assign(container[at], segments, index + 1, value);
    if (next === INVALID) return INVALID;
    const copy = container.slice();
    copy[at] = next;
    return copy;
  }

  if (!isRecord(container)) return INVALID;
  const next = last ? value : assign(container[segment], segments, index + 1, value);
  if (next === INVALID) return INVALID;
  return { ...container, [segment]: next };
}

/**
 * The member at `segments` removed from a copy of `current`: a key deleted, a list's slot deleted (its indices
 * are kept, since other pointers may name them). A path that is not there is left as it was, not an error.
 */
function remove(current: unknown, segments: readonly string[], index: number): unknown {
  const segment = segments[index] as string;
  const last = index + 1 === segments.length;

  if (Array.isArray(current)) {
    if (segment !== "-" && !isArrayIndex(segment)) return INVALID;
    const at = segment === "-" ? current.length : Number(segment);
    if (!Object.hasOwn(current, at)) return current;
    const next = last ? undefined : remove(current[at], segments, index + 1);
    if (next === INVALID) return INVALID;
    const copy = current.slice();
    if (last) delete copy[at];
    else copy[at] = next;
    return copy;
  }

  if (!isRecord(current)) return INVALID;
  if (!Object.hasOwn(current, segment)) return current;
  const next = last ? undefined : remove(current[segment], segments, index + 1);
  if (next === INVALID) return INVALID;
  const copy = { ...current };
  if (last) delete copy[segment];
  else copy[segment] = next;
  return copy;
}

/** The components of an `updateComponents` or an inline `createSurface`, by id; the malformed ones are reported. */
function upsert(surface: SurfaceState, components: unknown, warnings: string[], at: number): void {
  if (!Array.isArray(components)) {
    warnings.push(`Operation at index ${at} has malformed components.`);
    return;
  }
  for (const [i, component] of components.entries()) {
    if (
      !isRecord(component) ||
      typeof component.id !== "string" ||
      component.id === "" ||
      typeof component.component !== "string" ||
      component.component === ""
    ) {
      warnings.push(`Component at index ${i} in operation ${at} is malformed.`);
      continue;
    }
    surface.components.set(component.id, { ...component });
  }
}

/** A surface as a change leaves it: a copy of the components, never the map it was given. */
function copyOf(surface: SurfaceState): SurfaceState {
  return {
    catalogId: surface.catalogId,
    components: new Map(surface.components),
    dataModel: surface.dataModel,
  };
}

/**
 * The operations of A2UI 1.0 applied to `state`, in order: `createSurface` (inline `components` and `dataModel`
 * read), `updateComponents`, `updateDataModel` and `deleteSurface`. Returns the surfaces that stand and one
 * warning per operation that was refused; an operation of another version is refused, never read.
 */
export function applyA2uiOperations(state: A2uiState, operations: unknown): A2uiOperationResult {
  const next = new Map(state);
  const warnings: string[] = [];

  if (!Array.isArray(operations)) {
    return { state: next, warnings: ["A2UI operations must be an array."] };
  }

  for (const [at, entry] of operations.entries()) {
    try {
      if (!isRecord(entry)) {
        warnings.push(`Operation at index ${at} is malformed.`);
        continue;
      }

      const version = entry.version;
      if (version !== A2UI_VERSION) {
        warnings.push(
          typeof version === "string"
            ? `Operation at index ${at} is A2UI "${version}", which this app does not draw: it reads "${A2UI_VERSION}" only.`
            : `Operation at index ${at} has no A2UI version: this app reads "${A2UI_VERSION}" only.`,
        );
        continue;
      }

      const keys = Object.keys(entry).filter((key) => key !== "version");
      const operation = keys[0];
      if (keys.length !== 1 || operation === undefined) {
        warnings.push(`Operation at index ${at} must contain exactly one operation key.`);
        continue;
      }
      if (!OPERATIONS.has(operation)) {
        warnings.push(`Unknown A2UI operation "${operation}" at index ${at}.`);
        continue;
      }

      const payload = entry[operation];
      if (!isRecord(payload)) {
        warnings.push(`Operation at index ${at} has a malformed payload.`);
        continue;
      }
      const surfaceId = payload.surfaceId;
      if (typeof surfaceId !== "string" || surfaceId === "") {
        warnings.push(`Operation at index ${at} has no valid surfaceId.`);
        continue;
      }

      if (operation === "createSurface") {
        if (next.has(surfaceId)) {
          warnings.push(
            `Operation at index ${at} creates the surface "${surfaceId}", which is already live.`,
          );
          continue;
        }
        const catalogId = payload.catalogId;
        if (typeof catalogId !== "string" || catalogId === "") {
          warnings.push(`Operation at index ${at} creates a surface that names no catalogId.`);
          continue;
        }
        const surface: SurfaceState = {
          catalogId,
          components: new Map(),
          dataModel: payload.dataModel,
        };
        if (payload.components !== undefined) upsert(surface, payload.components, warnings, at);
        next.set(surfaceId, surface);
        continue;
      }

      if (operation === "deleteSurface") {
        next.delete(surfaceId);
        continue;
      }

      const current = next.get(surfaceId);
      if (current === undefined) {
        warnings.push(`Operation at index ${at} references missing surface "${surfaceId}".`);
        continue;
      }
      const surface = copyOf(current);

      if (operation === "updateComponents") {
        upsert(surface, payload.components, warnings, at);
        next.set(surfaceId, surface);
        continue;
      }

      if (!Object.hasOwn(payload, "value")) {
        warnings.push(`Operation at index ${at} has no data model value.`);
        continue;
      }
      const path = payload.path ?? "/";
      const segments = typeof path === "string" ? segmentsOf(path) : undefined;
      if (segments === undefined) {
        warnings.push(`Operation at index ${at} has a path that is not a JSON Pointer.`);
        continue;
      }
      if (segments.length === 0) {
        surface.dataModel = payload.value;
        next.set(surfaceId, surface);
        continue;
      }
      const written =
        payload.value === null
          ? remove(surface.dataModel, segments, 0)
          : assign(surface.dataModel, segments, 0, payload.value);
      if (written === INVALID) {
        warnings.push(`Operation at index ${at} has a path that is not a JSON Pointer.`);
        continue;
      }
      surface.dataModel = written;
      next.set(surfaceId, surface);
    } catch {
      warnings.push(`Operation at index ${at} is malformed.`);
    }
  }

  return { state: next, warnings };
}

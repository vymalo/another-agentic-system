/**
 * What a client of catalog version 8 does with a surface (docs/api/ui-catalog-v1.md, "Version 8"): a second
 * implementation, written from the contract page alone and not from the web's own (`src/features/chat/lib/
 * a2ui/prepare.ts`, `surfaces.ts`, `status.ts`), so that the two meeting on the page's conformance vectors
 * (`surfaces-v8.test.ts`) prove the page says enough for a client that is not the web. It judges the rules of
 * version 8 and leaves a component's own schema to the catalog (`check`, which the caller gives): what the
 * page says of a surface is built on it.
 */

type Rec = Record<string, unknown>;
const isRec = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);

/** The components a question may show: the page's closed list of seven. */
const SHOWABLE = new Set(["Text", "Image", "Mermaid", "Code", "Cards", "Callout", "Chart"]);

/** A plan is a surface whose id starts with `plan-` and has at least one more character. */
export const isPlan = (surfaceId: string): boolean =>
  surfaceId.length > 5 && surfaceId.startsWith("plan-");

/** The ids a Choices shows, with the question that shows each, in the order of the questions. */
function shownBy(choices: Rec): { id: string; question: string }[] {
  const out: { id: string; question: string }[] = [];
  const questions = Array.isArray(choices.questions) ? choices.questions : [];
  for (const q of questions) {
    if (!isRec(q) || !Array.isArray(q.show)) continue;
    for (const id of q.show) out.push({ id: String(id), question: String(q.id) });
  }
  return out;
}

export type Files = { sha256: string; kind: "image" | "other" }[];

/**
 * `drawn`, `pending` or `refused:<rule>`, in the order the page gives: each component against its schema and
 * its own rules (the `show` rules of one form for a Choices, the `artifact` of an Image), then the plan rule,
 * then what the forms of the surface show, then the walk from `root`, and `pending` last.
 */
export function surfaceResult(
  surfaceId: string,
  components: Rec[],
  files: Files,
  check: (component: Rec) => string | undefined,
): string {
  const byId = new Map(components.map((c) => [String(c.id), c]));
  for (const c of components) {
    if (check(c) !== undefined) return "refused:schema";
    if (c.component === "Choices") {
      const questions = Array.isArray(c.questions) ? c.questions : [];
      let total = 0;
      const seen = new Set<string>();
      for (const q of questions) {
        const ids = isRec(q) && Array.isArray(q.show) ? q.show.map(String) : [];
        for (const id of ids) {
          if (seen.has(id)) return "refused:schema"; // an id twice, in one question or in two
          seen.add(id);
        }
        total += ids.length;
      }
      if (total > 8) return "refused:schema";
    }
    if (c.component === "Image") {
      const file = files.find((f) => f.sha256 === c.artifact);
      if (file?.kind !== "image") return "refused:artifact";
    }
  }
  if (isPlan(surfaceId) && components.some((c) => c.component === "Choices")) {
    return "refused:action";
  }
  // what the forms of the whole surface show
  const shown = new Set<string>();
  const shownIn = new Map<string, string>();
  for (const c of components) {
    if (c.component !== "Choices") continue;
    for (const { id } of shownBy(c)) {
      const holder = shownIn.get(id);
      if (holder !== undefined && holder !== String(c.id)) return "refused:schema";
      shownIn.set(id, String(c.id));
      shown.add(id);
      const target = byId.get(id);
      if (target && !SHOWABLE.has(String(target.component))) return "refused:schema";
    }
  }
  // the walk from root: a shown component is reached by `show` only; one that is not there yet is pending
  let missing = false;
  const visit = (id: string, viaShow: boolean, path: Set<string>) => {
    const c = byId.get(id);
    if (!c) {
      missing = true;
      return;
    }
    if (!viaShow && shown.has(id)) throw new Expansion();
    if (path.has(id)) return;
    path.add(id);
    const kids: string[] = [];
    if (Array.isArray(c.children)) kids.push(...c.children.map(String));
    if (typeof c.child === "string") kids.push(c.child);
    for (const key of ["tabs", "items"]) {
      const list = c[key];
      if (Array.isArray(list)) {
        for (const e of list) if (isRec(e) && typeof e.child === "string") kids.push(e.child);
      }
    }
    for (const k of kids) visit(k, false, path);
    if (c.component === "Choices") for (const s of shownBy(c)) visit(s.id, true, path);
    path.delete(id);
  };
  class Expansion extends Error {}
  try {
    if (byId.has("root")) visit("root", false, new Set());
    else return "pending";
  } catch (e) {
    if (e instanceof Expansion) return "refused:expansion";
    throw e;
  }
  return missing ? "pending" : "drawn";
}

/** `[done, total]` of a checklist: a dropped item is in neither number. */
export function progress(items: { state: string }[]): [number, number] {
  const kept = items.filter((i) => i.state !== "dropped");
  return [kept.filter((i) => i.state === "done").length, kept.length];
}

/** The index of the first operation that is alive: the last `createSurface` of the surface, if everything before it is that surface's. */
export function aliveFrom(operations: Rec[]): number {
  const surfaceOf = (op: Rec): unknown => {
    for (const k of ["createSurface", "updateComponents", "updateDataModel", "deleteSurface"]) {
      const body = op[k];
      if (isRec(body)) return body.surfaceId;
    }
    return undefined;
  };
  let last = -1;
  operations.forEach((op, i) => {
    if (isRec(op.createSurface)) last = i;
  });
  if (last <= 0) return 0;
  const id = surfaceOf(operations[last] as Rec);
  return operations.slice(0, last).every((op) => surfaceOf(op) === id) ? last : 0;
}

export type Copy = { key: string; surfaceId: string; marker: string };

/** What each copy draws, and which plan is pinned (the marker of the plan sent last, `null` with none). */
export function placeCopies(copies: Copy[]): { result: string[]; pinned: string | null } {
  const lastOf = (key: string) =>
    copies
      .map((c, i) => (c.key === key ? i : -1))
      .filter((i) => i >= 0)
      .at(-1) ?? -1;
  const firstOf = (key: string) => copies.findIndex((c) => c.key === key);
  const result = copies.map((c, i) => {
    if (isPlan(c.surfaceId)) {
      return firstOf(c.key) === i ? `draws:${copies[lastOf(c.key)]?.marker}` : "nothing";
    }
    return lastOf(c.key) === i ? `draws:${c.marker}` : "older";
  });
  const plans = copies.filter((c) => isPlan(c.surfaceId));
  return { result, pinned: plans.at(-1)?.marker ?? null };
}

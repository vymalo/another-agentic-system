/**
 * What a client of catalog version 8 does with a surface (docs/api/ui-catalog-v1.md, "Version 8"): a second
 * implementation, written from the contract page and the catalog document alone (`catalog.json`, compiled here by
 * a JSON Schema engine of its own, Ajv) and not from the web's own (`src/features/chat/lib/a2ui/prepare.ts`,
 * `surfaces.ts`, `status.ts`, `catalog/validate.ts`), so that the two meeting on the page's conformance vectors
 * (`surfaces-v8.test.ts`) prove the page says enough for a client that is not the web. Where the page does not say
 * what the basic catalog's components are (a surface may name the A2UI basic catalog), it takes them as valid.
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

/** What a plan may not hold: a `Choices`, an input (with or without an action), a Button whose action is an event. */
function answerable(c: Rec): boolean {
  if (c.component === "Choices") return true;
  if (c.component === "TextField" || c.component === "CheckBox") return true;
  if (c.component === "Button") {
    const action = c.action;
    if (!isRec(action)) return false;
    if (isRec(action.functionCall)) return false; // a link (or a call nobody runs) sends nothing
    if (isRec(action.event)) return action.event.userMessage === undefined;
    return action.userMessage === undefined;
  }
  return false;
}

export type SurfaceCase = {
  surfaceId?: string;
  catalog?: "basic";
  components: Rec[];
  operationsAfter?: Rec[];
  /** The operations are `operationsAfter` alone: no createSurface, no updateComponents of the case. */
  noCreate?: boolean;
};

/**
 * `drawn`, `pending`, `deleted` or `refused:<rule>`, in the order the page gives: each component against its schema
 * and its own rules (the `show` rules of one form for a Choices, the `artifact` of an Image), then the plan rule,
 * then what the forms of the whole surface show, then the walk from `root`, and `pending` last. `check` judges a
 * component of this catalog against its schema (a message, or undefined).
 */
export function surfaceResult(
  c: SurfaceCase,
  files: Files,
  check: (component: Rec) => string | undefined,
): string {
  const surfaceId = c.surfaceId ?? "s1";
  const after = c.operationsAfter ?? [];
  // the operations that follow: a createSurface starts the surface over (nothing before it is alive, and the new
  // surface has no components yet), a deleteSurface removes it, and an updateComponents for a surface that is not
  // there (deleted, or never created) is refused (rule shape)
  let exists = c.noCreate !== true;
  let deleted = false;
  let restarted = false;
  for (const op of after) {
    if (isRec(op.createSurface)) {
      exists = true;
      restarted = true;
      deleted = false;
    } else if (isRec(op.deleteSurface)) {
      exists = false;
      deleted = true;
    } else if (isRec(op.updateComponents) && !exists) return "refused:shape";
  }
  if (c.noCreate === true) return after.length === 0 ? "pending" : "refused:shape";
  if (restarted) return "pending";
  if (deleted) return "deleted";
  const components = c.components;
  const own = c.catalog !== "basic";
  const byId = new Map(components.map((x) => [String(x.id), x]));
  for (const x of components) {
    if (own && check(x) !== undefined) return "refused:schema";
    if (x.component === "Choices") {
      const questions = Array.isArray(x.questions) ? x.questions : [];
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
    if (x.component === "Image") {
      const file = files.find((f) => f.sha256 === x.artifact);
      if (file?.kind !== "image") return "refused:artifact";
    }
  }
  if (isPlan(surfaceId) && components.some(answerable)) return "refused:action";
  // what the forms of the whole surface show
  const shown = new Set<string>();
  const shownIn = new Map<string, string>();
  for (const x of components) {
    if (x.component !== "Choices") continue;
    for (const { id } of shownBy(x)) {
      const holder = shownIn.get(id);
      if (holder !== undefined && holder !== String(x.id)) return "refused:schema";
      shownIn.set(id, String(x.id));
      shown.add(id);
      const target = byId.get(id);
      if (target && !SHOWABLE.has(String(target.component))) return "refused:schema";
    }
  }
  // the walk from root: a shown component is reached by `show` only; one that is not there yet is pending
  let missing = false;
  class Expansion extends Error {}
  const visit = (id: string, viaShow: boolean, path: Set<string>) => {
    const x = byId.get(id);
    if (!x) {
      missing = true;
      return;
    }
    if (!viaShow && shown.has(id)) throw new Expansion();
    if (path.has(id)) return;
    path.add(id);
    const kids: string[] = [];
    if (Array.isArray(x.children)) kids.push(...x.children.map(String));
    if (typeof x.child === "string") kids.push(x.child);
    for (const key of ["tabs", "items"]) {
      const list = x[key];
      if (Array.isArray(list)) {
        for (const e of list) if (isRec(e) && typeof e.child === "string") kids.push(e.child);
      }
    }
    for (const k of kids) visit(k, false, path);
    if (x.component === "Choices") for (const s of shownBy(x)) visit(s.id, true, path);
    path.delete(id);
  };
  try {
    if (!byId.has("root")) return "pending";
    visit("root", false, new Set());
  } catch (e) {
    if (e instanceof Expansion) return "refused:expansion";
    throw e;
  }
  return missing ? "pending" : "drawn";
}

/** `[done, total]` of one checklist: a dropped item is in neither number. */
export function progress(items: { state: string }[]): [number, number] {
  const kept = items.filter((i) => i.state !== "dropped");
  return [kept.filter((i) => i.state === "done").length, kept.length];
}

/** The progress of a plan: the sums over its checklists, `null` with none. */
export function planProgress(checklists: { state: string }[][]): [number, number] | null {
  if (checklists.length === 0) return null;
  return checklists
    .map(progress)
    .reduce<[number, number]>((a, p) => [a[0] + p[0], a[1] + p[1]], [0, 0]);
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

export type Copy = { key: string; surfaceId: string; marker: string; deleted?: boolean };
export type Item = Copy | { person: true };
const isCopy = (i: Item): i is Copy => !("person" in i);

/**
 * What each copy draws, and which plan is pinned (the marker of the newest plan that still exists, `null` with
 * none). A plan's copies are matched by `surfaceId` and cut into stretches by the messages of a person.
 */
export function placeCopies(items: Item[]): { result: string[]; pinned: string | null } {
  type Row = { c: Copy; stretch: number };
  const rows: Row[] = [];
  let stretch = 0;
  for (const it of items) {
    if (!isCopy(it)) stretch += 1;
    else rows.push({ c: it, stretch });
  }
  const say = (c: Copy, label: string) => (c.deleted ? "gone" : `${label}:${c.marker}`);
  const result = rows.map((r) => {
    if (isPlan(r.c.surfaceId)) {
      const mine = rows.filter((x) => x.c.surfaceId === r.c.surfaceId);
      const inStretch = mine.filter((x) => x.stretch === r.stretch);
      if (inStretch[0] !== r) return "nothing";
      const newest = (inStretch.at(-1) as Row).c;
      return say(newest, mine.at(-1)?.stretch === r.stretch ? "draws" : "older-plan");
    }
    const same = rows.filter((x) => x.c.key === r.c.key);
    const last = same.at(-1) as Row;
    if (last === r) return say(r.c, "draws");
    return last.c.deleted ? "gone" : "older";
  });
  // the pinned plan: the plan sent last that still exists, by its newest copy
  const seen = new Set<string>();
  let pinned: string | null = null;
  for (let i = rows.length - 1; i >= 0 && pinned === null; i--) {
    const c = (rows[i] as Row).c;
    if (!isPlan(c.surfaceId) || seen.has(c.surfaceId)) continue;
    seen.add(c.surfaceId);
    if (!c.deleted) pinned = c.marker;
  }
  return { result, pinned };
}

/** Whether each copy of a form is `answered` (a message of the person comes after it) or `open`. */
export function answeredCopies(items: string[]): string[] {
  return items.flatMap((it, i) =>
    it === "person" ? [] : [items.slice(i + 1).includes("person") ? "answered" : "open"],
  );
}

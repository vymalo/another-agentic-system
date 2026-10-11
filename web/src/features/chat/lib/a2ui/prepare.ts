import type { UIElement } from "@assistant-ui/react-generative-ui";
import {
  applyA2uiOperations,
  convertSurfaceToUISpec,
} from "@assistant-ui/react-generative-ui/a2ui";
import type { KeptFile } from "@/features/chat/lib/files";
import { firstBadUrl, readCards } from "./cards";
import { OWN_CATALOG, type OwnCatalog } from "./catalog";
import { type CompiledCatalog, compiledOf } from "./catalog/validate";
import { duplicateIn, limitsIn, readChoices } from "./choices";
import {
  ACCORDION,
  BASIC_CATALOG_IDS,
  CARDS,
  CHECK_BOX,
  CHOICES,
  FIELD,
  IMAGE,
  MAX_BYTES,
  MAX_COMPONENTS,
  MAX_DEPTH,
  MAX_ID_BYTES,
  MAX_NODES,
  MAX_TEMPLATE_ITEMS,
  MAX_USER_MESSAGE,
  MERMAID,
  OPEN_URL,
  OUTPUT_NAMES,
  RESERVED_PREFIX,
  TABS,
  TEXT_FIELD,
  UNSUPPORTED,
  USER_MESSAGE,
  VERSIONS,
  VOCABULARY,
} from "./limits";
import { isBinding, resolvePointer, resolveValue } from "./pointer";
import { safeHttpUrl } from "./url";

/**
 * The validator (ADR 0013, "The web renderer", rules 1 to 7): what stands between an agent's JSON
 * and the DOM. `prepareSurface` takes the operations of ONE surface exactly as the orchestrator
 * relayed them and either refuses the whole surface, with the rule and the reason, or hands back
 * what the renderer draws. It never returns part of a surface.
 *
 * Order, so that a cheap check comes before an expensive one: size, the shape of every operation,
 * the operations applied (the library's reducer, which only sees operations that already have the
 * right shape), the number of components, the vocabulary, function values, actions and inputs, and
 * last the walk from `root` that counts levels and nodes after expansion (early exit at the limit,
 * so an expansion bomb costs `MAX_NODES` steps, not its size). Only then is the library's
 * converter run, so nothing unchecked reaches it.
 *
 * Which components a surface may name depends on its catalog (ADR 0023): the basic catalog (or none
 * named) is the vocabulary of ten; this app's own catalog (`catalog/`) is the components it lists,
 * each checked against its JSON Schema; a surface that names any other catalog is refused. An
 * instance of our catalog that names a component this build does not have, in a thread whose
 * catalog is newer than this build's, is not an error of the agent's: it comes back as `newer`.
 *
 * Pure: no React, no I/O, no clock.
 */

export type Rule =
  | "shape"
  | "size"
  | "version"
  | "surfaces"
  | "catalog"
  | "schema"
  | "artifact"
  | "components"
  | "vocabulary"
  | "function"
  | "url"
  | "action"
  | "field"
  | "template"
  | "expansion"
  | "depth"
  | "cycle";

export type Prepared =
  /** Valid and complete: `spec` is what `renderGenerativeUI` draws. */
  | {
      kind: "surface";
      surfaceId: string;
      spec: UIElement;
      /** The initial value of every input, by field key (a JSON Pointer, or `#<component id>`). */
      fields: Record<string, unknown>;
      /** Buttons that send an event to the agent (they need a thread that waits for the owner). */
      eventActions: number;
    }
  /** Valid so far, nothing to draw yet: no `root`, or a reference to a component not sent yet. */
  | { kind: "pending" }
  /** The agent deleted the surface. */
  | { kind: "deleted" }
  /**
   * The surface names this app's catalog and a component of a newer version of it, which this
   * build cannot draw: shown as a placeholder asking for a newer app, never half drawn.
   */
  | { kind: "newer"; component: string }
  | { kind: "refused"; rule: Rule; reason: string };

class Refused extends Error {
  constructor(
    readonly rule: Rule,
    readonly reason: string,
  ) {
    super(reason);
  }
}
const refuse = (rule: Rule, reason: string): never => {
  throw new Refused(rule, reason);
};

type Rec = Record<string, unknown>;
const isRecord = (v: unknown): v is Rec => typeof v === "object" && v !== null && !Array.isArray(v);
const bytes = (s: string) => new TextEncoder().encode(s).length;
/** Agent text in a reason: bounded, and no control characters. */
// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them is the point
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/g;
const clip = (s: string, n = 40) =>
  `"${s.replace(CONTROL, "?").slice(0, n)}${s.length > n ? "…" : ""}"`;

const OPERATIONS = ["createSurface", "updateComponents", "updateDataModel", "deleteSurface"];
/** Props that name a URL, on any component. */
const URL_KEYS = ["url", "href", "src", "uri", "link", "iconUrl", "imageUrl"];
/** Nesting of the JSON inside one component: far beyond any real interface. */
const MAX_PROP_DEPTH = 32;

const isTemplate = (v: unknown): { componentId: string; path: string } | undefined => {
  if (!isRecord(v)) return undefined;
  const keys = Object.keys(v);
  // the pinned converter reads {template:{componentId,path}}; A2UI v0.9.1 writes {componentId,path}
  if (keys.length === 1 && keys[0] === "template" && isRecord(v.template))
    return isTemplate(v.template);
  if (keys.length === 2 && typeof v.componentId === "string" && typeof v.path === "string") {
    return { componentId: v.componentId, path: v.path };
  }
  return undefined;
};

/** Does a function-call value (`{call, args}`) hide in `v`? `skip` is a key that is not scanned. */
function hasFunctionValue(v: unknown, depth = 0, skip?: string): boolean {
  if (depth > MAX_PROP_DEPTH) refuse("shape", "a component nests its properties too deeply");
  if (Array.isArray(v)) return v.some((x) => hasFunctionValue(x, depth + 1));
  if (!isRecord(v)) return false;
  if (typeof v.call === "string") return true;
  return Object.entries(v).some(([k, x]) => k !== skip && hasFunctionValue(x, depth + 1));
}

/** The context of an action: a binding to an input becomes "the input's value at the click". */
function rewriteContext(v: unknown, inputs: ReadonlySet<string>, depth = 0): unknown {
  if (depth > MAX_PROP_DEPTH) refuse("shape", "an action context is nested too deeply");
  if (isBinding(v)) return inputs.has(v.path) ? { [FIELD]: v.path } : v;
  if (Array.isArray(v)) return v.map((x) => rewriteContext(x, inputs, depth + 1));
  if (!isRecord(v)) return v;
  const out: Rec = {};
  for (const [k, x] of Object.entries(v)) {
    Object.defineProperty(out, k, {
      value: rewriteContext(x, inputs, depth + 1),
      enumerable: true,
      writable: true,
      configurable: true,
    });
  }
  return out;
}

/** What a Button does, as the renderer knows it: the action becomes an `event` the converter maps. */
function lowerAction(id: string, action: unknown, inputs: ReadonlySet<string>): unknown {
  const at = `Button ${clip(id)}`;
  if (!isRecord(action)) return refuse("action", `${at} has no action`);
  if (!isRecord(action.event) && "functionCall" in action) {
    const call = action.functionCall;
    if (!isRecord(call) || typeof call.call !== "string") {
      return refuse("action", `${at} has a function call that is not {call, args}`);
    }
    if (call.call !== "openUrl") return { event: { name: UNSUPPORTED } }; // ignored (ADR 0013 rule 7)
    const url = isRecord(call.args) ? call.args.url : undefined;
    // literal only: a bound URL would be read from the data model at another time than it is checked
    const safe = typeof url === "string" ? safeHttpUrl(url) : undefined;
    if (safe === undefined) {
      return refuse("url", `${at} opens a URL that is not a literal absolute http(s) URL`);
    }
    return { event: { name: OPEN_URL, context: { url: safe } } };
  }
  const event = isRecord(action.event) ? action.event : action;
  if (event.userMessage !== undefined) {
    const text = event.userMessage;
    if (typeof text !== "string" || text === "" || text.length > MAX_USER_MESSAGE) {
      return refuse(
        "action",
        `${at} has a userMessage that is not text of 1 to ${MAX_USER_MESSAGE} characters`,
      );
    }
    return { event: { name: USER_MESSAGE, context: { text } } };
  }
  const name = event.name;
  if (typeof name !== "string" || name === "" || bytes(name) > MAX_ID_BYTES) {
    return refuse(
      "action",
      `${at} has an action name that is not 1 to ${MAX_ID_BYTES} bytes of text`,
    );
  }
  if (name.startsWith(RESERVED_PREFIX)) {
    return refuse(
      "action",
      `${at} names an action ${clip(name)}: the prefix ${RESERVED_PREFIX} is reserved`,
    );
  }
  const context = event.context;
  if (context !== undefined && !isRecord(context)) {
    return refuse("action", `${at} has an action context that is not an object`);
  }
  return {
    event: { name, ...(context !== undefined ? { context: rewriteContext(context, inputs) } : {}) },
  };
}

type Ctx = {
  components: Map<string, Rec>;
  templated: Set<string>;
  nodes: number;
  dangling: boolean;
  drawn: Set<string>;
};

/**
 * The components that are heavy to draw (a diff, a block of code, a table, a chart, a picture, a video). A reference
 * counts as one node, so a heavy one referenced again would be drawn again at full size: it is drawn once.
 */
const DRAWN_ONCE = new Set([
  "Code",
  "Diff",
  "Terminal",
  "Table",
  "Chart",
  "Gallery",
  "Video",
  "Audio",
  "FileCard",
]);

/** The walk from `root`: levels, nodes after expansion, cycles, templates, URLs. */
function walk(
  ctx: Ctx,
  id: string,
  data: unknown,
  depth: number,
  path: Set<string>,
  inTemplate: boolean,
): void {
  if (depth > MAX_DEPTH) refuse("depth", `more than ${MAX_DEPTH} levels deep`);
  if (path.has(id)) refuse("cycle", `component ${clip(id)} contains itself`);
  const c = ctx.components.get(id);
  if (!c) {
    ctx.dangling = true; // not sent (yet)
    return;
  }
  if (++ctx.nodes > MAX_NODES) {
    refuse("expansion", `more than ${MAX_NODES} nodes after expanding templates and references`);
  }
  if (typeof c.component === "string" && DRAWN_ONCE.has(c.component)) {
    if (ctx.drawn.has(id)) {
      refuse(
        "expansion",
        `component ${clip(id)} (${c.component}) is reached more than once; a ${c.component} is drawn once`,
      );
    }
    ctx.drawn.add(id);
  }
  if (inTemplate) ctx.templated.add(id);
  for (const key of URL_KEYS) {
    if (!Object.hasOwn(c, key)) continue;
    const value = resolveValue(c[key], data);
    if (value !== undefined && safeHttpUrl(value) === undefined) {
      refuse("url", `component ${clip(id)} names a URL that is not an absolute http(s) URL`);
    }
  }
  path.add(id);
  const kids = c.children;
  if (Array.isArray(kids)) {
    for (const kid of kids) {
      if (typeof kid !== "string")
        refuse("shape", `component ${clip(id)} has a child that is not an id`);
      walk(ctx, kid as string, data, depth + 1, path, inTemplate);
    }
  } else if (kids !== undefined) {
    const template = isTemplate(kids);
    if (!template)
      refuse("shape", `component ${clip(id)} has children that are neither a list nor a template`);
    const { componentId, path: at } = template as { componentId: string; path: string };
    const list = resolvePointer(data, at);
    if (!Array.isArray(list))
      refuse("template", `the template of ${clip(id)} does not read a list`);
    const items = list as unknown[];
    if (items.length > MAX_TEMPLATE_ITEMS) {
      refuse(
        "template",
        `the template of ${clip(id)} has ${items.length} items; the limit is ${MAX_TEMPLATE_ITEMS}`,
      );
    }
    for (const item of items) {
      // each item is a node of its own (the wrapper the converter draws), then its subtree
      if (++ctx.nodes > MAX_NODES) {
        refuse(
          "expansion",
          `more than ${MAX_NODES} nodes after expanding templates and references`,
        );
      }
      walk(ctx, componentId, item, depth + 1, path, true);
    }
  }
  if (c.child !== undefined) {
    if (typeof c.child !== "string")
      refuse("shape", `component ${clip(id)} has a child that is not an id`);
    walk(ctx, c.child as string, data, depth + 1, path, inTemplate);
  }
  // the child of each tab, and of each item of an accordion, is a component of the surface too
  for (const key of ["tabs", "items"]) {
    const entries = c[key];
    if (!Array.isArray(entries)) continue;
    for (const entry of entries) {
      const child = isRecord(entry) ? entry.child : undefined;
      if (typeof child === "string") walk(ctx, child, data, depth + 1, path, inTemplate);
    }
  }
  path.delete(id);
}

export type PrepareOptions = {
  /** This build's catalog; the real one unless a test brings its own. */
  catalog?: OwnCatalog;
  /** The version of the catalog the thread has recorded (`STATE_SNAPSHOT.thread.uiCatalog`), if any. */
  threadVersion?: number | undefined;
  /**
   * The files the thread holds (ADR 0032): an `Image` may name only one of these, and only an image.
   * Absent means none: a surface with an `Image` is then refused.
   */
  files?: readonly Pick<KeptFile, "sha256" | "preview" | "mimeType">[] | undefined;
};

export function prepareSurface(operations: unknown, options: PrepareOptions = {}): Prepared {
  try {
    return prepare(operations, options);
  } catch (e) {
    if (e instanceof Refused) return { kind: "refused", rule: e.rule, reason: e.reason };
    // The validator itself failed on input it did not foresee: refuse, never draw.
    return { kind: "refused", rule: "shape", reason: "the operations could not be read" };
  }
}

/**
 * The rules of a Choices that JSON Schema cannot say, and the name of its action: unique ids and
 * values, the counts a type allows, a default that is an option, and a picture for every tile of a
 * deck that is an image of this thread (rule `artifact`, as for an `Image`: never half drawn).
 */
function checkChoices(id: string, c: Rec, files: PrepareOptions["files"]) {
  const spec = readChoices(c);
  if (!spec)
    return refuse("schema", `component ${clip(id)} (Choices) has no questions it can read`);
  const duplicate = duplicateIn(spec);
  if (duplicate) return refuse("schema", `component ${clip(id)} (Choices): ${duplicate}`);
  const limit = limitsIn(spec);
  if (limit) return refuse("schema", `component ${clip(id)} (Choices): ${limit}`);
  for (const q of spec.questions) {
    for (const o of q.options) {
      if (!o.image) continue;
      const file = files?.find((f) => f.sha256 === o.image?.sha256);
      const at = `the option ${clip(o.value)} of the question ${clip(q.id)} of ${clip(id)} (Choices)`;
      if (!file) {
        return refuse("artifact", `${at} names a file that is not one of this thread's files`);
      }
      if (file.preview !== "image") {
        return refuse("artifact", `${at} names a file that is not an image`);
      }
    }
  }
  const name = spec.actionName;
  if (bytes(name) > MAX_ID_BYTES) {
    return refuse("action", `component ${clip(id)} has an action name over ${MAX_ID_BYTES} bytes`);
  }
  if (name.startsWith(RESERVED_PREFIX)) {
    return refuse(
      "action",
      `component ${clip(id)} names an action ${clip(name)}: the prefix ${RESERVED_PREFIX} is reserved`,
    );
  }
  return undefined;
}

/**
 * What JSON Schema cannot say about Cards: a card's link must be an absolute http(s) URL by the
 * rule of ADR 0013 (the schema only checks how it starts). The links of the top level of a
 * component are checked by the walk; these are one level down.
 */
function checkCards(id: string, c: Rec) {
  if (!readCards(c))
    return refuse("schema", `component ${clip(id)} (Cards) has no cards it can read`);
  const bad = firstBadUrl(c);
  if (bad !== undefined) {
    return refuse(
      "url",
      `component ${clip(id)} (Cards) names, in card ${bad + 1}, a URL that is not an absolute http(s) URL`,
    );
  }
  return undefined;
}

/**
 * What JSON Schema cannot say about a FileCard (catalog v6): its `artifact` is the hash of a file this
 * thread holds, of any kind (an Image must be an image; ADR 0032).
 */
function checkFileCard(id: string, c: Rec, files: PrepareOptions["files"]) {
  if (!files?.some((f) => f.sha256 === c.artifact)) {
    return refuse(
      "artifact",
      `component ${clip(id)} (FileCard) names a file that is not one of this thread's files`,
    );
  }
  return undefined;
}

/** What JSON Schema cannot say about a Table (catalog v6): each column's key is used once. */
function checkTable(id: string, c: Rec) {
  const keys = new Set<string>();
  for (const column of Array.isArray(c.columns) ? c.columns : []) {
    const key = isRecord(column) ? column.key : undefined;
    if (typeof key !== "string") continue;
    if (keys.has(key)) {
      return refuse("schema", `component ${clip(id)} (Table) has the column ${clip(key)} twice`);
    }
    keys.add(key);
  }
  return undefined;
}

/**
 * What JSON Schema cannot say about a Chart (catalog v6): each series has a value for each label; a pie has one
 * series, of values none of which is negative and at least one above zero (a pie of nothing draws nothing).
 */
function checkChart(id: string, c: Rec) {
  const labels = Array.isArray(c.labels) ? c.labels.length : 0;
  const series = Array.isArray(c.series) ? c.series : [];
  for (const s of series) {
    const count = isRecord(s) && Array.isArray(s.values) ? s.values.length : 0;
    if (count !== labels) {
      return refuse(
        "schema",
        `component ${clip(id)} (Chart) has a series of ${count} values for ${labels} labels`,
      );
    }
    const values = isRecord(s) && Array.isArray(s.values) ? s.values : [];
    if (values.some((v) => typeof v === "number" && v !== 0 && Math.abs(v) < 1e-6)) {
      return refuse(
        "schema",
        `component ${clip(id)} (Chart) has a value between 0 and 1e-6; a value is 0 or at least 1e-6 in size`,
      );
    }
  }
  if (c.type === "pie") {
    if (series.length !== 1) {
      return refuse(
        "schema",
        `component ${clip(id)} (Chart) is a pie of ${series.length} series; a pie has one`,
      );
    }
    const values = isRecord(series[0]) && Array.isArray(series[0].values) ? series[0].values : [];
    if (values.some((v) => typeof v === "number" && v < 0)) {
      return refuse("schema", `component ${clip(id)} (Chart) is a pie with a negative value`);
    }
    if (!values.some((v) => typeof v === "number" && v > 0)) {
      return refuse("schema", `component ${clip(id)} (Chart) is a pie with no value above zero`);
    }
  }
  return undefined;
}

/** What JSON Schema cannot say about a Gallery (catalog v6): each picture is an image of this thread. */
function checkGallery(id: string, c: Rec, files: PrepareOptions["files"]) {
  const images = Array.isArray(c.images) ? c.images : [];
  for (const [i, image] of images.entries()) {
    const file = files?.find((f) => f.sha256 === (isRecord(image) ? image.artifact : undefined));
    if (!file) {
      return refuse(
        "artifact",
        `component ${clip(id)} (Gallery) picture ${i + 1} names a file that is not one of this thread's files`,
      );
    }
    if (file.preview !== "image") {
      return refuse(
        "artifact",
        `component ${clip(id)} (Gallery) picture ${i + 1} names a file that is not an image`,
      );
    }
  }
  return undefined;
}

/** What JSON Schema cannot say about a Video or an Audio (catalog v6): its file is a video, or a sound, of this thread. */
function checkMedia(id: string, type: "Video" | "Audio", c: Rec, files: PrepareOptions["files"]) {
  const file = files?.find((f) => f.sha256 === c.artifact);
  if (!file) {
    return refuse(
      "artifact",
      `component ${clip(id)} (${type}) names a file that is not one of this thread's files`,
    );
  }
  const kind = type === "Video" ? "video/" : "audio/";
  if (!file.mimeType?.startsWith(kind)) {
    return refuse(
      "artifact",
      `component ${clip(id)} (${type}) names a file that is not ${type === "Video" ? "a video" : "a sound"}`,
    );
  }
  return undefined;
}

/**
 * What JSON Schema cannot say about an Image (ADR 0032, catalog v4): its `artifact` must be the
 * hash of a file this thread holds, and that file must be an image. Never a URL: the schema has no
 * member for one, and the renderer fetches only the file's own `href`.
 */
function checkImage(id: string, c: Rec, files: PrepareOptions["files"]) {
  const file = files?.find((f) => f.sha256 === c.artifact);
  if (!file) {
    return refuse(
      "artifact",
      `component ${clip(id)} (Image) names a file that is not one of this thread's files`,
    );
  }
  if (file.preview !== "image") {
    return refuse("artifact", `component ${clip(id)} (Image) names a file that is not an image`);
  }
  return undefined;
}

/** The first component whose name the catalog does not have (a non-text name counts as one). */
function firstUnknown(components: ReadonlyMap<string, Rec>, catalog: CompiledCatalog) {
  for (const c of components.values()) {
    const type = c.component;
    if (typeof type !== "string") return String(type);
    if (!catalog.has(type)) return type;
  }
  return undefined;
}

/**
 * The catalog the surface names, by its `createSurface` operations (all of them: a later one
 * starts the surface over, so a catalog that changes half way would change what the earlier
 * checks meant). None named is the basic catalog, as it always was; the basic ids are the basic
 * catalog; the id of `own` is ours; any other is refused.
 */
function catalogOf(operations: readonly Rec[], own: OwnCatalog): "basic" | "own" {
  const named = new Set<string | undefined>();
  for (const op of operations) {
    const body = op.createSurface;
    if (!isRecord(body)) continue;
    const id = body.catalogId;
    if (id !== undefined && typeof id !== "string") {
      return refuse("catalog", "the surface names a catalog that is not text");
    }
    named.add(id);
  }
  if (named.size > 1) return refuse("catalog", "the surface names more than one catalog");
  const [id] = named;
  if (id === undefined || (BASIC_CATALOG_IDS as readonly string[]).includes(id)) return "basic";
  if (id === own.catalogId) return "own";
  return refuse(
    "catalog",
    `the surface names the catalog ${clip(id, 80)}, which this app does not have`,
  );
}

function prepare(operations: unknown, options: PrepareOptions): Prepared {
  const own = options.catalog ?? OWN_CATALOG;
  if (operations === undefined) return { kind: "pending" };
  if (!Array.isArray(operations)) return refuse("shape", "the operations are not a list");

  // 1. size
  const size = bytes(JSON.stringify(operations));
  if (size > MAX_BYTES) refuse("size", `${size} bytes of operations; the limit is ${MAX_BYTES}`);

  // 2. the shape of every operation, before the reducer sees any
  const normalised: Rec[] = [];
  let surfaceId: string | undefined;
  let deleted = false;
  for (const [i, op] of operations.entries()) {
    if (!isRecord(op)) return refuse("shape", `operation ${i} is not an object`);
    const version = op.version;
    if (typeof version !== "string" || !(VERSIONS as readonly string[]).includes(version)) {
      return refuse("version", `operation ${i} has a version the renderer does not read`);
    }
    const keys = Object.keys(op).filter((k) => k !== "version");
    const key = keys[0];
    if (keys.length !== 1 || key === undefined) {
      return refuse("shape", `operation ${i} does not have exactly one operation`);
    }
    if (!OPERATIONS.includes(key))
      return refuse("shape", `operation ${i} is ${clip(key)}, which is unknown`);
    const body = op[key];
    const id = isRecord(body) ? body.surfaceId : undefined;
    if (typeof id !== "string" || id === "" || bytes(id) > MAX_ID_BYTES) {
      return refuse("shape", `operation ${i} has no surface id of 1 to ${MAX_ID_BYTES} bytes`);
    }
    if (surfaceId !== undefined && surfaceId !== id) {
      return refuse("surfaces", "one activity holds the operations of more than one surface");
    }
    surfaceId = id;
    if (key === "deleteSurface") deleted = true;
    normalised.push({ ...op, version: version === "v0.9.1" ? "v0.9" : version });
  }
  if (normalised.length === 0 || surfaceId === undefined) return { kind: "pending" };
  const mode = catalogOf(normalised, own);
  const catalog = compiledOf(own.catalog);

  // 3. the operations applied: anything the reducer had to skip is a defect, not a detail
  const { state, warnings } = applyA2uiOperations(new Map(), normalised);
  if (warnings[0] !== undefined) return refuse("shape", warnings[0]);
  const surface = state.get(surfaceId);
  if (!surface) return deleted ? { kind: "deleted" } : { kind: "pending" };
  const components = surface.components;

  // 4. components and vocabulary
  if (components.size > MAX_COMPONENTS) {
    refuse("components", `${components.size} components; the limit is ${MAX_COMPONENTS}`);
  }
  const lowered = new Map<string, Rec>();
  const inputs = new Map<string, string>(); // component id -> field key
  const inputKeys = new Set<string>(); // the JSON Pointers that inputs are bound to
  const fields: Record<string, unknown> = {};
  if (mode === "own") {
    // a component of a newer catalog comes before any other defect: it is not the agent's fault
    const unknown = firstUnknown(components, catalog);
    if (unknown !== undefined) {
      if (options.threadVersion !== undefined && options.threadVersion > own.version) {
        return { kind: "newer", component: unknown };
      }
      return refuse(
        "catalog",
        `component ${clip(unknown)} is not in this app's catalog (version ${own.version})`,
      );
    }
  }
  for (const [id, c] of components) {
    const type = c.component;
    if (mode === "own") {
      const broken = catalog.check(type as string, c);
      if (broken !== undefined) {
        return refuse("schema", `component ${clip(id)} (${type}) ${broken}`);
      }
      if (type === "Choices") checkChoices(id, c, options.files);
      else if (type === "Cards") checkCards(id, c);
      else if (type === "Image") checkImage(id, c, options.files);
      else if (type === "FileCard") checkFileCard(id, c, options.files);
      else if (type === "Table") checkTable(id, c);
      else if (type === "Gallery") checkGallery(id, c, options.files);
      else if (type === "Video" || type === "Audio") checkMedia(id, type, c, options.files);
      else if (type === "Chart") checkChart(id, c);
    } else if (typeof type !== "string" || !(VOCABULARY as readonly string[]).includes(type)) {
      return refuse(
        catalog.has(String(type)) ? "catalog" : "vocabulary",
        catalog.has(String(type))
          ? `component ${clip(String(type))} is of this app's catalog, which the surface does not name`
          : `component ${clip(String(type))} is not in the vocabulary`,
      );
    }
    if (bytes(id) > MAX_ID_BYTES)
      return refuse("shape", `a component id is over ${MAX_ID_BYTES} bytes`);
    // 5. function values (formatString, ...): the pinned converter cannot run them
    if (hasFunctionValue(c, 0, "action")) {
      return refuse(
        "function",
        `component ${clip(id)} uses a function value, which the renderer cannot run`,
      );
    }
    if (isRecord(c.action) && hasFunctionValue(c.action, 0, "functionCall")) {
      return refuse(
        "function",
        `component ${clip(id)} uses a function value, which the renderer cannot run`,
      );
    }
    if (type === "TextField" || type === "CheckBox") {
      const bound = c.value ?? c.text;
      let key = `#${id}`;
      if (isBinding(bound)) {
        if (!bound.path.startsWith("/")) {
          return refuse("field", `input ${clip(id)} is bound to a relative path`);
        }
        key = bound.path;
        inputKeys.add(key);
      }
      inputs.set(id, key);
      fields[key] = isBinding(bound) ? resolvePointer(surface.dataModel, bound.path) : bound;
    }
  }

  // 6. actions, inputs and the shape the converter reads
  let eventActions = 0;
  for (const [id, c] of components) {
    let next: Rec = c;
    if (isTemplate(c.children) && !isRecord((c.children as Rec).template)) {
      const t = isTemplate(c.children);
      next = { ...next, children: { template: { componentId: t?.componentId, path: t?.path } } };
    }
    if (c.component === "Button") {
      const action = lowerAction(id, c.action, inputKeys) as { event: { name: string } };
      if (![OPEN_URL, USER_MESSAGE, UNSUPPORTED].includes(action.event.name)) eventActions++;
      const style =
        c.variant === "primary" ? "primary" : c.variant === "borderless" ? "ghost" : "secondary";
      next = { ...next, action, ...(c.buttonStyle === undefined ? { buttonStyle: style } : {}) };
    } else if (c.component === "TextField" || c.component === "CheckBox") {
      next = {
        ...next,
        component: c.component === "TextField" ? TEXT_FIELD : CHECK_BOX,
        fieldKey: inputs.get(id),
      };
    } else if (mode === "own" && c.component === "Choices") {
      // the converter keeps an unknown component's properties but not its id, which is the
      // `sourceComponentId` of the answer's action
      eventActions++;
      next = { ...next, component: CHOICES, componentId: id };
    } else if (mode === "own" && c.component === "Cards") {
      // output only: no action, nothing to send; kept by the converter under its own name
      next = { ...next, component: CARDS };
    } else if (mode === "own" && c.component === "Mermaid") {
      next = { ...next, component: MERMAID };
    } else if (mode === "own" && c.component === "Image") {
      next = { ...next, component: IMAGE };
    } else if (mode === "own" && (c.component === "Tabs" || c.component === "Accordion")) {
      // the converter draws a component's `children`, in order: the child of each tab or item
      const entries = (c.component === "Tabs" ? c.tabs : c.items) as Rec[];
      next = {
        ...next,
        component: c.component === "Tabs" ? TABS : ACCORDION,
        children: entries.map((entry) => entry.child),
      };
    } else if (mode === "own" && typeof c.component === "string" && OUTPUT_NAMES.has(c.component)) {
      next = { ...next, component: OUTPUT_NAMES.get(c.component) };
    }
    if (next !== c) lowered.set(id, next);
  }

  // 7. the walk: levels, nodes after expansion, cycles, templates, URLs
  const ctx: Ctx = { components, templated: new Set(), nodes: 0, dangling: false, drawn: new Set() };
  if (!components.has("root")) return { kind: "pending" };
  walk(ctx, "root", surface.dataModel, 1, new Set(), false);
  for (const id of inputs.keys()) {
    if (ctx.templated.has(id)) return refuse("field", `input ${clip(id)} is inside a template`);
  }
  if (ctx.dangling) return { kind: "pending" };

  // 8. only now the converter, on what was checked and lowered
  for (const [id, c] of lowered) components.set(id, c);
  const { spec, warnings: conversion } = convertSurfaceToUISpec(surface, {
    keepUnknownComponents: true,
  });
  if (conversion[0] !== undefined) return refuse("shape", conversion[0]);
  if (!spec) return { kind: "pending" };
  return { kind: "surface", surfaceId, spec, fields, eventActions };
}

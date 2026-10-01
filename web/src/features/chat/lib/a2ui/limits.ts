/**
 * The limits and names of the A2UI validator (ADR 0013, docs/api/agui.md "A2UI (generative UI)",
 * web/README.md "A2UI surfaces"). The orchestrator refuses a payload over `MAX_BYTES` before it is
 * stored; the renderer checks the rest, because only a renderer knows the vocabulary and the data.
 */

/** Serialised size of the operations of one surface. */
export const MAX_BYTES = 64 * 1024;
/** Components a surface holds once its operations are applied. */
export const MAX_COMPONENTS = 400;
/** Nodes a surface renders after template children are expanded over their lists. */
export const MAX_NODES = 2000;
/** Items one template may expand over. */
export const MAX_TEMPLATE_ITEMS = 100;
/** Levels of the component tree; the root is level 1. */
export const MAX_DEPTH = 24;
/** Bytes of a surface id, an action name and a component id (what the orchestrator accepts). */
export const MAX_ID_BYTES = 256;
/** Bytes of the context an action carries (what the orchestrator accepts). */
export const MAX_CONTEXT_BYTES = 16 * 1024;
/** Characters of a `userMessage` put in the message box. */
export const MAX_USER_MESSAGE = 4000;
/** Characters of a URL a surface may name. */
export const MAX_URL = 2048;

/**
 * The components of the basic catalog the renderer draws, and nothing else. Any other component
 * refuses the whole surface (never silently dropped). Not drawn, so not here: `Icon`, `Tabs`,
 * `Modal`, `Slider`, `DateTimeInput`, `ChoicePicker`, `AudioPlayer`, `Video`.
 */
export const VOCABULARY = [
  "Text",
  "Image",
  "Row",
  "Column",
  "List",
  "Card",
  "Divider",
  "Button",
  "TextField",
  "CheckBox",
] as const;
export type Vocabulary = (typeof VOCABULARY)[number];

/**
 * The ids the basic catalog goes by (the spellings of the three A2UI versions, as the orchestrator
 * lists them in `core/src/ui.rs`): a surface that names one, or none, is drawn with the vocabulary
 * above. Any other id must be this app's own catalog (`catalog/`), or the surface is refused.
 */
export const BASIC_CATALOG_IDS = [
  "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json",
  "https://a2ui.org/specification/v0_9_1/catalogs/basic/catalog.json",
  "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json",
] as const;

/** The A2UI versions the renderer reads. `v0.9.1` is read as `v0.9`. */
export const VERSIONS = ["v0.9", "v0.9.1", "v1.0"] as const;

/**
 * Action names the renderer gives to what it lowers before conversion (an `openUrl` call, a
 * `userMessage`, a call it does not run). An agent's own event may not use the prefix.
 */
export const RESERVED_PREFIX = "vymalo:";
export const OPEN_URL = `${RESERVED_PREFIX}openUrl`;
export const USER_MESSAGE = `${RESERVED_PREFIX}userMessage`;
export const UNSUPPORTED = `${RESERVED_PREFIX}unsupported`;

/** The component names the renderer gives to the inputs it keeps out of the library's converter. */
export const TEXT_FIELD = "vymalo.TextField";
export const CHECK_BOX = "vymalo.CheckBox";
/** The components of this app's own catalog that the converter keeps (it has no such component). */
export const CHOICES = "vymalo.Choices";

/** The marker that stands for "the current value of this input" in an action's context. */
export const FIELD = "$field";

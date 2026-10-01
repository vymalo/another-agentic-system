/**
 * The `Mermaid` component of the UI catalog (docs/api/ui-catalog-v1.md, version 3): a graph an
 * agent wrote in mermaid's text language, drawn in the browser as an image.
 *
 * This file is the pure part: what a Mermaid says, the configuration the renderer gives mermaid
 * (security first, then the colours of the page), and what is done with the SVG mermaid returns.
 * Drawing it (the lazy import, the queue, the DOM) is `components/surface/mermaid.tsx` and
 * `lib/a2ui/mermaid-render.ts`.
 *
 * What stands between an agent's text and the page (ADR 0013 rules 4 to 6), each of them tested:
 * 1. `securityLevel: "strict"`, which mermaid does not let a diagram's own `%%{init}%%` directive
 *    or front matter change (`securityLevel` is in mermaid's default `secure` list);
 * 2. no HTML labels (`htmlLabels: false`, so no `foreignObject` for a label), and the settings a
 *    directive could use to switch that back on, to pick another theme or to inject CSS are added
 *    to `secure`, which mermaid refuses a directive to touch;
 * 3. the SVG becomes the `src` of an `<img>` (a data URL), never markup of the page: an image
 *    cannot run a script, follow a link or load anything, whatever is in it. The SVG is read as a tree
 *    first and refused for a script, an event handler or CSS that loads, as a second line;
 * 4. at most 20,000 characters (the schema's limit, `maxTextSize` the same).
 */

type Rec = Record<string, unknown>;
const str = (v: unknown): string | undefined => (typeof v === "string" ? v : undefined);

/** The characters of a graph's source (the schema's `maxLength`, and mermaid's `maxTextSize`). */
export const MAX_MERMAID_CODE = 20_000;

export type MermaidSpec = { title?: string; code: string; caption?: string };

/** The Mermaid a component object says, or undefined when it is not one. */
export function readMermaid(component: Rec): MermaidSpec | undefined {
  const code = str(component.code);
  if (code === undefined || code.trim() === "") return undefined;
  const title = str(component.title);
  const caption = str(component.caption);
  return { ...(title ? { title } : {}), code, ...(caption ? { caption } : {}) };
}

// ---- the colours --------------------------------------------------------------------------

export type Scheme = "light" | "dark";

/**
 * The page's colours a graph is drawn in: the custom properties of `app/globals.css` (web/DESIGN.md,
 * "Colour"). They are read from the page when the graph is drawn, so a change of scheme or of token
 * is followed without a copy of the palette here.
 */
export const TOKEN_NAMES = [
  "background",
  "foreground",
  "card",
  "muted",
  "muted-foreground",
  "border",
  "input",
  "brand",
  "sidebar",
  "sidebar-accent",
] as const;
export type TokenName = (typeof TOKEN_NAMES)[number];
export type Tokens = Record<TokenName, string>;

/** Used for a token the page does not define (a test, a page that failed to load its CSS). */
const FALLBACK: Record<Scheme, Tokens> = {
  light: {
    background: "#ffffff",
    foreground: "#1a1d1a",
    card: "#ffffff",
    muted: "#f3f2ec",
    "muted-foreground": "#5c6058",
    border: "#e7e5dd",
    input: "#dad8cf",
    brand: "#3f7341",
    sidebar: "#f6f5f0",
    "sidebar-accent": "#eae8e0",
  },
  dark: {
    background: "#141614",
    foreground: "#e8e9e4",
    card: "#1a1d1a",
    muted: "#232723",
    "muted-foreground": "#a6aba3",
    border: "#2e332e",
    input: "#3a3f3a",
    brand: "#8dc58b",
    sidebar: "#1b1e1b",
    "sidebar-accent": "#2a2e2a",
  },
};

/** mermaid reads colours with khroma: hex, `rgb()` and `hsl()`; anything else would throw there. */
const COLOUR = /^(#[0-9a-f]{3,8}|(rgb|hsl)a?\([0-9a-z%.,\s/-]+\))$/i;

/** The tokens of `read` (a custom property's value, or ""), each falling back to the palette of the scheme. */
export function tokensFrom(read: (name: string) => string, scheme: Scheme): Tokens {
  const out = { ...FALLBACK[scheme] };
  for (const name of TOKEN_NAMES) {
    const value = read(`--${name}`).trim();
    if (COLOUR.test(value)) out[name] = value;
  }
  return out;
}

// ---- the configuration --------------------------------------------------------------------

/**
 * Mermaid's own list of settings a diagram cannot change (`secure` in its default configuration,
 * *verified 2026-10-01* in `mermaid@12.0.0`, `dist/chunks/mermaid.core/chunk-O7XYJQB3.mjs`), and the
 * ones this app adds: the labels' kind, the theme and the CSS, the font, the look and the layout,
 * and the sanitizer's configuration. A directive naming one of them is dropped, at any depth.
 */
export const SECURE_KEYS = [
  "secure",
  "securityLevel",
  "startOnLoad",
  "maxTextSize",
  "suppressErrorRendering",
  "maxEdges",
  "htmlLabels",
  "theme",
  "themeVariables",
  "themeCSS",
  "fontFamily",
  "darkMode",
  "look",
  "layout",
  "dompurifyConfig",
];

/** A system font: an SVG drawn as an image cannot use the fonts of the page. */
const FONT = 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif';

/**
 * What `mermaid.initialize` gets. Typed loosely (`Rec`) so that this file needs no import of mermaid:
 * the library is loaded lazily, and a type import would be the only thing tying it to the bundle.
 */
export function mermaidConfig(tokens: Tokens, scheme: Scheme): Rec {
  return {
    startOnLoad: false,
    securityLevel: "strict",
    htmlLabels: false,
    maxTextSize: MAX_MERMAID_CODE,
    suppressErrorRendering: true,
    secure: SECURE_KEYS,
    theme: "base",
    darkMode: scheme === "dark",
    // plain shapes, no drop shadows (`neo` is mermaid 12's default), and the layout that ships in the core chunk
    look: "classic",
    layout: "dagre",
    fontFamily: FONT,
    themeVariables: {
      fontFamily: FONT,
      fontSize: "14px",
      background: tokens.card,
      textColor: tokens.foreground,
      primaryColor: tokens.muted,
      primaryTextColor: tokens.foreground,
      primaryBorderColor: tokens.input,
      secondaryColor: tokens["sidebar-accent"],
      secondaryTextColor: tokens.foreground,
      secondaryBorderColor: tokens.input,
      tertiaryColor: tokens.sidebar,
      tertiaryTextColor: tokens.foreground,
      tertiaryBorderColor: tokens.border,
      mainBkg: tokens.muted,
      nodeBorder: tokens.input,
      lineColor: tokens["muted-foreground"],
      clusterBkg: tokens.background,
      clusterBorder: tokens.border,
      edgeLabelBackground: tokens.card,
      titleColor: tokens.foreground,
    },
  };
}

// ---- the picture --------------------------------------------------------------------------

/** What an `<img>` needs to show an SVG: its source and its natural size. */
export type SvgImage = { src: string; width: number; height: number };

const SVG_NS = "http://www.w3.org/2000/svg";
/** The most characters of SVG that become an image (a data URL is held in memory twice). */
const MAX_SVG = 2_000_000;

/** Elements that have no business in a picture of a graph, in any case. Their presence refuses it. */
const FORBIDDEN_ELEMENTS = new Set([
  "script",
  "foreignobject",
  "iframe",
  "object",
  "embed",
  "audio",
  "video",
  "canvas",
  "link",
  "meta",
  "base",
]);

/** CSS that would load something: an `@import`, or a `url()` that is not a reference into the picture. */
const LOADING_CSS = /@import|url\((?![\s"']*#)/i;

/**
 * The SVG mermaid returned, as an image source, or `undefined` when it must not be drawn.
 *
 * mermaid's string is meant for `innerHTML`: it writes `viewbox`, `refx` and `markerwidth` in lower
 * case, which an HTML parser puts right and an image (parsed as XML, where case counts) does not,
 * so a picture made from the string as it is has no `viewBox` and no arrow heads. It is parsed here
 * as HTML, which restores the case, and written out as XML. On the way it is read as what it
 * is, a tree, instead of searched as text:
 * - it must be one `<svg>` element and nothing else, within a size, with a `viewBox` that sizes it;
 * - a script, a `foreignObject`, a frame, an embed, a stylesheet link and the like refuse it, and so do
 *   an event handler attribute (`onclick`, `onerror`) and CSS that loads something (`@import`, a
 *   `url()` that is not `#id`); a link's `href` that does not point into the picture is dropped;
 * - the root's `width`, `height` and `style` (mermaid writes `width="100%"` and `max-width: …px`)
 *   are replaced by the size of the `viewBox`, so the image has a natural size and the page only
 *   ever shrinks it.
 *
 * None of that is what keeps the page safe: an `<img>` cannot run a script, follow a link or load a
 * resource whatever its SVG holds. It is a second line, and it makes the failure visible.
 */
export function svgImage(svg: string): SvgImage | undefined {
  if (svg.length > MAX_SVG) return undefined;
  const body = new DOMParser().parseFromString(svg, "text/html").body;
  const root = body.firstElementChild;
  if (
    !root ||
    body.children.length !== 1 ||
    root.localName !== "svg" ||
    root.namespaceURI !== SVG_NS
  ) {
    return undefined;
  }
  for (const el of [root, ...root.querySelectorAll("*")]) {
    if (FORBIDDEN_ELEMENTS.has(el.localName.toLowerCase())) return undefined;
    if (el.localName === "style" && LOADING_CSS.test(el.textContent ?? "")) return undefined;
    for (const attr of Array.from(el.attributes)) {
      const name = attr.name.toLowerCase();
      if (name.startsWith("on")) return undefined;
      if (name === "style" && LOADING_CSS.test(attr.value)) return undefined;
      if ((name === "href" || name === "xlink:href") && !attr.value.startsWith("#")) {
        el.removeAttribute(attr.name); // a link out of the picture is not one
      }
    }
  }
  const [, , w, h] = (root.getAttribute("viewBox") ?? "")
    .trim()
    .split(/[\s,]+/)
    .map(Number);
  if (w === undefined || h === undefined || !(w > 0) || !(h > 0) || w > 20_000 || h > 20_000) {
    return undefined;
  }
  const width = Math.ceil(w);
  const height = Math.ceil(h);
  root.removeAttribute("style");
  root.setAttribute("width", String(width));
  root.setAttribute("height", String(height));
  const xml = new XMLSerializer().serializeToString(root);
  return { src: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(xml)}`, width, height };
}

// ---- errors -------------------------------------------------------------------------------

// biome-ignore lint/suspicious/noControlCharactersInRegex: stripping them is the point
const CONTROL = /[\u0000-\u0008\u000b-\u001f\u007f-\u009f]/g;

/**
 * Why a graph could not be drawn, as one short piece of text: mermaid's own message, the first
 * three lines (the place, the line of the source and a caret under it; what follows is a list of
 * every token that would have been accepted), without control characters.
 */
export function reasonOf(error: unknown): string {
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  const text = message.replace(CONTROL, "?").trim().split("\n").slice(0, 3).join("\n");
  return text === "" ? "mermaid could not read the graph" : text.slice(0, 200);
}

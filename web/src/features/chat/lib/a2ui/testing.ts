/**
 * Test support for the A2UI validator and renderer: surfaces to build, and the URLs a surface may
 * and may not name. Not part of the app bundle.
 */

import { OWN_CATALOG } from "./catalog";
import { BASIC_CATALOG_IDS } from "./limits";

type Rec = Record<string, unknown>;

export const SURFACE = "s1";
/** The id the basic catalog goes by in the surfaces the tests build (the `v0_9_1` spelling). */
export const BASIC_CATALOG: string = BASIC_CATALOG_IDS[1];
/** The id of this app's own catalog. */
export const OWN_CATALOG_ID = OWN_CATALOG.catalogId;

/** The operations of one surface: `createSurface`, its components, and optionally its data. */
export function surface(
  components: Rec[],
  dataModel?: unknown,
  version = "v0.9",
  catalogId: string = BASIC_CATALOG,
): Rec[] {
  return [
    { version, createSurface: { surfaceId: SURFACE, catalogId } },
    { version, updateComponents: { surfaceId: SURFACE, components } },
    ...(dataModel !== undefined
      ? [{ version, updateDataModel: { surfaceId: SURFACE, path: "/", contents: dataModel } }]
      : []),
  ];
}

export const text = (id: string, value: string): Rec => ({ id, component: "Text", text: value });
export const column = (id: string, children: string[] | Rec, extra: Rec = {}): Rec => ({
  id,
  component: "Column",
  children,
  ...extra,
});
export const button = (id: string, action: unknown, extra: Rec = {}): Rec => ({
  id,
  component: "Button",
  child: `${id}_label`,
  action,
  ...extra,
});
/** A button with its label: `[button, label]`. */
export const labelled = (id: string, label: string, action: unknown): Rec[] => [
  button(id, action),
  text(`${id}_label`, label),
];
export const event = (name: string, context?: Rec): Rec => ({
  event: { name, ...(context ? { context } : {}) },
});
export const openUrl = (url: unknown): Rec => ({
  functionCall: { call: "openUrl", args: { url } },
});

/** A surface whose root has these children and nothing else. */
export const withRoot = (children: string[], rest: Rec[]): Rec[] =>
  surface([column("root", children), ...rest]);

/** The serialised size of operations, as the validator measures it. */
export const sizeOf = (ops: unknown): number =>
  new TextEncoder().encode(JSON.stringify(ops)).length;

/** Absolute http(s) URLs: what a surface may name (as written, and as they normalise). */
export const GOOD_URLS: [string, string][] = [
  ["https://example.com/pull/1", "https://example.com/pull/1"],
  ["http://example.com", "http://example.com/"],
  ["HTTPS://Example.COM/A?b=1#c", "https://example.com/A?b=1#c"],
  ["HtTp://localhost:3000/x", "http://localhost:3000/x"],
  ["https://github.com/acme/demo/pull/1", "https://github.com/acme/demo/pull/1"],
];

/** Everything else, and the tricks that get a browser to run or fetch it anyway. */
export const BAD_URLS: string[] = [
  // executable and inline schemes, in every case
  "javascript:alert(1)",
  "JAVASCRIPT:alert(1)",
  "JaVaScRiPt:alert(1)",
  "jAvAsCrIpT:alert(1)",
  "data:text/html,<script>alert(1)</script>",
  "DaTa:text/html;base64,PHNjcmlwdD5hbGVydCgxKTwvc2NyaXB0Pg==",
  "vbscript:msgbox(1)",
  "file:///etc/passwd",
  "FILE:///C:/Windows/win.ini",
  "blob:https://example.com/1234",
  "about:blank",
  "chrome://settings",
  "ws://example.com/",
  "wss://example.com/",
  "ftp://example.com/x",
  "mailto:someone@example.com",
  "tel:+15550100",
  "sms:+15550100",
  "intent://scan/#Intent;scheme=zxing;end",
  // relative references and scheme-relative tricks
  "//evil.example/x",
  "///evil.example/x",
  "/relative/path",
  "relative/path",
  "./x",
  "../x",
  "?q=1",
  "#fragment",
  "\\\\evil.example\\share",
  "\\/evil.example",
  "/\\evil.example",
  // an http scheme without its slashes: browsers repair these, so they are refused
  "http:evil.example",
  "https:evil.example",
  "https:/evil.example",
  "https:\\\\evil.example",
  "http:",
  "https://",
  "http://:80/",
  // whitespace and control characters, before, inside and after
  " javascript:alert(1)",
  "\tjavascript:alert(1)",
  "\njavascript:alert(1)",
  "\rjavascript:alert(1)",
  "\u0000javascript:alert(1)",
  "\u001fjavascript:alert(1)",
  "java\tscript:alert(1)",
  "java\nscript:alert(1)",
  "java\rscript:alert(1)",
  "java\u0000script:alert(1)",
  "jav&#x09;ascript:alert(1)",
  " https://example.com",
  "\thttps://example.com",
  "\nhttps://example.com",
  "https://example.com ",
  "https://example.com\n",
  "https://exa mple.com",
  "https://example.com/a b",
  "https://example.com/\u0007",
  "https://example.com/\u007f",
  "https://example.com/\u0085",
  "\u00a0https://example.com",
  "\u2028https://example.com",
  "\ufeffhttps://example.com",
  // lookalike schemes and hosts
  "ｈｔｔｐｓ://example.com",
  "https＿://example.com",
  "xhttps://example.com",
  "https+evil://example.com",
  // credentials in front of the host
  "https://user:pass@example.com/",
  "https://trusted.example@evil.example/",
  "http://@example.com/",
  "https://example.com\\@evil.example/",
  "https://example.com/a\\b",
  // not a URL at all
  "",
  "   ",
  "null",
  "example.com",
  "http",
];

/** Values of another type than a string. */
export const NOT_TEXT: unknown[] = [
  undefined,
  null,
  0,
  1,
  true,
  {},
  [],
  ["https://example.com"],
  { href: "https://example.com" },
];

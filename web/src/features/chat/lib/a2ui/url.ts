import { MAX_URL } from "./limits";

/**
 * The URL as a link target, only when it is an absolute `http:` or `https:` URL (ADR 0013 rule 6).
 *
 * It is strict on purpose: anything that a browser would repair is refused rather than repaired.
 * - the text must start with `http://` or `https://` (any case: schemes are case-insensitive, and
 *   the result is the normalised `href`), so `javascript:`, `data:`, `file:`, `blob:`, `vbscript:`,
 *   `mailto:`, a relative path, `//host` and `http:host` are all refused;
 * - no character below U+0021 or from U+007F to U+009F, anywhere: a leading space, a tab or newline
 *   inside the scheme (`java\tscript:`) and every other control character are refused;
 * - no user information (`https://trusted.example@evil.example`), no backslash, and a host.
 */
export function safeHttpUrl(value: unknown): string | undefined {
  if (typeof value !== "string" || value.length === 0 || value.length > MAX_URL) return undefined;
  // biome-ignore lint/suspicious/noControlCharactersInRegex: refusing control characters is the point
  if (/[\u0000- \u007f-\u009f]/.test(value)) return undefined;
  if (!/^https?:\/\//i.test(value)) return undefined;
  // A backslash is a slash to a browser (`https://a.example\@b.example`), and an `@` in the
  // authority is user information, empty or not: neither is repaired, both are refused.
  if (value.includes("\\")) return undefined;
  if (
    value
      .slice(value.indexOf("//") + 2)
      .split(/[/?#]/, 1)[0]
      ?.includes("@")
  )
    return undefined;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return undefined;
  if (url.hostname === "" || url.username !== "" || url.password !== "") return undefined;
  return url.href;
}

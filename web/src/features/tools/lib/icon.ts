/*
 * The icon of an MCP server (ADR 0024, open question 38). The deployment's configuration gives it
 * as a `data:` URI of at most 8 KiB, and the screen draws exactly that: the browser decodes the
 * bytes it was given and requests nothing. A URL is never an icon (a request to it would tell
 * whoever runs the host that this person looked at the conversation), so an icon that is not a
 * `data:` image of the three kinds is no icon at all and the generic one is drawn. An SVG is drawn
 * through an `<img>`, which does not run its scripts or load what it names, and is never inlined.
 */

/** The bytes the contract allows an icon (`ToolServer.icon`, `maxLength: 8192`). */
export const ICON_MAX_LENGTH = 8192;

/** `data:image/(svg+xml|png|webp);base64,` and then base64 and nothing else (no parameters, no `;charset`). */
const DATA_ICON = /^data:image\/(?:svg\+xml|png|webp);base64,[A-Za-z0-9+/]+={0,2}$/;

/**
 * What an `<img src>` may be for this icon: the `data:` URI itself when it is one of the allowed
 * kinds and within the size, else `null` (draw the generic icon). Nothing is resolved, fetched or
 * rewritten.
 */
export function iconSrc(icon: string | undefined | null): string | null {
  if (typeof icon !== "string" || icon.length > ICON_MAX_LENGTH) return null;
  return DATA_ICON.test(icon) ? icon : null;
}

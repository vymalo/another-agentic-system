import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, it } from "vitest";
import { headerPolicy } from "./csp";
import { STATIC_SHELLS, staticTarget } from "./static-routes";

/*
 * The web image's `Caddyfile` and the e2e's `scripts/serve-static.ts` (which uses `static-routes.ts` and `csp.ts`) are one
 * server: the same rewrites of the shells and the same headers. A change to one that the other lacks fails here.
 */
const caddyfile = readFileSync(path.resolve("Caddyfile"), "utf8");

/** The `path_regexp` and `rewrite` pairs of the Caddyfile, by matcher name. */
function caddyRewrites(): Array<[RegExp, string]> {
  const shapes = new Map<string, RegExp>();
  for (const m of caddyfile.matchAll(/^\t@(\w+) path_regexp (\S+)$/gm)) {
    shapes.set(m[1] as string, new RegExp(m[2] as string));
  }
  return [...caddyfile.matchAll(/^\trewrite @(\w+) (\S+)$/gm)].map((m) => {
    const shape = shapes.get(m[1] as string);
    if (!shape) throw new Error(`rewrite @${m[1]} has no path_regexp`);
    return [shape, (m[2] as string).replace(/\{re\.\w+\.1\}/, "$1")];
  });
}

const caddyTarget = (pathname: string): string => {
  for (const [shape, target] of caddyRewrites()) {
    const match = shape.exec(pathname);
    if (match) return target.replace("$1", match[1] ?? "");
  }
  return pathname;
};

describe("the static server", () => {
  it("rewrites what the Caddyfile rewrites, no more and no less", () => {
    expect(caddyRewrites().map(([shape, target]) => [shape.source, target])).toEqual(
      STATIC_SHELLS.map(([shape, target]) => [shape.source, target]),
    );
  });

  it("answers any thread and any share link with its shell, and every other address with itself", () => {
    const cases: Array<[string, string]> = [
      ["/threads/0199c0de-0000-7000-8000-000000000001", "/threads/_.html"],
      ["/threads/0199c0de-0000-7000-8000-000000000001.txt", "/threads/_.txt"],
      [
        "/threads/0199c0de-0000-7000-8000-000000000001/__next.threads.txt",
        "/threads/_/__next.threads.txt",
      ],
      ["/s/abcDEF_123-x", "/s/_.html"],
      ["/s/abcDEF_123-x.txt", "/s/_.txt"],
      ["/", "/"],
      ["/auth/callback", "/auth/callback"],
      ["/_next/static/chunks/a.js", "/_next/static/chunks/a.js"],
      ["/threads/a/b/c", "/threads/a/b/c"],
      ["/threads/../config.json", "/threads/../config.json"],
    ];
    for (const [address, file] of cases) {
      expect(staticTarget(address), address).toBe(file);
      expect(caddyTarget(address), address).toBe(file);
    }
  });

  it("sends the header half of the policy, with the issuer's origin from the environment", () => {
    const header = /Content-Security-Policy "([^"]+)"/.exec(caddyfile)?.[1];
    expect(header).toBe(headerPolicy(["{$WEB_CSP_CONNECT_SRC}"]));
  });

  it("keeps the headers of the web's old server: no sniffing, the referrer on this origin only", () => {
    expect(caddyfile).toMatch(/^\t\tX-Content-Type-Options nosniff$/m);
    expect(caddyfile).toMatch(/^\t\tReferrer-Policy same-origin$/m);
    // on every answer, the 404 page's included
    expect(caddyfile.match(/^\t+import headers$/gm)).toHaveLength(2);
  });
});

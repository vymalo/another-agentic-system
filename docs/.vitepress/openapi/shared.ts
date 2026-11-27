// Shared by the node loader (openapi/spec.ts), the config and the client (openapi/client.ts): no node APIs here.

// The site's base, written once: config.ts reads DOCS_BASE with this default, and so does the loader.
export const DEFAULT_BASE = "/another-agentic-system/";

// The tag the library puts the spec's untagged operations under. The contract names no tags.
export const DEFAULT_TAG = "Chat";

const REPO_BLOB = "https://github.com/vymalo/another-agentic-system/blob/main/docs";

// A route id or tag becomes a path segment of a built page: nothing else may.
export function assertRouteId(id: unknown, what: string): string {
  if (typeof id !== "string" || !/^[A-Za-z0-9_-]+$/.test(id)) throw new Error(`${what} ${JSON.stringify(id)} is not a safe route segment ([A-Za-z0-9_-]+)`);
  return id;
}

// The spec's descriptions link to sibling files the way the files in docs/api/ do (`usage-v1.md`,
// `../decisions/0054-x.md#y?q "title"`). The library renders them as plain anchors, so each relative target is
// rewritten to a root-relative path inside the site, under `base`; the `?query`, `#hash` and `"title"` are kept.
// `http(s):`, `#anchor` and root-absolute targets are left alone. A file that is not a page of the site (examples/,
// chat-api.yaml, any non-.md) points at the repository. A target that climbs out of docs/ throws.
export function rewriteDocLinks(spec: string, base = DEFAULT_BASE): string {
  const prefix = base.endsWith("/") ? base : `${base}/`;
  return spec.replace(/\]\(([^)\s]+)((?:\s+"[^"]*")?)\)/g, (whole, target: string, title: string) => {
    if (/^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) return whole;
    const m = /^([^?#]*)(.*)$/.exec(target)!;
    const suffix = m[2] + title;
    const parts = ["api"];
    for (const seg of m[1].split("/")) {
      if (seg === "..") {
        if (!parts.length) throw new Error(`link ${target} climbs out of docs/`);
        parts.pop();
      } else if (seg && seg !== ".") parts.push(seg);
    }
    const last = parts[parts.length - 1] ?? "";
    const onSite = /\.md$/.test(last) && !(parts[0] === "api" && parts[1] === "examples");
    if (!onSite) return `](${REPO_BLOB}/${parts.join("/")}${suffix})`;
    const dir = parts.slice(0, -1).join("/");
    if (last === "README.md") return `](${prefix}${dir ? `${dir}/` : ""}${suffix})`;
    return `](${prefix}${[...parts.slice(0, -1), last.replace(/\.md$/, "")].join("/")}${suffix})`;
  });
}

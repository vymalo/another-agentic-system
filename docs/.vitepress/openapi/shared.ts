// Shared by the node loader (openapi/spec.ts) and the client theme (theme/index.ts): no node APIs here.

// The tag the library puts the spec's untagged operations under. The contract names no tags.
export const DEFAULT_TAG = "Chat";

const REPO_TREE = "https://github.com/vymalo/another-agentic-system/tree/main/docs";

// The spec's descriptions link to sibling files the way the files in docs/api/ do (`usage-v1.md`,
// `../decisions/0054-x.md#y`). The library renders them as plain anchors, so each target is rewritten
// to a root-relative path inside the site, under `base`. `http(s):` and `#anchor` targets are left alone.
// Folders that are not part of the site (examples/) point at the repository.
export function rewriteDocLinks(spec: string, base = "/"): string {
  const prefix = base.endsWith("/") ? base : `${base}/`;
  return spec.replace(/\]\(([^)\s]+)\)/g, (whole, target: string) => {
    if (/^([a-z][a-z0-9+.-]*:|#|\/)/i.test(target)) return whole;
    const [pathPart, ...hash] = target.split("#");
    const parts = ["api"];
    for (const seg of pathPart.split("/")) {
      if (seg === "..") parts.pop();
      else if (seg && seg !== ".") parts.push(seg);
    }
    const last = parts[parts.length - 1];
    const hashPart = hash.length ? `#${hash.join("#")}` : "";
    if (parts[1] === "examples" || parts.length === 0 || !/\.md$/.test(last)) {
      return /\.md$/.test(last) || parts[1] === "examples" ? `](${REPO_TREE}/${parts.join("/")}${hashPart})` : whole;
    }
    parts[parts.length - 1] = last.replace(/\.md$/, "");
    let route = parts.join("/");
    if (parts[parts.length - 1] === "README") route = parts.slice(0, -1).join("/") + "/";
    return `](${prefix}${route}${hashPart})`;
  });
}

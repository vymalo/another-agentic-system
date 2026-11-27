// The sidebar as vitepress-plugin-llms reads it for llms.txt (ANR-1088).
// The plugin drops `base` from the links of a group nested in a section (1.15.0, verified 2026-11-27 in its
// dist/index.js: resolveNestedSections passes the empty section base), so the groups are lifted to sections
// of leaf links, which keep it. The operations and tags of the chat API are dynamic routes with no file
// behind them; the spec itself (api/chat-api.yaml) stands for them in llms.txt. Lifting flattens Reference into
// sibling sections (API, Extensions, Configuration) on purpose: no page is lost, only the heading level.
type Item = { text?: string; link?: string; items?: Item[] };
const dynamic = /^\/api\/(reference$|operations\/|tags\/)/;

function lift(item: Item, out: Item[]) {
  const leaves = (item.items ?? []).filter((i) => i.link && !dynamic.test(i.link));
  if (leaves.length > 0) out.push({ text: item.text, items: leaves });
  for (const child of item.items ?? []) if (child.items) lift(child, out);
}

export function llmsSidebar(sidebar: Item[] | Record<string, Item[]> | undefined): Item[] {
  const out: Item[] = [];
  for (const section of Array.isArray(sidebar) ? sidebar : Object.values(sidebar ?? {}).flat()) lift(section, out);
  return out;
}

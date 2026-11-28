// The table of docs/api/extensions.md, read once for both of its users: the Reference > Extensions group of the sidebar
// (config.ts) and the status rule of tools/docs-check. No dependency, so the checker needs none of the site's packages.

// The index's words, each with its icon (the legend in api/README.md#status-icons).
export const STATUS_ICONS = { built: "✅", experimental: "🧪", partial: "⚠️", planned: "🚧", retired: "➖" };

// A status as a cell or a page's Status bullet starts: icon, word, then ` · …` or nothing.
export const STATUS_PATTERN = new RegExp(`^(?:${Object.entries(STATUS_ICONS).map(([w, i]) => `${i} ${w}`).join("|")})(?= ·|\\s*$)`, "u");

// One row per extension: its line, name, status (or null if the cell does not start with a legend icon and word), the
// cell itself, and the page its Contract column links first in this directory (`{ file, anchor }`, or null).
export function readExtensionIndex(source) {
  const lines = source.split("\n");
  const header = lines.findIndex((l) => /^\|\s*Extension\s*\|\s*URI suffix\s*\|\s*Status\s*\|/.test(l));
  if (header < 0) return [];
  const rows = [];
  for (let i = header + 2; i < lines.length && lines[i].startsWith("|"); i++) {
    const cells = lines[i].split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());
    const link = [...cells.at(-1).matchAll(/\]\(([\w.-]+\.md)(?:#([^)]*))?\)/g)][0];
    rows.push({
      line: i + 1,
      name: cells[0].replace(/\*\*/g, ""),
      cell: cells[2],
      status: cells[2].match(STATUS_PATTERN)?.[0] ?? null,
      page: link ? { file: link[1], anchor: link[2] } : null,
    });
  }
  return rows;
}

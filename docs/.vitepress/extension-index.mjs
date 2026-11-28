// The table of docs/api/extensions.md, read once for both of its users: the Reference > Extensions group of the sidebar
// (config.ts) and the status rule of tools/docs-check, which also reads the Status column of docs/api/README.md. No dependency, so the checker needs none of the site's packages.

// The index's words, each with its icon (the legend in api/README.md#status-icons).
export const STATUS_ICONS = { built: "✅", experimental: "🧪", partial: "⚠️", planned: "🚧", retired: "➖" };

// A status as a cell or a page's Status bullet starts: icon, word, then ` · …` or nothing.
export const STATUS_PATTERN = new RegExp(`^(?:${Object.entries(STATUS_ICONS).map(([w, i]) => `${i} ${w}`).join("|")})(?= ·|\\s*$)`, "u");

// The cells of a table row; an escaped pipe stays in its cell.
const cellsOf = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());

// The contract table of api/README.md (| Page | What it is | Status |): one row per page it links first, with its line and
// status (or null if the cell does not start with a legend icon and word).
export function readContractTable(source) {
  const lines = source.split("\n");
  const header = lines.findIndex((l) => /^\|\s*Page\s*\|\s*What it is\s*\|\s*Status\s*\|/.test(l));
  if (header < 0) return [];
  const rows = [];
  for (let i = header + 2; i < lines.length && lines[i].startsWith("|"); i++) {
    const cells = cellsOf(lines[i]);
    const file = cells[0].match(/\]\(([\w.-]+\.md)\)/)?.[1];
    if (file) rows.push({ line: i + 1, file, status: cells.at(-1).match(STATUS_PATTERN)?.[0] ?? null });
  }
  return rows;
}

// One row per extension: its line, name, status (or null if the cell does not start with a legend icon and word), the
// cell itself, and the page its Contract column links first in this directory (`{ file, anchor }`, or null).
export function readExtensionIndex(source) {
  const lines = source.split("\n");
  const header = lines.findIndex((l) => /^\|\s*Extension\s*\|\s*URI suffix\s*\|\s*Status\s*\|/.test(l));
  if (header < 0) return [];
  const rows = [];
  for (let i = header + 2; i < lines.length && lines[i].startsWith("|"); i++) {
    const cells = cellsOf(lines[i]);
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

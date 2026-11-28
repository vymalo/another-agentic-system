// The table of docs/api/extensions.md, read once for both of its users: the Reference > Extensions group of the sidebar
// (config.ts) and the status rule of tools/docs-check, which also reads the Status column of docs/api/README.md. No dependency, so the checker needs none of the site's packages.

// The index's words, each with its icon (the legend in api/README.md#status-icons).
export const STATUS_ICONS = { built: "✅", experimental: "🧪", partial: "⚠️", planned: "🚧", retired: "➖" };

// A status as a cell or a page's Status bullet starts: icon, word, then ` · …` or nothing.
export const STATUS_PATTERN = new RegExp(`^(?:${Object.entries(STATUS_ICONS).map(([w, i]) => `${i} ${w}`).join("|")})(?= ·|\\s*$)`, "u");

// The lines of a Markdown source as a reader sees them: the lines of a fenced code block (the fence's own included) are
// emptied, and the characters of an HTML comment become spaces, so what is left keeps its line number and column.
export function visibleLines(source) {
  const out = [];
  const lines = source.split("\n");
  let fence = null;
  let comment = false;
  for (let n = 0; n < lines.length; n++) {
    const line = lines[n].replace(/\r$/, "");
    const f = /^\s{0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      if (f && f[1][0] === fence[0] && f[1].length >= fence.length && f[2].trim() === "") fence = null;
      out.push("");
      continue;
    }
    if (!comment && f && !(f[1][0] === "`" && f[2].includes("`"))) {
      fence = f[1];
      out.push("");
      continue;
    }
    let seen = "";
    let i = 0;
    while (i < line.length) {
      if (comment) {
        const end = line.indexOf("-->", i);
        const to = end < 0 ? line.length : end + 3;
        seen += " ".repeat(to - i);
        comment = end < 0;
        i = to;
        continue;
      }
      // A code span is text, a `<!--` in it opens nothing.
      const code = /^(`+)(?:[^`]|(?!\1(?!`))`)*?\1(?!`)/.exec(line.slice(i));
      if (code) {
        seen += code[0];
        i += code[0].length;
      } else if (line.startsWith("<!--", i)) {
        // `<!-->` and `<!--->` are whole comments. One that never closes hides the rest only if it starts its line (an HTML
        // block); in the middle of a line it is text.
        const end = line.indexOf("-->", i + 2);
        if (end >= 0) {
          seen += " ".repeat(end + 3 - i);
          i = end + 3;
        } else if (line.slice(0, i).trim() === "" || lines.slice(n + 1).some((l) => l.includes("-->"))) {
          comment = true;
          seen += " ".repeat(line.length - i);
          i = line.length;
        } else {
          seen += line.slice(i);
          i = line.length;
        }
      } else if (line[i] === "`") {
        const ticks = /^`+/.exec(line.slice(i))[0];
        seen += ticks;
        i += ticks.length;
      } else seen += line[i++];
    }
    out.push(seen);
  }
  return out;
}

// The cells of a table row; an escaped pipe stays in its cell.
const cellsOf = (line) => line.split(/(?<!\\)\|/).slice(1, -1).map((c) => c.trim());

// The visible rows of the first visible table whose header matches: each with its line and cells, and `malformed` (why)
// when it does not have the header's number of cells, closing pipe included.
function tableRows(source, headerPattern) {
  const lines = visibleLines(source);
  const header = lines.findIndex((l) => headerPattern.test(l));
  if (header < 0) return [];
  const columns = cellsOf(lines[header].trimEnd()).length;
  const rows = [];
  for (let i = header + 2; i < lines.length && lines[i].startsWith("|"); i++) {
    const cells = cellsOf(lines[i].trimEnd());
    const malformed = cells.length === columns ? null : `malformed row: ${cells.length} cells, the header has ${columns}`;
    rows.push({ line: i + 1, cells, malformed });
  }
  return rows;
}

// The contract table of api/README.md (| Page | What it is | Status |): one row per page it links first, with its line and
// status (or null if the cell does not start with a legend icon and word). A malformed row is `{ line, malformed }`.
export function readContractTable(source) {
  return tableRows(source, /^\|\s*Page\s*\|\s*What it is\s*\|\s*Status\s*\|/).flatMap(({ line, cells, malformed }) => {
    if (malformed) return [{ line, malformed }];
    const file = cells[0].match(/\]\(([\w.-]+\.md)\)/)?.[1];
    return file ? [{ line, file, status: cells[2].match(STATUS_PATTERN)?.[0] ?? null }] : [];
  });
}

// One row per extension: its line, name, status (or null if the cell does not start with a legend icon and word), the
// cell itself, and the page its Contract column links first in this directory (`{ file, anchor }`, or null). A malformed
// row is `{ line, malformed, page: null }`.
export function readExtensionIndex(source) {
  return tableRows(source, /^\|\s*Extension\s*\|\s*URI suffix\s*\|\s*Status\s*\|/).map(({ line, cells, malformed }) => {
    if (malformed) return { line, malformed, page: null };
    const link = cells.at(-1).match(/\]\(([\w.-]+\.md)(?:#([^)]*))?\)/);
    return {
      line,
      name: cells[0].replace(/\*\*/g, ""),
      cell: cells[2],
      status: cells[2].match(STATUS_PATTERN)?.[0] ?? null,
      page: link ? { file: link[1], anchor: link[2] } : null,
    };
  });
}

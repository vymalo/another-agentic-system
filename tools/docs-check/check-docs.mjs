// Validates the repository's Markdown:
//   1. every ```mermaid block parses with the pinned Mermaid version, and
//   2. every relative Markdown link points at a file or directory that exists,
//   3. every relative src="..." and srcset="..." of an HTML tag in the Markdown (an <img>, or the
//      <source> of a <picture>) does too, and
//   4. every `::: in-depth` block of the site (docs/.vitepress/theme/in-depth.ts) is closed, and
//   5. every `#fragment` of such a link, from a README.md or a file under docs/, to a Markdown file
//      names a heading (slugged as GitHub and the docs site do, github-slugger, -1, -2 for repeats) or an `id="..."` of an HTML tag of that file, and
//   6. every Rust crate (a directory with a Cargo.toml under orchestrator/crates/ or
//      orchestrator/bin/) has a README.md next to it, and
//   7. every extension contract page of docs/api says the status that its row of extensions.md says, both ways, and so
//      does the page's row in the contract table of docs/api/README.md.
// Usage (from the repo root):  npm --prefix tools/docs-check ci && node tools/docs-check/check-docs.mjs
// Exits 1 on any failure, listing file:line for each.
import fs from 'node:fs';
import path from 'node:path';
import { JSDOM } from 'jsdom';
import { readContractTable, readExtensionIndex, STATUS_PATTERN, visibleLines } from '../../docs/.vitepress/extension-index.mjs';

const root = process.cwd();
// dist: the site's build output, which holds a generated .md per page (vitepress-plugin-llms), not sources.
const skipDirs = new Set(['.git', 'node_modules']);
const skipPaths = new Set([path.join(process.cwd(), 'docs', '.vitepress', 'dist')]);

function* markdownFiles(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (skipDirs.has(entry.name) || skipPaths.has(full)) continue;
    if (entry.isSymbolicLink()) continue; // symlinked skills/AGENTS.md are checked at their target
    if (entry.isDirectory()) yield* markdownFiles(full);
    else if (entry.name.endsWith('.md')) yield full;
  }
}

// Mermaid needs a DOM to parse.
const dom = new JSDOM('<!doctype html><html><body></body></html>');
globalThis.window = dom.window;
globalThis.document = dom.window.document;
Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator, configurable: true });
globalThis.DOMParser = dom.window.DOMParser;
globalThis.Element = dom.window.Element;
const { default: mermaid } = await import('mermaid');
mermaid.initialize({ startOnLoad: false });

const lineOf = (src, index) => src.slice(0, index).split('\n').length;
let diagrams = 0, links = 0, images = 0, anchors = 0;

// `::: name` opens a container and a bare `:::` closes the innermost one; a fenced block is code (the same
// character, at least as long, nothing after it closes it). Returns the line of each `::: in-depth` left open.
function unclosedInDepth(src) {
  const open = [];
  let fence = null;
  const lines = src.split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trimStart();
    const f = /^(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      if (f && f[1][0] === fence.ch && f[1].length >= fence.len && f[2].trim() === '') fence = null;
      continue;
    }
    if (f && !(f[1][0] === '`' && f[2].includes('`'))) { fence = { ch: f[1][0], len: f[1].length }; continue; }
    const m = /^:{3,}\s*(.*?)\s*$/.exec(line);
    if (!m) continue;
    if (m[1] === '') open.pop();
    else open.push({ name: m[1].split(/\s/)[0], line: i + 1 });
  }
  return open.filter((o) => o.name === 'in-depth').map((o) => o.line);
}
const failures = [];

// GitHub's slug of a heading, which is also the docs site's (docs/.vitepress/config.ts sets markdown.anchor.slugify to
// github-slugger), so one link works on both: lowercase, punctuation dropped (not turned into a hyphen: "dispatcher's"
// is "dispatchers", "ask_agent" keeps its underscore), spaces to hyphens, -1, -2 on a repeat.
const githubSlug = (str) => str.toLowerCase().replace(/[^\p{L}\p{M}\p{N}_\- ]/gu, '').replace(/ /g, '-');

// The text markdown-it-anchor slugs: the text and code of the heading, no markup, no image, no HTML tag.
const headingText = (raw) => raw
  .replace(/\s+#+\s*$/, '')
  .replace(/!\[[^\]]*\]\([^)]*\)/g, '')
  .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
  .replace(/\[([^\]]*)\]\[[^\]]*\]/g, '$1')
  .replace(/<[^>]*>/g, '')
  .replace(/`([^`]*)`/g, '$1')
  .replace(/(\*\*|__|\*|~~)/g, '')
  .replace(/(^|\W)_|_(?=\W|$)/g, '$1')
  .trim();

// The anchors of a Markdown file: its headings (ATX or Setext, outside fenced code) and the id="" of its HTML tags.
const anchorCache = new Map();
function anchorsOf(file) {
  const key = file;
  if (anchorCache.has(key)) return anchorCache.get(key);
  const slug = githubSlug;
  const ids = new Set();
  const used = new Map();
  const add = (title) => {
    const base = slug(title);
    let id = base;
    for (let n = used.get(base) ?? 1; used.has(id) || ids.has(id); n++) { id = `${base}-${n}`; used.set(base, n + 1); }
    used.set(id, 1);
    ids.add(id);
  };
  let fence = null;
  const lines = fs.readFileSync(file, 'utf8').split('\n');
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const f = /^\s{0,3}(`{3,}|~{3,})(.*)$/.exec(line);
    if (fence) {
      if (f && f[1][0] === fence.ch && f[1].length >= fence.len && f[2].trim() === '') fence = null;
      continue;
    }
    if (f && !(f[1][0] === '`' && f[2].includes('`'))) { fence = { ch: f[1][0], len: f[1].length }; continue; }
    const h = /^ {0,3}#{1,6}(?:[ \t]+(.*?))?[ \t]*$/.exec(line);
    if (h) { add(headingText(h[1] ?? '')); continue; }
    if (/^ {0,3}(=+|-+)[ \t]*$/.test(line) && i > 0 && /^ {0,3}\S/.test(lines[i - 1]) && !/^\s*([-*+]|\d+[.)]|>|#|\||<)/.test(lines[i - 1])) add(headingText(lines[i - 1].trim()));
    for (const m of line.replace(/`[^`]*`/g, '').matchAll(/<[a-z][^>]*?\sid\s*=\s*(?:"([^"]*)"|'([^']*)')/gi)) ids.add(m[1] ?? m[2]);
  }
  anchorCache.set(key, ids);
  return ids;
}
const checksAnchors = (rel) => path.basename(rel) === 'README.md' || rel.startsWith('docs' + path.sep);

for (const file of markdownFiles(root)) {
  const rel = path.relative(root, file);
  const src = fs.readFileSync(file, 'utf8');

  for (const m of src.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
    diagrams++;
    try { await mermaid.parse(m[1]); }
    catch (e) { failures.push(`${rel}:${lineOf(src, m.index)} mermaid: ${String(e.message).split('\n')[0]}`); }
  }

  for (const line of unclosedInDepth(src)) failures.push(`${rel}:${line} unclosed ::: in-depth`);

  // Blank out fenced blocks and inline code spans (keeping line numbers) so
  // example links inside code are not checked.
  const blank = (code) => code.replace(/[^\n]/g, ' ');
  const prose = src
    .replace(/```[\s\S]*?```/g, blank)
    .replace(/`[^`\n]+`/g, blank);
  for (const m of prose.matchAll(/\]\(([^)\s]+)\)/g)) {
    const [target, ...frag] = m[1].split('#');
    const fragment = frag.join('#');
    if (/^[a-z][a-z0-9+.-]*:/i.test(target)) continue; // http(s):, mailto:
    let resolved = file; // a bare #fragment is this file's own
    if (target) {
      links++;
      resolved = path.resolve(path.dirname(file), decodeURI(target));
      if (!fs.existsSync(resolved)) {
        failures.push(`${rel}:${lineOf(src, m.index)} broken link: ${m[1]}`);
        continue;
      }
    }
    if (fragment && checksAnchors(rel) && resolved.endsWith('.md') && fs.statSync(resolved).isFile()) {
      anchors++;
      const want = decodeURIComponent(fragment);
      if (!anchorsOf(resolved).has(want)) {
        failures.push(`${rel}:${lineOf(src, m.index)} dead anchor: #${fragment} is no heading or id of ${path.relative(root, resolved)}`);
      }
    }
  }

  // HTML in Markdown: the files an <img> or a <picture>'s <source> names (GitHub shows both
  // themes of a screenshot this way, where ![alt](path) cannot). A srcset is a comma-separated
  // list of "path [descriptor]"; external URLs and data: URIs are not files of this repository.
  for (const tag of prose.matchAll(/<[a-z][^>]*>/gi)) {
    for (const attr of tag[0].matchAll(/\s(src|srcset)\s*=\s*(?:"([^"]*)"|'([^']*)')/gi)) {
      const value = attr[2] ?? attr[3];
      const candidates = attr[1].toLowerCase() === 'srcset' ? value.split(',') : [value];
      for (const candidate of candidates) {
        const target = candidate.trim().split(/\s+/)[0].split('#')[0];
        if (!target || /^([a-z][a-z0-9+.-]*:|\/\/)/i.test(target)) continue; // http(s):, data:, //host
        images++;
        if (!fs.existsSync(path.resolve(path.dirname(file), decodeURI(target)))) {
          failures.push(`${rel}:${lineOf(src, tag.index + attr.index)} broken ${attr[1].toLowerCase()}: ${target}`);
        }
      }
    }
  }
}

// The extension contract pages say the status the index says (ANR-925). Each row of docs/api/extensions.md links its
// contract page (A2UI: a section of agui.md); that page has exactly one `- **Status:**` line before its first section (or
// in the linked section) that starts with the row's icon and word. A page that declares an extension URI must have a row.
// Only what a reader sees counts (visibleLines: no fenced block, no HTML comment). The one line is a `- **Status:**`
// bullet at column 0; a status bullet written any other way (indented, `*`, `+`, numbered, quoted, `__`) is counted too,
// so a second one cannot hide.
const statusBullet = /^[\s>]*(?:[-*+]|\d+[.)])\s+(\*\*|__)\s*status\s*(?::\s*\1|\1\s*:)/i;
// These rows have no contract page in docs/api: a new row without a page is a failure until it is named here.
const rowsWithoutPage = new Set(['Identity', 'Prompt statement', 'Build', 'Credential request']);
let statuses = 0;
const indexRel = path.join('docs', 'api', 'extensions.md');
const apiDir = path.join(root, 'docs', 'api');
if (fs.existsSync(path.join(root, indexRel))) {
  const rows = readExtensionIndex(fs.readFileSync(path.join(root, indexRel), 'utf8'));
  if (!rows.length) failures.push(`${indexRel}:1 no extension table (| Extension | URI suffix | Status | …) found`);
  const listed = new Set();
  for (const row of rows) {
    const at = `${indexRel}:${row.line}`;
    if (row.malformed) {
      failures.push(`${at} ${row.malformed}`);
      continue;
    }
    if (!row.status) failures.push(`${at} ${row.name}: status does not start with an icon and word of the legend`);
    if (!row.page) {
      if (!rowsWithoutPage.has(row.name)) failures.push(`${at} ${row.name}: the Contract column links no page of docs/api`);
      continue;
    }
    if (rowsWithoutPage.has(row.name)) failures.push(`${at} ${row.name}: now links a page; take it off rowsWithoutPage in tools/docs-check`);
    const { file, anchor } = row.page;
    listed.add(file);
    const rel = path.join('docs', 'api', file);
    if (!fs.existsSync(path.join(root, rel))) {
      failures.push(`${at} ${row.name}: its contract page ${rel} does not exist`);
      continue;
    }
    const lines = visibleLines(fs.readFileSync(path.join(root, rel), 'utf8'));
    // The part that holds the status line: before the first section, or the section the row links.
    let from = 0;
    if (anchor) {
      from = lines.findIndex((l) => /^##+ /.test(l) && githubSlug(headingText(l.replace(/^#+ /, ''))) === anchor);
      if (from < 0) {
        failures.push(`${at} ${row.name}: ${rel} has no section #${anchor}`);
        continue;
      }
    }
    let to = lines.findIndex((l, i) => i > from && (anchor ? /^##+ / : /^## /).test(l));
    if (to < 0) to = lines.length;
    const found = [];
    for (let i = from; i < to; i++) if (statusBullet.test(lines[i])) found.push(i);
    if (found.length !== 1) {
      failures.push(`${rel}:${(found[1] ?? from) + 1} ${found.length ? `more than one status line (also line ${found[0] + 1})` : 'no status line (- **Status:** …)'}; ${at} says ${row.status}`);
      continue;
    }
    if (!lines[found[0]].startsWith('- **Status:** ')) {
      failures.push(`${rel}:${found[0] + 1} the status line is not a \`- **Status:** \` bullet at column 0; ${at} says ${row.status}`);
      continue;
    }
    statuses++;
    const says = lines[found[0]].slice('- **Status:** '.length).match(STATUS_PATTERN)?.[0];
    if (!says) failures.push(`${rel}:${found[0] + 1} status line does not start with an icon and word of the legend; ${at} says ${row.status}`);
    else if (row.status && says !== row.status) failures.push(`${rel}:${found[0] + 1} status is ${says}, but ${at} says ${row.status}`);
  }
  // The third copy: the contract table of api/README.md. A row of the index that links a section (A2UI) is not compared:
  // the README's row for that file is the whole page's, which is not the extension.
  const readmeRel = path.join('docs', 'api', 'README.md');
  const readme = fs.existsSync(path.join(root, readmeRel)) ? readContractTable(fs.readFileSync(path.join(root, readmeRel), 'utf8')) : [];
  if (!readme.length) failures.push(`${readmeRel}:1 no contract table (| Page | What it is | Status |) found`);
  for (const bad of readme.filter((c) => c.malformed)) failures.push(`${readmeRel}:${bad.line} ${bad.malformed}`);
  for (const row of readme.length ? rows.filter((r) => r.page && !r.page.anchor) : []) {
    const copies = readme.filter((c) => c.file === row.page.file);
    const at = `${indexRel}:${row.line}`;
    if (!copies.length) failures.push(`${readmeRel}:1 no row for ${row.page.file}; ${at} says ${row.status}`);
    for (const copy of copies) {
      if (!copy.status) failures.push(`${readmeRel}:${copy.line} status does not start with an icon and word of the legend; ${at} says ${row.status}`);
      else if (row.status && copy.status !== row.status) failures.push(`${readmeRel}:${copy.line} status is ${copy.status}, but ${at} says ${row.status}`);
    }
  }
  for (const name of fs.readdirSync(apiDir).filter((f) => f.endsWith('.md') && f !== 'README.md' && f !== 'extensions.md')) {
    const head = visibleLines(fs.readFileSync(path.join(apiDir, name), 'utf8')).join('\n').split(/^## /m)[0];
    const declares = /^# A2A extension:|^- \*\*(?:URI|A2A extension):\*\* `https:\/\/[^`]*\/a2a\/extensions\//m.test(head);
    if (declares && !listed.has(name)) failures.push(`docs/api/${name}:1 an extension contract page that ${indexRel} does not list`);
  }
}

// Every crate documents itself: a directory under these roots with a Cargo.toml needs a
// README.md, updated in the same change as any change to its public API, environment
// variables or tests (CLAUDE.md, "Code").
const crateRoots = ['orchestrator/crates', 'orchestrator/bin'];
let crates = 0;
for (const crateRoot of crateRoots) {
  const dir = path.join(root, crateRoot);
  if (!fs.existsSync(dir)) continue;
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const crateDir = path.join(dir, entry.name);
    if (!fs.existsSync(path.join(crateDir, 'Cargo.toml'))) continue;
    crates++;
    if (!fs.existsSync(path.join(crateDir, 'README.md'))) {
      failures.push(`${path.relative(root, crateDir)}: crate has a Cargo.toml but no README.md`);
    }
  }
}

console.log(`${diagrams} diagrams, ${links} relative links, ${anchors} anchors, ${images} image paths, ${crates} crate READMEs, ${statuses} extension statuses checked`);
if (failures.length) {
  console.error(failures.join('\n'));
  console.error(`${failures.length} problem(s)`);
  process.exit(1);
}
console.log('docs OK');

// Validates the repository's Markdown:
//   1. every ```mermaid block parses with the pinned Mermaid version, and
//   2. every relative Markdown link points at a file or directory that exists,
//   3. every relative src="..." and srcset="..." of an HTML tag in the Markdown (an <img>, or the
//      <source> of a <picture>) does too, and
//   4. every Rust crate (a directory with a Cargo.toml under orchestrator/crates/ or
//      orchestrator/bin/) has a README.md next to it.
// Usage (from the repo root):  npm --prefix tools/docs-check ci && node tools/docs-check/check-docs.mjs
// Exits 1 on any failure, listing file:line for each.
import fs from 'node:fs';
import path from 'node:path';
import { JSDOM } from 'jsdom';

const root = process.cwd();
const skipDirs = new Set(['.git', 'node_modules']);

function* markdownFiles(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (skipDirs.has(entry.name)) continue;
    const full = path.join(dir, entry.name);
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
let diagrams = 0, links = 0, images = 0;
const failures = [];

for (const file of markdownFiles(root)) {
  const rel = path.relative(root, file);
  const src = fs.readFileSync(file, 'utf8');

  for (const m of src.matchAll(/```mermaid\n([\s\S]*?)```/g)) {
    diagrams++;
    try { await mermaid.parse(m[1]); }
    catch (e) { failures.push(`${rel}:${lineOf(src, m.index)} mermaid: ${String(e.message).split('\n')[0]}`); }
  }

  // Blank out fenced blocks and inline code spans (keeping line numbers) so
  // example links inside code are not checked.
  const blank = (code) => code.replace(/[^\n]/g, ' ');
  const prose = src
    .replace(/```[\s\S]*?```/g, blank)
    .replace(/`[^`\n]+`/g, blank);
  for (const m of prose.matchAll(/\]\(([^)\s]+)\)/g)) {
    const target = m[1].split('#')[0];
    if (!target || /^[a-z][a-z0-9+.-]*:/i.test(target)) continue; // anchors, http(s):, mailto:
    links++;
    if (!fs.existsSync(path.resolve(path.dirname(file), decodeURI(target)))) {
      failures.push(`${rel}:${lineOf(src, m.index)} broken link: ${m[1]}`);
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

console.log(`${diagrams} diagrams, ${links} relative links, ${images} image paths, ${crates} crate READMEs checked`);
if (failures.length) {
  console.error(failures.join('\n'));
  console.error(`${failures.length} problem(s)`);
  process.exit(1);
}
console.log('docs OK');

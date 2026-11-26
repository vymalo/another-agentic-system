import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { defineMd3Config } from "@vaam-apps/vitepress-md3/config";
import { withMermaid } from "vitepress-plugin-mermaid";

const here = path.dirname(fileURLToPath(import.meta.url));
const docs = path.resolve(here, "..");

// Every page of a folder, as sidebar items, titled by its first heading.
function pages(dir: string, prefix: string) {
  return fs
    .readdirSync(path.join(docs, dir))
    .filter((f) => f.endsWith(".md"))
    .sort()
    .map((f) => {
      const first = fs
        .readFileSync(path.join(docs, dir, f), "utf8")
        .split("\n")
        .find((l) => l.startsWith("# "));
      return {
        text: first ? first.replace(/^#\s+/, "") : f.replace(/\.md$/, ""),
        link: `${prefix}${f.replace(/\.md$/, "")}`,
      };
    });
}

// The same seven sections as every Another repository: .vitepress/STRUCTURE.md.
const sidebar = [
  { text: "1. Overview", items: [{ text: "Vision", link: "/vision" }, { text: "Tester note", link: "/tester-note" }] },
  { text: "2. Get started", items: [{ text: "MVP and build order", link: "/mvp" }] },
  {
    text: "3. Guides",
    collapsed: true,
    items: [{ text: "Lessons from Agent Canvas", link: "/lessons-from-agent-canvas" }, { text: "App Factory MVP plan", link: "/plans/app-factory-mvp-2026-12" }],
  },
  {
    text: "4. Reference",
    collapsed: true,
    items: [
      { text: "API index", link: "/api/README" },
      ...pages("api", "/api/").filter((p) => p.link !== "/api/README"),
    ],
  },
  {
    text: "5. Architecture",
    items: [{ text: "Architecture", link: "/architecture" }, { text: "Orchestrator", link: "/orchestrator" }],
  },
  {
    text: "6. Decisions",
    collapsed: true,
    items: [{ text: "Open questions", link: "/open-questions" }, ...pages("decisions", "/decisions/")],
  },
  { text: "7. Contributing", items: [{ text: "Vendor check 2026-11-24", link: "/vendor-check-2026-11-24" }] },
];

// The pages are plain Markdown for GitHub, where `<tier>` in prose is text. VitePress compiles
// them as Vue, where it is an unclosed element. Inline HTML that is not a real HTML tag becomes text.
const HTML_TAGS = new Set(
  "a abbr br code del details div em h1 h2 h3 h4 h5 h6 hr i img kbd li ol p picture pre source span strong sub summary sup table tbody td tfoot th thead tr u ul video".split(" "),
);
// `{{ x }}` in prose or in a code span is interpolation in Vue; here it is text.
function literalAngleBrackets(md: any) {
  for (const rule of ["text", "code_inline"]) {
    const base = md.renderer.rules[rule];
    md.renderer.rules[rule] = (tokens: any, i: number, o: any, env: any, self: any) => {
      const html = base
        ? base(tokens, i, o, env, self)
        : md.utils.escapeHtml(tokens[i].content);
      return html.replace(/\{\{/g, "&#123;&#123;").replace(/\}\}/g, "&#125;&#125;");
    };
  }
  // A link out of docs/ (the code, a crate's README) or into the excluded fixtures is a
  // link into the repository: it goes to GitHub instead of being a dead link.
  md.core.ruler.push("repository_links", (state: any) => {
    const page: string | undefined = state.env?.relativePath;
    if (!page) return;
    const walk = (tokens: any[]) => {
      for (const t of tokens) {
        if (t.type === "link_open") {
          const href: string = t.attrGet("href") ?? "";
          if (/^([a-z]+:|#|\/)/i.test(href)) continue;
          const [file, hash] = href.split("#");
          const target = path.posix.normalize(path.posix.join("docs", path.posix.dirname(page), file));
          if (target.startsWith("docs/") && !target.startsWith("docs/api/examples")) continue;
          t.attrSet("href", `https://github.com/vymalo/another-agentic-system/blob/main/${target.replace(/\/$/, "")}${hash ? "#" + hash : ""}`);
        }
        if (t.children) walk(t.children);
      }
    };
    walk(state.tokens);
  });
  md.core.ruler.push("literal_angle_brackets", (state: any) => {
    for (const block of state.tokens) {
      if (block.type === "html_block") {
        const m = /^\s*<\/?([A-Za-z][\w-]*)/.exec(block.content);
        if (!m || !HTML_TAGS.has(m[1].toLowerCase())) {
          block.type = "fence";
          block.info = "text";
          block.tag = "code";
          block.markup = "```";
        }
        continue;
      }
      if (block.type !== "inline" || !block.children) continue;
      for (const t of block.children) {
        if (t.type !== "html_inline") continue;
        const m = /^<\/?([A-Za-z][\w-]*)/.exec(t.content);
        if (!m || !HTML_TAGS.has(m[1].toLowerCase())) t.type = "text";
      }
    }
  });
}

export default withMermaid(
  defineMd3Config({
    title: "another-agentic-system",
    description: "A protocol-agnostic orchestration layer for multi-agent work.",
    base: process.env.DOCS_BASE ?? "/another-agentic-system/",
    cleanUrls: true,
    markdown: { config: literalAngleBrackets },
    srcExclude: ["**/node_modules/**", "api/examples/**", "**/.vitepress/STRUCTURE.md"],
    themeConfig: {
      md3: { style: "expressive", seed: "#0f766e" },
      search: { provider: "local" },
      sidebar,
      nav: [
        { text: "Overview", link: "/vision" },
        { text: "Architecture", link: "/architecture" },
        { text: "Reference", link: "/api/README" },
        { text: "Decisions", link: "/decisions/0001-rust-state-machine-on-postgres" },
      ],
      socialLinks: [{ icon: "github", link: "https://github.com/vymalo/another-agentic-system" }],
    },
  }),
);

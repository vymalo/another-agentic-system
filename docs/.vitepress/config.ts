import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineMd3Config } from "@vaam-apps/vitepress-md3/config";
import { withMermaid } from "vitepress-plugin-mermaid";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

// The sidebar of a folder: every page in it, titled by its first heading.
function pages(dir: string, skip: string[] = []) {
  return readdirSync(join(root, dir))
    .filter((f) => f.endsWith(".md") && f !== "README.md" && !skip.includes(f))
    .sort()
    .map((f) => {
      const text = readFileSync(join(root, dir, f), "utf8").match(/^# (.+)$/m)?.[1];
      return { text: (text ?? f.replace(/\.md$/, "")).replace(/`/g, ""), link: `/${dir}/${f.replace(/\.md$/, "")}` };
    });
}

// The shared section structure: see STRUCTURE.md. The platform's site uses the same one.
const sidebar = [
  {
    text: "Overview",
    items: [
      { text: "Vision", link: "/vision" },
      { text: "MVP and build order", link: "/mvp" },
      { text: "Lessons as requirements", link: "/lessons-from-agent-canvas" },
      { text: "Open questions", link: "/open-questions" },
    ],
  },
  { text: "Get started", items: [{ text: "Run it locally", link: "/getting-started" }, { text: "Tester note", link: "/tester-note" }] },
  { text: "Guides", collapsed: true, items: [...pages("plans"), { text: "Vendor check", link: "/vendor-check-2026-11-24" }] },
  {
    text: "Reference",
    collapsed: true,
    items: [{ text: "Chat API (OpenAPI)", link: "/api/chat-api" }, { text: "API index", link: "/api/" }, ...pages("api", ["chat-api.md"])],
  },
  { text: "Architecture", items: [{ text: "Architecture", link: "/architecture" }, { text: "Orchestrator", link: "/orchestrator" }] },
  { text: "Decisions", collapsed: true, items: [{ text: "ADR index", link: "/decisions/" }, ...pages("decisions")] },
  { text: "Contributing", items: [{ text: "How to contribute", link: "/contributing" }] },
];

// Prose writes <agent> or <ThreadShare> meaning a placeholder; Vue would read it as a component and fail the build.
// Only real HTML elements pass through, everything else is shown as the text it is.
const HTML = new Set(
  "a abbr blockquote br code details dd div dl dt em figcaption figure h1 h2 h3 h4 h5 h6 hr i img kbd li ol p picture pre small source span strong sub summary sup table tbody td tfoot th thead tr u ul video".split(" "),
);
export function escapeUnknownTags(md: any) {
  // `{{` is a Vue expression to the compiler; the docs mean the characters.
  const render = md.render.bind(md);
  md.render = (src: string, env: unknown) => render(src, env).replace(/\{\{/g, "&#123;&#123;");
  for (const rule of ["html_inline", "html_block"]) {
    const original = md.renderer.rules[rule];
    md.renderer.rules[rule] = (tokens: any, i: number, ...rest: any[]) => {
      const content: string = tokens[i].content;
      const tags = [...content.matchAll(/<\/?([A-Za-z][\w-]*)/g)].map((m) => m[1].toLowerCase());
      const wellFormed = /^\s*(<\/?[A-Za-z][\w-]*(\s+[\w:-]+(="[^"]*")?)*\s*\/?>\s*)+$/.test(content) || tokens[i].type === "html_block";
      if (wellFormed && tags.every((t) => HTML.has(t))) return original(tokens, i, ...rest);
      return md.utils.escapeHtml(content).replace(/\{\{/g, "&#123;&#123;");
    };
  }
}

export default withMermaid(
  defineMd3Config({
    title: "another-agentic-system",
    description: "A protocol-agnostic orchestration layer for multi-agent work.",
    base: process.env.DOCS_BASE ?? "/another-agentic-system/",
    cleanUrls: true,
    rewrites: { "api/README.md": "api/index.md" },
    // Pages link to source files outside docs/ (checked by tools/docs-check); every other dead link fails the build.
    ignoreDeadLinks: [/^\.\/(\.\.\/)+/, /(^|\/)examples\//],
    srcExclude: ["**/node_modules/**", "**/examples/**"],
    themeConfig: {
      md3: { style: "expressive", seed: "#0f766e" },
      nav: [
        { text: "Vision", link: "/vision" },
        { text: "Architecture", link: "/architecture" },
        { text: "API", link: "/api/" },
        { text: "Decisions", link: "/decisions/" },
      ],
      sidebar,
      search: { provider: "local" },
      socialLinks: [{ icon: "github", link: "https://github.com/vymalo/another-agentic-system" }],
    },
    markdown: { config: escapeUnknownTags, attrs: { disable: true } },
    mermaid: {},
  }),
);

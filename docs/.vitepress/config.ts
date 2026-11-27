import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineMd3Config } from "@vaam-apps/vitepress-md3/config";
import { useSidebar } from "vitepress-openapi";
import llmstxt from "vitepress-plugin-llms";
import { DEFAULT_BASE, DEFAULT_TAG } from "./openapi/shared";
import { loadSpec } from "./openapi/spec";
import { llmsSidebar } from "./llms";
import { inDepth } from "./theme/in-depth";

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

// A page of docs/ by its path without the extension, titled by its first heading.
function page(path: string, text?: string) {
  const title = readFileSync(join(root, `${path}.md`), "utf8").match(/^# (.+)$/m)?.[1];
  return { text: text ?? (title ?? path).replace(/`/g, ""), link: `/${path}` };
}

// A group of pages, shown only when it holds one: an empty group is not drawn.
type SidebarItem = { text: string; link?: string; collapsed?: boolean; items?: SidebarItem[] };
function group(text: string, items: SidebarItem[], collapsed = true) {
  return { text, collapsed, items };
}

// Reference > API: the one place to add a page. Every file of docs/api/ is in exactly one of the three
// Reference arrays below (the build throws on one that is in none), so a new page is one name in one array.
// The chat API's own pages (reference.md, and the tags/ and operations/ routes below it) are rendered from
// api/chat-api.yaml by vitepress-openapi.
const referencePage = { text: "Chat API reference", link: "/api/reference" };
const apiPages: { text: string; link: string }[] = [
  { text: "API index", link: "/api/" },
  ...["agui", "attachments", "history", "memory-v1", "message-text-v1", "missions-v1", "notifications-v1", "projects-v1", "prompts-v1", "teams-v1", "thread-markdown-v1", "thread-search-v1", "webhooks", "windows-v1"].map((f) => page(`api/${f}`)),
];
// The operations of the chat API, one page each under /api/operations/: dynamic routes, so not files the guard reads.
const chatApiOperations: SidebarItem = {
  text: "Chat API operations",
  collapsed: true,
  items: useSidebar({ spec: loadSpec(), defaultTag: DEFAULT_TAG, linkPrefix: "/api/operations/", tagLinkPrefix: "/api/tags/" }).generateSidebarGroups({ tags: [DEFAULT_TAG] }),
};
// Reference > Extensions: the A2A extensions and the other contracts a tool server or an agent implements.
const extensionPages = [
  "extensions", "access-tier-v1", "child-sessions-v1", "commands-v1", "locale-v1", "mentions-v1", "model-pick-v1", "model-relay-v1",
  "network-tier-v1", "signed-tool-requests-v1", "steer-v1", "steps-v1", "text-stream-v1", "thread-tools-v1", "trusted-components-v1",
  "ui-catalog-v1", "ui-catalog-v2", "uploads-v1", "usage-v1",
].map((f) => page(`api/${f}`));
// Reference > Configuration: what an operator sets or a deployment must honour.
const configurationPages = ["config", "client-version", "model-injector"].map((f) => page(`api/${f}`));

// Every page of docs/api/ must be placed: a page in none of the groups would be on the site and in no sidebar.
{
  const placed = new Set([referencePage, ...apiPages, ...extensionPages, ...configurationPages].map((p) => p.link.replace(/^\/api\/?/, "")));
  const missing = readdirSync(join(root, "api"))
    .filter((f) => f.endsWith(".md") && f !== "README.md")
    .filter((f) => !placed.has(f.replace(/\.md$/, "")));
  if (missing.length) throw new Error(`docs/api pages in no sidebar group: ${missing.join(", ")}; add them to apiPages, extensionPages or configurationPages in .vitepress/config.ts`);
}

// The shared section structure: see STRUCTURE.md. The platform's and adam-rs's sites use the same one.
// Guides and Reference hold fixed groups; a group with no page is left out until it has one.
const withPages = (g: { items: unknown[] }) => g.items.length > 0;
const section = (text: string, groups: ReturnType<typeof group>[], collapsed = true) => ({ text, collapsed, items: groups.filter(withPages) });
const sidebar = [
  {
    text: "Overview",
    items: [
      page("vision", "Vision"),
      page("mvp", "MVP and build order"),
      page("lessons-from-agent-canvas", "Lessons as requirements"),
      page("open-questions", "Open questions"),
      group("Plans and checks", [...pages("plans"), page("vendor-check-2026-11-24", "Vendor check")]),
    ],
  },
  { text: "Get started", items: [page("getting-started", "Run it locally")] },
  // Guides: one task per page. No page of the repository is one yet, so the four groups are empty and not drawn.
  section("Guides", [group("Deploy", []), group("Operate", []), group("Use", []), group("Extend", [])]),
  section("Reference", [
    group("API", [referencePage, chatApiOperations, ...apiPages]),
    group("Extensions", extensionPages),
    group("Configuration", configurationPages),
    group("Errors", []),
  ]),
  { text: "Architecture", items: [page("concepts", "Concepts"), page("architecture", "Architecture"), page("orchestrator", "Orchestrator")] },
  { text: "Decisions", collapsed: true, items: [{ text: "ADR index", link: "/decisions/" }, ...pages("decisions", ["index.md"])] },
  { text: "Contributing", items: [{ text: "How to contribute", link: "/contributing" }] },
].filter((s) => s.items.length > 0);

// Prose writes <agent> or <ThreadShare> meaning a placeholder; Vue would read it as a component and fail the build.
// Only real HTML elements pass through, everything else is shown as the text it is.
// The API pages (api/reference.md, api/tags/, api/operations/) also use `script` and the components of vitepress-openapi:
// those are allowed there and nowhere else.
const API_ONLY = ["script", "oaspec", "oaoperation"];
const isApiPage = (relativePath = "") => /^api\/(reference\.md|tags\/|operations\/)/.test(relativePath);
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
      const env = rest[1] as { relativePath?: string } | undefined;
      const content: string = tokens[i].content;
      const tags = [...content.matchAll(/<\/?([A-Za-z][\w-]*)/g)].map((m) => m[1].toLowerCase());
      const wellFormed = /^\s*(<\/?[A-Za-z][\w-]*(\s+[\w:-]+(="[^"]*")?)*\s*\/?>\s*)+$/.test(content) || tokens[i].type === "html_block";
      if (wellFormed && tags.every((t) => HTML.has(t) || (API_ONLY.includes(t) && isApiPage(env?.relativePath)))) return original(tokens, i, ...rest);
      return md.utils.escapeHtml(content).replace(/\{\{/g, "&#123;&#123;");
    };
  }
}

export default defineMd3Config({
  title: "another-agentic-system",
  description: "A protocol-agnostic orchestration layer for multi-agent work.",
  base: process.env.DOCS_BASE ?? DEFAULT_BASE,
  cleanUrls: true,
  rewrites: { "api/README.md": "api/index.md" },
  // Pages link to source files outside docs/ (checked by tools/docs-check); every other dead link fails the build.
  ignoreDeadLinks: [/^\.\/(\.\.\/)+/, /(^|\/)examples\//],
  // Each operation and tag page has its own title, from the route's pageTitle.
  transformPageData(pageData) {
    const title = pageData.params?.pageTitle;
    if (typeof title === "string" && title) pageData.title = title;
  },
  // tester-note.md is a draft for the owner: it stays in the repository and is not on the site.
  srcExclude: ["**/node_modules/**", "**/examples/**", "tester-note.md"],
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
  markdown: { config: (md) => { md.use(inDepth); escapeUnknownTags(md); }, attrs: { disable: true } },
  // llms.txt, llms-full.txt and a .md per page, for agents that read the docs (ANR-1088, STRUCTURE.md).
  vite: {
    plugins: [
      llmstxt({
        title: "another-agentic-system",
        description: "A protocol-agnostic orchestration layer for multi-agent work: a chat surface and a stateless orchestrator over one Postgres event log.",
        details: "The chat API's machine-readable form is the OpenAPI file [chat-api.yaml](" + (process.env.DOCS_BASE ?? DEFAULT_BASE) + "api/chat-api.yaml).",
        ignoreFiles: ["api/reference.md", "api/operations/*", "api/tags/*", "tester-note.md"],
        injectLLMHint: false,
        sidebar: (s) => llmsSidebar(s as never) as never,
      }),
    ],
  },
});

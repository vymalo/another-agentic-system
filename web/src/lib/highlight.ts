import type { HighlighterCore, LanguageInput } from "shiki/core";

/*
 * Syntax highlighting of the code in an answer (web/README.md "Code and math in answers").
 *
 * Shiki with its **JavaScript regex engine**: the page's content security policy has no `wasm-unsafe-eval`
 * and must not gain it, and the engine that needs WebAssembly (oniguruma) is never imported. Everything here is
 * loaded on the first fenced block, and each language on its first use: a thread with no code downloads none of it.
 *
 * The languages are the ones an agent that works on this family of projects writes (Rust, TypeScript and
 * JavaScript, their JSX forms, JSON, shell, Python, SQL, Go, diffs, Dockerfiles, HTML and CSS). Any other
 * language, and a fence with none, stays plain text: never an error. Each is its own chunk of the build. YAML and
 * TOML are not in the list on purpose: their grammars have no licence we could verify (third-party-notices.md).
 *
 * A block over `MAX_CODE_CHARS` characters or `MAX_CODE_LINES` lines is not highlighted: shiki runs on the main
 * thread (200 000 characters took 1.3 s), so a huge block stays plain, as every block is until it is final.
 *
 * Two themes at once (GitHub's high-contrast pair, chosen for contrast on the code box in both schemes): a token
 * carries both colours as the custom properties `--shiki-light` and `--shiki-dark`, and the stylesheet picks one
 * by the page's theme (`<html data-theme>`, lib/theme.ts), never the device's.
 */

/** A coloured piece of one line: its text and the custom properties that colour it. */
export type Token = { content: string; style: Record<string, string> };

const LANGUAGES: Record<string, () => Promise<{ default: LanguageInput }>> = {
  rust: () => import("shiki/langs/rust.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  bash: () => import("shiki/langs/bash.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  diff: () => import("shiki/langs/diff.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  css: () => import("shiki/langs/css.mjs"),
};

const ALIASES: Record<string, string> = {
  rs: "rust",
  ts: "typescript",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  sh: "bash",
  shell: "bash",
  shellscript: "bash",
  zsh: "bash",
  console: "bash",
  py: "python",
  golang: "go",
  patch: "diff",
  docker: "dockerfile",
};

/** The largest block that is highlighted. Past either limit the block stays plain text. */
export const MAX_CODE_CHARS = 20_000;
export const MAX_CODE_LINES = 500;

/** Whether a block is small enough to highlight on the main thread. */
export function withinLimits(code: string): boolean {
  if (code.length > MAX_CODE_CHARS) return false;
  // the newline that ends the last line does not start another
  const end = code.endsWith("\n") ? code.length - 1 : code.length;
  let lines = 1;
  for (let at = code.indexOf("\n"); at !== -1 && at < end; at = code.indexOf("\n", at + 1)) {
    if (++lines > MAX_CODE_LINES) return false;
  }
  return true;
}

const has = (table: object, key: string) => Object.hasOwn(table, key);

/** The bundled language a fence's name stands for, or null (an unknown one, none, `text`). */
export function languageFor(name: string): string | null {
  const key = name.trim().toLowerCase();
  const language = has(ALIASES, key) ? ALIASES[key] : key;
  return language !== undefined && has(LANGUAGES, language) ? language : null;
}

const THEMES = { light: "github-light-high-contrast", dark: "github-dark-high-contrast" } as const;

let engine: Promise<HighlighterCore> | undefined;
const loaded = new Map<string, Promise<void>>();

function core(): Promise<HighlighterCore> {
  engine ??= (async () => {
    const [{ createHighlighterCore }, { createJavaScriptRegexEngine }, light, dark] =
      await Promise.all([
        import("shiki/core"),
        import("shiki/engine/javascript"),
        import("shiki/themes/github-light-high-contrast.mjs"),
        import("shiki/themes/github-dark-high-contrast.mjs"),
      ]);
    return createHighlighterCore({
      themes: [light.default, dark.default],
      langs: [],
      engine: createJavaScriptRegexEngine(),
    });
  })();
  // a failed load is tried again by the next block, not remembered
  engine.catch(() => {
    engine = undefined;
  });
  return engine;
}

async function ensure(highlighter: HighlighterCore, language: string): Promise<void> {
  let pending = loaded.get(language);
  if (!pending) {
    const load = LANGUAGES[language];
    if (!load) throw new Error(`no such language: ${language}`);
    pending = load().then((module) => highlighter.loadLanguage(module.default));
    loaded.set(language, pending);
    pending.catch(() => loaded.delete(language));
  }
  await pending;
}

/**
 * The tokens of `code`, one array per line, or null for a language the web does not bundle. It rejects when
 * the highlighter cannot load or cannot read the code: the caller shows plain text then.
 */
export async function highlightCode(code: string, name: string): Promise<Token[][] | null> {
  const language = languageFor(name);
  if (!language) return null;
  const highlighter = await core();
  await ensure(highlighter, language);
  const { tokens } = highlighter.codeToTokens(code.replace(/\n$/, ""), {
    lang: language,
    themes: THEMES,
    defaultColor: false,
  });
  return tokens.map((line) =>
    line.map((token) => ({ content: token.content, style: { ...token.htmlStyle } })),
  );
}

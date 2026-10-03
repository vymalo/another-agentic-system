# ADR 0038 — One agent definition, linked into each tool

- **Status:** accepted (2026-10-03, the owner's request: "Setup -system with the skills from adam-rs and create custom agents and
  sub agents configs that will reuse them. The agents configs shall be used by us for development later. So it must work with both
  Claude and other agents. You shall use FS links")

## Context

People and agents develop this repository with several tools, and each reads its own agent files from its own directory and in
its own format. Without one source, the definitions drift, or one tool's format breaks another's. Skills already solve this with one
copy in `.agents/skills/` and relative symlinks from each tool's directory (`CLAUDE.md`, *Skills*). Agents need the same, and where a
tool cannot follow a link, a generated file that points at the canonical one.

What each tool does with an agent file, checked on 2026-10-03:

| Tool | Reads | Finding | Status |
|---|---|---|---|
| Claude Code 2.1.288 | `.claude/agents/*.md` (Cursor reads it too) | Symlinked files are loaded; an unknown frontmatter key is ignored; `claude plugin validate` passes on the real files | *verified*: run in this container, 2026-10-03; docs https://code.claude.com/docs/en/sub-agents.md. Cursor: *unverified* (docs https://cursor.com/docs/context/subagents) |
| OpenCode 1.18.34 | `.opencode/agents/*.md` | Symlinks are followed. A Claude-style `tools: Read, Grep` string makes the whole configuration invalid (`tools` must be a map), and Claude's colour names fail its `color` schema. `model` must resolve to a provider | *verified*: run (`opencode agent list`), 2026-10-03; source `packages/opencode/src/config/agent.ts` at `907b3bc` |
| Goose (1.34 or later) | `.agents/agents/*.md` itself | Reads `name`, `description` and `model`, and passes `model` to its provider | *verified* in source (`crates/goose/src/agents/platform_extensions/summon.rs`, 2026-10-03); not run: no Goose binary here |
| Kiro CLI 2.27.1 | `.kiro/agents/*.json` | Does not read a Markdown agent. `prompt` accepts `file://`, resolved relative to the agent file. `agent validate` exits 0 even when it prints `Error:` | *verified*: run, 2026-10-03; docs https://kiro.dev/docs/custom-agents/ |
| Codex 0.160.0 | `.codex/agents/*.toml` | TOML; unknown keys are an error; `developer_instructions` is an inline string that nothing reads from a file | *verified* in source (`codex-rs/agent-roles`, `codex-rs/config` at `b741e48`, 2026-10-03); not run: needs a login |
| Gemini CLI | `.gemini/agents/*.md` | The frontmatter schema is strict (`kind`, `name`, `description`, `tools`, `model`, … and nothing else). The loader skips symlinks. A project agent must be acknowledged once per content hash | *verified* in source (`packages/core/src/agents/agentLoader.ts` at `fb972b2`, 2026-10-03); not run: needs a login |

## Decision

1. **One canonical file per agent: `.agents/agents/<name>.md`.** The body is the system prompt. The frontmatter is a portable
   subset: `name` (equal to the file name), `description` (a double-quoted string), `mode` (`primary`, `subagent` or `all`), `skills`
   (a list to preload), and for a restricted agent `readonly`, `disallowedTools` and `permission`.
2. **`tools`, `model` and `color` are forbidden** in the canonical file. `tools` has three incompatible shapes (a string for Claude, a
   map for OpenCode, a list for Kiro) and the first breaks OpenCode's whole configuration. `model` is passed to a provider by Goose
   and OpenCode, and one tool's alias is another's error. `color` has different vocabularies. No model is pinned anywhere:
   each tool runs the person's default.
3. **Links where a tool reads the format, adapters where it cannot.** `.claude/agents/<name>.md` and `.opencode/agents/<name>.md` are
   relative symlinks to the canonical file. Goose reads `.agents/agents/` and needs nothing. `.kiro/agents/<name>.json` carries a
   `file://` prompt that points at the canonical file. `.codex/agents/<name>.toml` and `.gemini/agents/<name>.md` hold a pointer
   prompt that tells the agent to read the canonical file; they exist for every agent that is not `primary`. **No adapter copies the
   prompt**, so there is nothing to drift.
4. **The five tool directories are generated-only.** `tools/agents/sync.mjs` writes them from the canonical files and deletes the
   ones whose agent is gone; `--check` changes nothing and fails on a missing, wrong or hand-edited entry, on a file it did not
   generate, and on an invalid canonical file (a forbidden key, a skill that does not resolve through the three skill mirrors,
   a relative Markdown link in the body). The Docs workflow runs `--check`.
5. **A restriction is expressed where a tool has a key for it.** `disallowedTools` for Claude, `permission` for OpenCode,
   `readonly` for Cursor, and the generated Kiro `tools` and Codex `sandbox_mode`. Gemini and Goose get it only from the prompt.
6. **Agent prompts cite repository paths as code spans**, because `tools/docs-check` scans the canonical files and Gemini does not
   load `CLAUDE.md`, so each prompt tells its agent to read it.

The roster, the routing and each agent's prompt are in `.agents/agents/`; `CLAUDE.md`, *Agents*, lists them.

## Consequences

- A new agent is one file and one run of `node tools/agents/sync.mjs`.
- A tool restriction is not the same everywhere: a read-only agent is enforced in Claude, OpenCode, Kiro and Codex, and rests on its
  prompt in Gemini and Goose.
- Gemini asks the person to acknowledge each project agent once. Codex loads `.codex/` only for a trusted project.
- Not run in this change (*unverified*): Codex, Gemini and Goose loading an agent, Kiro starting a chat with one (needs a login),
  and Cursor following the `.claude` links.
- Kiro's Markdown agents (documented for a later CLI) could replace the JSON adapters; revisit when a tool changes its format, and
  when one of the *unverified* rows is run.

## Alternatives rejected

- **Copy the file into each tool's directory.** Five copies drift, and the point of the request is one definition.
- **Make every tool's directory a link to `.agents/agents`.** Gemini skips symlinks, Codex and Kiro need another format, and a
  directory link would put `.md` files where Kiro expects JSON.
- **Use each tool's own keys in the shared frontmatter** (`tools`, `model`, `color`). One of them breaks OpenCode's whole
  configuration (*verified*, above).
- **A pinned model per agent.** It cannot be one value for all tools, and a person's default is the safer choice.

# The vendor check: can another service drive an outside command-line agent? (2026-11-24)

Slice 1 of the missions epic, [#342](https://github.com/vymalo/another-agentic-system/issues/342). This page records
what each candidate's own texts say, for slice 9 ("an outside worker": an adapter agent in a run pod drives one outside
product over ACP, on the person's own key, and returns a branch).

**It draws no conclusion.** What a vendor's terms allow is a legal reading, and it's the owner's. The page quotes and
cites; it doesn't say that any product may be shipped or adapted.

**Quotes are exact**, except that words in [brackets] replace or leave out a few of the vendor's words, under this
repository's wording rules. The full text is at each URL.

**Every fact is marked:**
- **verified** means it was read on 2026-11-24 at the URL given, or in the named repository at the named commit;
- **unverified** means it wasn't read, and the page says why.

**The candidates** are the command-line agents the images repository's `CLAUDE.md` lists as carried in its images:
Claude Code, Codex and OpenCode.

## The three candidates at a glance

| | Claude Code | Codex CLI | OpenCode |
|---|---|---|---|
| **Code licence** | Proprietary: "© Anthropic PBC. All rights reserved. Use is subject to Anthropic's Commercial Terms of Service." (verified: [LICENSE.md](https://github.com/anthropics/claude-code/blob/main/LICENSE.md)) | Apache-2.0 (verified: GitHub's licence field for `openai/codex`, `f64f003d`) | MIT (verified: GitHub's licence field for `anomalyco/opencode`, `40b6e9cd`; `sst/opencode` redirects there) |
| **Terms on being driven by another service** | Quoted below (verified) | **Unverified**: OpenAI's terms and the Codex auth page weren't read | **Unverified**: no terms page was read. OpenCode signs in to model providers, whose own terms would apply |
| **Headless mode** | `-p` / `--print`, `--output-format json\|stream-json`, `--bare`, permission modes (verified) | `codex exec`, with `--json` and `--ephemeral` (verified in the code) | `opencode run`, with `--format json`; `opencode serve` (verified in the docs source) |
| **ACP** | No native support found. An adapter repository exists, `agentclientprotocol/claude-agent-acp` (Apache-2.0): "Use Claude Agent SDK from any ACP client" (verified: its repository description) | No native support found. A repository `zed-industries/codex-acp` exists; what it covers is **unverified** (it has no description, and its README wasn't read) | **Native:** "`opencode acp` runs OpenCode as an Agent Client Protocol agent" (verified) |
| **Reports usage** | Yes: `usage` tokens and `total_cost_usd`, which the docs call "client-side estimates" (verified) | Yes: `turn.completed` carries `usage` with input, cached-input, cache-write and output tokens (verified in the code) | **Unverified** |

## Claude Code (Anthropic)

Source: [Legal and compliance](https://code.claude.com/docs/en/legal-and-compliance), read 2026-11-24, unless another
URL is given.

**Which terms apply:** *"Your use of Claude Code is subject to: Commercial Terms of Service - for Team, Enterprise, and
Claude API users; Consumer Terms of Service - for Free, Pro, Max, and Ultra users"*.

**Running it inside another service:** *"Unless we've mutually agreed otherwise, preinstalling or running Claude Code
in your products or services (e.g. in hosted sandboxes or other agent infrastructure) requires agreeing to our
Commercial Terms of Service and complying with the conditions below"*. The page then gives two conditions:
- *"The Claude Code binary must not be modified. Claude Code must be installed and run as published by Anthropic, and
  [those who run it] may not remove, disable, or restrict any authentication method built into it (including methods that
  permit signing in with a Claude account or the user's own API key)."*
- *"[Those who run it] may not [fund], resell, or intermediate Claude usage on their end users' behalf. Each end user must
  authenticate with their own Anthropic API key, Claude subscription plan credentials, or 3P inference provider
  credential (Amazon Bedrock, Google Cloud's Agent Platform, Microsoft Foundry). That usage is [accounted] directly to the
  end user under their own agreement with Anthropic or, for third-party inference providers, with the applicable
  provider."*

**Subscription sign-in and API keys:**
- *"OAuth authentication is intended exclusively for [holders] of Claude Free, Pro, Max, Ultra, Team, and Enterprise
  subscription plans and is designed to support ordinary use of Claude Code and other native Anthropic applications."*
- *"Developers building products or services that interact with Claude's capabilities, including those using the Agent
  SDK, should use API key authentication through Claude Console or a supported cloud provider. Anthropic does not permit
  third-party developers to offer Claude.ai login into their own applications, or to route requests through Free, Pro,
  Max, or Ultra plan credentials on behalf of their users. Moreover, developers may not collect, store, or intermediate
  Claude.ai credentials or session tokens — sign-in to a Claude account must complete through Anthropic's own flow."*
- *"Nor does it prevent an end user from signing in to the unmodified Claude Code binary with their own Claude
  subscription, including where a platform hosts Claude Code as described under* Can [operators] offer Claude Code in
  their products? *above."*
- *"Anthropic reserves the right to take measures to enforce these restrictions and may do so without prior notice."*

**Use and names:**
- *"Claude Code usage is subject to the Anthropic Usage Policy."*
- *"You can accurately say, in plain text, that your product has Claude Code preinstalled or that it runs Claude Code.
  But you can't use the Claude Code or Anthropic names or logos as part of your own product, feature, or company
  name…"*
- On the Agent SDK ([overview](https://code.claude.com/docs/en/agent-sdk/overview)): *"Use of the Claude Agent SDK is
  governed by Anthropic's Commercial Terms of Service, including when you use it to power products and services that
  you make available to your own […] end users…"*

**Headless** ([headless](https://code.claude.com/docs/en/headless), verified):
- *"Add the `-p` (or `--print`) flag to any `claude` command to run it non-interactively."*
- `--output-format text|json|stream-json`.
- `--bare` *"is the recommended mode for scripted and SDK calls"*, and in it *"Claude Code never reads OAuth credentials
  or the system keychain"*.
- Tools are allowed with `--allowedTools` and permission rules (for example `Bash(git diff *)`), and `--permission-mode`
  sets the mode.

**A custom endpoint** ([LLM gateway](https://code.claude.com/docs/en/llm-gateway), verified):
- *"`ANTHROPIC_BASE_URL` is the variable that points Claude Code at the gateway."*
- *"Anthropic … doesn't support routing Claude Code to non-Claude models through any gateway."*

**Usage** ([headless](https://code.claude.com/docs/en/headless) and
[cost tracking](https://code.claude.com/docs/en/agent-sdk/cost-tracking), verified):
- With `--output-format json`, the result carries `usage` (input, output, cache-creation and cache-read tokens),
  `modelUsage` and `total_cost_usd`.
- *"The `total_cost_usd` and `costUSD` fields are client-side estimates…"* The page says not to rely on them as authoritative figures.

**Not read (unverified):** the Consumer Terms and the Commercial Terms themselves (anthropic.com/legal), and whether
any Claude Code page mentions ACP.

## Codex CLI (OpenAI)

**Licence:** Apache-2.0 (verified: GitHub's licence field for `openai/codex`, read 2026-11-24).

**Headless**, verified in the code at `openai/codex` `f64f003d`:
- `codex exec` is the non-interactive command.
- It takes `--json` (`codex-rs/exec/src/cli.rs:90`) and `--ephemeral` (`:66`), and has a "dangerously bypass approvals
  and sandbox" flag (`:190`).
- The repository's own `docs/exec.md` and `docs/authentication.md` only point to developers.openai.com/codex
  (`/noninteractive` and `/auth`), which weren't read.

**Usage**, verified in the code: the JSON stream's `turn.completed` event carries `usage`, with `input_tokens`,
`cached_input_tokens`, `cache_write_input_tokens` and output tokens (`codex-rs/exec/src/exec_events.rs:19-21`, `:50-70`).

**ACP:** no native support was found in what was read. A repository `zed-industries/codex-acp` exists; its README
wasn't read, so what it does is unverified.

**Unverified:**
- OpenAI's Terms of Use, Services Agreement and Usage Policies;
- the Codex auth page (ChatGPT sign-in compared with an API key);
- any rule on running Codex for other people;
- setting a custom model endpoint (`model_providers`, `base_url`).

## OpenCode

**Licence:** MIT (verified: GitHub's licence field for `anomalyco/opencode`, which `sst/opencode` now redirects to,
read 2026-11-24).

**Headless**, verified in the docs source at `anomalyco/opencode` `40b6e9cd`, in
`services/site/src/docs/v1/content/cli.mdx`:
- `opencode run [message..]` runs non-interactively. `--format` is *"default (formatted) or json (raw JSON events)"*.
- `opencode serve` starts *"a headless server"*, and `opencode run --attach http://localhost:4096` reuses it.
- Credentials: *"you can use `opencode auth login` to configure API keys for any provider you'd like to use. This is
  stored in `~/.local/share/opencode/auth.json`."*

**ACP**, verified in `services/site/src/docs/content/cli/acp.mdx`:
- *"`opencode acp` runs OpenCode as an Agent Client Protocol agent, so you can use it from an ACP client such as Zed."*
- *"OpenCode uses the provider credentials it already has. Sign in from a terminal before you start the ACP client"*.

**Unverified:**
- whether the JSON events report token usage;
- the terms of OpenCode's own hosted services, if any;
- the terms of the model providers it signs in to.

## What wasn't read, and why

This session could reach the Claude Code documentation and GitHub's API, but couldn't fetch other web pages. Before
slice 9 picks a product, these must be read, with the date:

1. Anthropic's Consumer Terms and Commercial Terms (anthropic.com/legal).
2. OpenAI's Terms of Use, Services Agreement and Usage Policies, and the Codex pages
   [`/codex/auth`](https://developers.openai.com/codex/auth) and
   [`/codex/noninteractive`](https://developers.openai.com/codex/noninteractive).
3. The README of `zed-industries/codex-acp`.
4. OpenCode: whether `--format json` reports usage, and the terms of any OpenCode-hosted service.

## The question for the owner

**Which product, if any, does slice 9's outside worker adapt?** The candidates are Claude Code, Codex and OpenCode,
each on the person's own key, as #342 says. This page records only what each one's texts say, and the legal reading is
yours. Points that bear on it:
- Claude Code's terms, quoted above, set conditions for running it in a hosted sandbox: the Commercial Terms, an
  unmodified binary, the end user's own credentials, and no intermediated usage.
- Codex's and OpenCode's terms are still unread.
- OpenCode alone speaks ACP natively.

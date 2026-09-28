# Lessons from running OpenHands Agent Canvas

Before designing this, we ran [OpenHands Agent Canvas](https://github.com/OpenHands/OpenHands/tree/v1.24.0/helm/agent-canvas)
`1.24.0` on netcup (2026-09-28, `WhyThatFunction/home-os` PRs #166–#171,
image repo [vymalo/openhand-images](https://github.com/vymalo/openhand-images)).
Every item below was hit live, not predicted.

Since this system became protocol-only (ADR 0007), most items are requirements
on the **agent host** — another-agentic-platform — rather than on this
repository. Items 8–10 (MCP env, credentials in the browser, shared config
namespaces) also bind this system directly.

| # | What happened | Requirement it creates |
|---|---|---|
| 1 | **One pod does everything.** UI, API, automation server, editor, MCP servers and every agent's builds share one container. At 2 CPU / 4Gi a single `cargo build` pinned memory at the limit (rustc ~2.2 GB RSS), `/alive` stopped answering in 5s, the UI dropped and liveness headed for a restart. | Control plane and workers are separate. A worker's build can only starve its own sandbox. |
| 2 | **~2.3 GB of harness before any work** (agent-server, automation, 5 MCP servers via npx, editor, web server). | Keep the always-on footprint small; workers exist only while a task runs. |
| 3 | **It cannot scale out.** State (settings, SQLite, conversations, workspace) is on one RWO volume; extra replicas become independent instances with split state. | Stateless agents; state only in Postgres, git and caches. |
| 4 | **Work was lost on a restart.** agent-server put conversation git worktrees in `/tmp/conversation-worktrees`, on the container filesystem. Uncommitted changes vanished. | The durable artifact is a pushed branch. A worker pushes before it reports done. |
| 5 | **Toolchains reinstalled on every restart.** The image had no Rust/Flutter, so the agent ran rustup itself into `$HOME`. | Toolchains are baked into the worker image, under `/opt`. |
| 6 | **Volume mounts hide image contents.** Mounting a PVC over a path shadows whatever the image put there. | Toolchains under `/opt`; only caches/state under mounted paths; check the image before mounting. |
| 7 | **Debian login shells reset `PATH`** (`/etc/profile`), and agent terminals are login shells. | Export toolchain `PATH` from `/etc/profile.d/`, and smoke-test in a login shell. |
| 8 | **MCP stdio servers get a minimal env** (`HOME`, `PATH`, `SHELL`, `TERM`, `USER`, `LOGNAME`). `npm_config_cache` never reached them, their npx cache stayed unpersisted, and after each restart four servers re-downloaded at once and blew the single 30s tool-listing timeout. | Pre-install or cache MCP servers at their *default* paths; never rely on env vars reaching MCP subprocesses; budget tool-listing per server, not for the batch. |
| 9 | **The frontend shipped a backend credential.** The static server injects the backend session API key into the HTML, so anyone who can load the page can drive the agent. | Auth must cover every path, and no credential that grants execution ever reaches the browser. |
| 10 | **`npm_config_*` is shared namespace.** Setting `npm_config_store_dir` for pnpm made npm warn "Unknown env config store-dir" on every run. | Configure each tool in its own config file. |
| 11 | **pnpm 10 and 12 read different global config files** (`~/.config/pnpm/rc` vs `config.yaml`). | Test the versions projects actually pin, not just the latest. |
| 12 | **The agent had passwordless sudo** in a container on the cluster that also runs CI runners. | Sandboxes run without sudo, with NetworkPolicy, on nodes/namespaces isolated from CI. |
| 13 | **Agent CLIs vs ACP adapters.** The adapters (`claude-agent-acp`, `codex-acp`) shipped upstream; the CLIs you sign in with did not, and a user `npm i -g` hit EACCES. Logins live on disk (`~/.claude/.credentials.json` with `CLAUDE_CONFIG_DIR`, `~/.codex/auth.json`). | Decide per worker whether it uses subscription logins or gateway API keys; persist or inject accordingly. |
| 14 | **Recursive re-chown on every start.** The default `fsGroupChangePolicy` walks the whole volume. | `fsGroupChangePolicy: OnRootMismatch` on anything with a large volume. |

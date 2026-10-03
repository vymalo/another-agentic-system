---
name: bump-adam
description: "Bump adam-rs in this repo: dev/coder/UPSTREAM, the vendored dev/coder files, the x-adam-image tag+digest in compose.yaml and the adam-rs git revs in orchestrator/Cargo.toml, together."
---

# Bumping adam-rs

The adam-rs commit this repository follows (call it **B**, the one now pinned **A**) is written in
four places that must name the same commit. Moving only some of them is the bug this skill exists
to prevent:

| Where | What it holds |
|---|---|
| `dev/coder/UPSTREAM` | `commit=<40-hex>`, the commit the vendored files are copies of |
| `dev/coder/{agent,wiremock,git-server,podman,coder-agent}/` | byte-for-byte copies (the file list and the upstream path of each are in `UPSTREAM` and `dev/coder/check-vendored.sh`) |
| `compose.yaml`, `x-adam-image` | `ghcr.io/vymalo/another-adam-rs/coder:sha-<7>@sha256:<digest>`, one pin for the coder and for every agent that is only a folder |
| `orchestrator/Cargo.toml` (+ `Cargo.lock`) | seven `rev = "<40-hex>"` lines (`adam-host`, `adam-core`, `adam-runtime`, `adam-a2a`, `adam-a2a-runtime`, `adam-store-postgres`, `adam-notify-postgres`), used by the off-by-default feature `agent-local` (ADR 0015) |

The worked example is the bump `af1e715` to `b64e3fe`, vymalo/another-agentic-system#136 (two commits on its
branch, the image and the crates, squashed into one on `main`). `git log --oneline -3 -- dev/coder/UPSTREAM`
finds it; `git show <sha>` is the shape of a bump.

## Procedure

1. **Choose B** from the commits of adam-rs `main` whose image exists (step 5). Clone adam-rs
   next to this repository (`git clone https://github.com/vymalo/another-adam-rs`) and start
   from the **`adam-upgrade`** skill: it does the A to B analysis (commits since A, breaking
   changes, changed READMEs and environment variables, the list of known migrations). Do that
   first; the steps below are this repository's side of it.
2. **What did B change for this repository?** From the adam-rs clone:

   ```sh
   git diff --stat A B -- dev bin/adam-coder/agent          # the vendored files: empty means nothing to re-copy
   git diff A B -- dev/compose.github-app.yaml dev/coder-e2e.sh compose.yaml dev/compose.devcontainer.yaml
   git log --format='%h %s' A..B | grep '!:'                # commits that say they break
   ```

   The first diff decides step 3. The second lists the files that are **not** vendored on purpose
   (`dev/coder/UPSTREAM` says which: ours are `compose.yaml`, `dev/compose.devcontainer.yaml`,
   `dev/coder-e2e.sh`; `dev/compose.github-app.yaml` is a copy with its comments adapted); a
   change there is a change to port by hand, or to decide not to.
3. **Re-copy what changed**, never edit a copy here (`UPSTREAM` says why): `bin/adam-coder/agent/*`
   to `dev/coder/agent/*`; `dev/git-server`, `dev/podman`, `dev/coder-agent` to the same paths
   below `dev/coder/` (these three hold *exactly* upstream's files, a file added or removed
   upstream is added or removed here); `dev/wiremock/<mock>/` to `dev/coder/wiremock/<mock>/`
   (a deliberate subset: the mappings of the scripted coder run, and every `bodyFileName` a copied mapping names).
   A mapping the coder starts to need shows up as an unmatched request in `dev/coder-e2e.sh`.
   When nothing changed (as in `af1e715` to `b64e3fe`), copy nothing and say so in the ADR note.
4. **Set `commit=` in `dev/coder/UPSTREAM`** to B's full sha (40 characters, lower case).
5. **Pin the image by tag and digest.** The tag is `sha-` plus the first **7** characters of B
   (`git rev-parse B | cut -c1-7`, not `--short`, which may print more; `check-vendored.sh` asserts
   `sha-<7>`). Anonymous pull works for the package:

   ```sh
   TOKEN=$(curl -s "https://ghcr.io/token?scope=repository:vymalo/another-adam-rs/coder:pull" \
     | python3 -c 'import sys,json;print(json.load(sys.stdin)["token"])')
   A='Accept: application/vnd.oci.image.index.v1+json, application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json'
   curl -sI -H "Authorization: Bearer $TOKEN" -H "$A" \
     https://ghcr.io/v2/vymalo/another-adam-rs/coder/manifests/sha-<7> | grep -iE '^(HTTP|docker-content-digest)'
   ```

   `200` and a `docker-content-digest` line: the tag exists. Then check it is B: GET the manifest
   with the same headers, fetch the blob named by `config.digest`
   (`.../blobs/<config digest>`, `curl -L` with the token) and compare
   `.config.Labels["org.opencontainers.image.revision"]` with B's full sha (the bump to `b64e3fe` also
   recorded one `linux/amd64` manifest, thirteen layers, 2.92 GB; take the platform's manifest first if
   the answer is an index). In `compose.yaml` write `x-adam-image: &adam-image ghcr.io/vymalo/another-adam-rs/coder:sha-<7>@sha256:<digest>`
   and update the comment block above it (the manifest facts, the label, the adam-rs pull requests the
   image now contains). Do not add a second line that names an image of that repository: the
   services take `*adam-image`.
6. **Move the crates.** Replace the `rev =` of **all seven** lines in `orchestrator/Cargo.toml` with B's
   full sha (a mix gives two copies of the same traits), then update the lockfile:

   ```sh
   cd orchestrator
   cargo update -p adam-host --precise <B, 40-hex>      # and the same for each other adam crate if the lock still names A
   grep -c 'another-adam-rs?rev=<B>' Cargo.lock          # 7 packages; none may still say A
   ```

   The bump to `b64e3fe` did this and the lockfile also let `tempfile` resolve a newer `getrandom`
   (a side effect of re-resolving, not of adam): name such changes in the commit. A plain
   `cargo update` moves far more, so prefer `-p`.
   Then migrate code the breaking commits of step 2 require (adam's `Store` gained a required
   method `lease_until` in `7e5dcc3`; nothing here implements adam's `Store`, the orchestrator
   uses adam's Postgres store as it is, so nothing had to change). Read `orchestrator/crates/agent-adam/README.md`
   for the surface this repository uses; `adam-embed` describes the crates.
7. **Say it in the docs** (the repository's rules: dated, with *verified*/*unverified*):
   - an ADR **status note** at the end of `docs/decisions/0014-adam-coder-default-agent-over-a2a.md`
     (amend, never rewrite: `write-adr`): `### Status note, <date>: <what the bump brings> (adam-rs <B7>)`, B as A plus which
     adam-rs pull requests, whether a vendored file changed
     (`git diff A B -- dev bin/adam-coder/agent` empty or not), that the crates moved with it, and a
     *Verified <date>* bullet (anonymous ghcr API, HTTP 200, the manifest, the revision label, the digest
     prefix, `check-vendored.sh` passes) and an *Unverified* bullet (what was not run: the 2.9 GB image
     usually is not pulled locally);
   - the ADR whose behaviour the bump delivers, if there is one (the `b64e3fe` bump added a note to
     `docs/decisions/0036-sending-while-an-agent-works.md`);
   - every sentence that names the pin: `grep -rn '<A7>' --exclude-dir=.git --exclude-dir=node_modules --exclude=Cargo.lock .`
     (for example `dev/README.md`, "the pin is `...`", and `orchestrator/crates/agent-adam/README.md`). Older dated
     notes in the ADRs and in `dev/README.md` stay as they are: they say what was true then;
   - a crate's README changes in the same commit as its public API, environment variables or tests
     (CLAUDE.md, *Code*).
8. **Commit in two** on the branch, as in #136 (the merge squashes them): `build(compose): pin the coder to adam-rs <B7> (<what it brings>)`
   (`compose.yaml`, `UPSTREAM`, vendored copies, the ADR 0014 note) and
   `build(orchestrator): move the adam-rs crates of agent-local to <B7>` (`Cargo.toml`, `Cargo.lock`, the READMEs, the other ADR note).

## Verify

```sh
dev/coder/check-vendored.sh                          # the copies equal upstream at B, nothing missing, compose.yaml pins sha-<7>@sha256 (network)
docker compose --profile '*' config -q               # compose.yaml is valid for every profile
cd orchestrator
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p orchestrator --features agent-local --all-targets --locked -- -D warnings
cargo test -p orchestrator --features agent-local --locked
cargo test -p orch-agent-adam --locked
cd .. && node tools/docs-check/check-docs.mjs        # "docs OK" (npm --prefix tools/docs-check ci once per clone)
```

Then open the pull request and wait for **Coder E2E** (`.github/workflows/coder-e2e.yml`: its path
filter includes `compose.yaml`, `dev/**` and `orchestrator/**`, so a bump always runs it). It pulls the
new image and runs `dev/e2e-all.sh`, every scenario on the new pin, then the GitHub App, work-environment
and split passes. It is the only run that proves the image works with this stack; `check-vendored.sh`
only proves the files agree. Say in the pull request which of these you ran yourself and which only CI ran.
The Rust checks need a lot of disk and time (the first `agent-local` build fetches adam-rs by git).

## Pitfalls

- **The tag is `sha-` plus exactly 7 characters**, and `check-vendored.sh` fails when it is not the
  first 7 of the `commit=` in `UPSTREAM`.
- **A new image may 404** until adam-rs's `coder` workflow for that commit has finished (the
  `b64e3fe` check answered 404 while the previous commit's tag already answered 200). Wait, or pin
  the commit before. A tag of a private package behaves the same: the token request still answers,
  the manifest request does not return 200.
- **Digest, not tag alone**, and the digest is the registry's `Docker-Content-Digest` of the manifest
  (also the sha-256 of the body it returns); check the revision label equals B.
- **Seven `rev` lines, one sha.** A branch or a tag as `rev` is not reproducible; a lockfile that
  still names A for one crate fails `--locked` or builds two copies of adam's traits.
- **A trait with a new required method** breaks every implementer, not every user: grep this
  repository for `impl .* for` on adam's traits before assuming a compile error is yours
  (`adam-upgrade` has the known list).
- **A change with no compile error can still change behaviour** (leases, claims, task states):
  read the ADRs the log names; it is why Coder E2E, not `cargo build`, is the proof.
- **Vendored copies are never edited here.** A change we need is made in adam-rs first; to try one
  before it is upstream, point `CODER_AGENT_DIR` at a copy (`dev/README.md`, "Change what the coder says").
- **`docs-check` fails on a broken link** in a note: a new ADR anchor or file is a real path.

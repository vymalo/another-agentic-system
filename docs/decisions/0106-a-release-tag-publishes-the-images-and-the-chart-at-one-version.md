---
sidebarTitle: "0106 · Release images and chart by tag"
---

# ADR 0106 — A release tag publishes the images and the chart at one version

- **Status:** **Accepted (2026-11-27)** by the owner: *"Cheat how I'm doing [here](https://github.com/vaam-apps/vpay/pull/417) to have
  automatic commits collections for building versions ... a similar flow for all of them. In prod we'll deploy only fixed versions
  like that."* The platform's twin is its AD-070 (vymalo/another-agentic-platform#129).

## Context

release-please (`release-please.yml`, an App token) already collects the Conventional Commits of `main` into one release pull
request and cuts `vX.Y.Z` when it merges; until now a tag versioned only the Tauri app, whose desktop and mobile workflows build
from the published release. The server side had no version at all: `orchestrator.yml` and `web.yml` push `sha-<7>` (and `latest`)
on every green build of `main`, their bump jobs write that tag into `deploy/chart/values.yaml`, and Argo CD follows `HEAD`. The
chart's own `version` (0.2.0) and `appVersion` (0.1.0) never moved.

Two facts shape how a tag can publish images:

- **A release commit builds no image.** It changes `CHANGELOG.md`, the manifest and the version files (v0.7.1's commit `9fdc245`
  changed six such files and nothing under `orchestrator/` or `web/`), and both image workflows run only on their own paths. So
  there is no `sha-<7>` of the tag's commit to copy.
- **The web's image carried no revision label.** Its config held Caddy's base labels, `org.opencontainers.image.source` among them
  pointing at the Caddy image's repository (*verified 2026-11-27* on `web:sha-a6fb6d7`'s config at ghcr.io). The orchestrator's
  has `org.opencontainers.image.revision` from `docker/metadata-action` (*verified* the same day on `orchestrator:sha-794618a`:
  `794618a4…`). The web's static export does inline the commit it was built from (`NEXT_PUBLIC_BUILD_REVISION`) and the edge's
  sign-in path (`NEXT_PUBLIC_SIGN_IN_PATH=/oauth2/start`): both strings are in a chunk of `web:sha-a6fb6d7`'s `/srv`
  (*verified 2026-11-27* by reading that layer).

## Decision

1. **A tag `vX.Y.Z` publishes three things at `X.Y.Z`** (`.github/workflows/release.yml`, on the tag, or `workflow_dispatch` with
   the tag from `main`): `ghcr.io/vymalo/another-agentic-system/orchestrator` and `.../web` as `X.Y.Z` and `X.Y`, and the chart as
   `oci://ghcr.io/vymalo/another-agentic-system/charts/another-agentic-system` `X.Y.Z`. **Never `latest`**; `X.Y` moves only to the
   highest patch of its minor. Production pins `X.Y.Z`. (`main`'s own `sha-<7>` and `latest` stay what they are, for development.)
2. **The images are copied, not rebuilt.** For each workflow, the image of a release is the `sha-<7>` of the newest commit at or
   before the tag that changed that workflow's `on.push.paths` (`tools/release-source.sh`, which reads the paths from the
   workflow, so they cannot drift): the same tree under those paths as the tag, built and smoke-tested by `main`, pushed only
   after its checks passed. `release.yml` waits for it, refuses an image whose `org.opencontainers.image.revision` label is not
   that commit, and for the web also one whose export does not inline that commit and `/oauth2/start`; then it copies it **by
   digest** (`imagetools create --prefer-index=false`). If that commit's run did not push, nothing is published and there is no
   fallback build.
3. **`web.yml` labels its image** with `org.opencontainers.image.revision` and `.source` of this repository, on the build it smoke
   tests and on the one it pushes. The web's build-time values were checked rather than rebuilt: `main`'s image is built with
   `NEXT_PUBLIC_SIGN_IN_PATH=/oauth2/start`, the one value the chart's edge needs, so it is the release image.
4. **Signed and attested in one job.** Only the publish job holds `id-token: write`. Each image digest and the chart digest get a
   cosign keyless signature (identity: `release.yml` at a `v` tag, or on `main` for a re-run); each image an SPDX SBOM made by
   syft and attested with `actions/attest`, and a provenance attestation of the publishing run, whose summary names the run that
   built the image. Every action is pinned by commit SHA.
5. **Git keeps `sha-<7>`; the package says the version.** `deploy/chart/release-pin.sh` sets `orchestrator.image` and `web.image`
   to `X.Y.Z` and their digests in a **copy** of the chart and refuses the file git tracks, so `bump-tag.sh` on `main` stays the
   only writer of those values in git. The chart's `_validate.tpl` accepts a first-party tag `X.Y.Z` only with a
   `sha256:` digest; `sha-<7>` as before; nothing else.
6. **A published version is never overwritten.** An existing image tag `X.Y.Z` must be the same digest; an existing chart version
   must have the same `values.yaml`, and is then only signed again.
7. **One version, release-please's.** `deploy/chart/Chart.yaml`'s `version` and `appVersion` carry `# x-release-please-version`
   and are in `extra-files`. The Tauri app keeps following (its `Cargo.toml`, `package.json` and `tauri.conf.json` were already
   there): its desktop and mobile releases are built from the same tag and named by it, so one tag names one version of
   everything it releases. `tools/check-release-version.sh` holds the files and markers on every pull request that touches
   them, the single version on release-please's pull request, and the tag's version before anything is published.
8. **The release pull request needs no governance exemption.** release-please writes the AI Governance template's sections from
   the config's `pull-request-header`, and the check passes on that body (#596, `governance / governance-check` green at
   `ad14529`, *verified 2026-11-27*). `governance.yml` is left as it is.

## Consequences

- A deployment pins `another-agentic-system` `X.Y.Z` from the OCI registry and leaves the image values alone; home-os can keep
  following `HEAD` until it moves to a version (its change, not this repository's).
- A release depends on `main` being green for both images at the tag: a red or cancelled run of the source commit blocks it until
  its failed jobs are re-run, then `workflow_dispatch` with the tag publishes. Merging the release pull request right after an
  image change makes `release.yml` wait (up to 160 minutes) for that build.
- The first release after this ADR moves the chart from 0.2.0 and 0.1.0 to the release's version. Every version change moves the
  labels `helm.sh/chart` and `app.kubernetes.io/version`, and the pods' configuration and Secret checksums include them: a
  release rolls those pods once. `render-check.sh`'s goldens drop those two labels, so its 1321 checks pass with a moved version
  (run with `Chart.yaml` at 0.9.0, 2026-11-27).
- GHCR makes a new package private: the chart's package is made public by hand the first time.
- *Unverified until a tag runs it:* the images job (the wait, the label and the web's inlined strings read through `imagetools`
  and `docker cp`), the copy by digest, cosign, the attestations, `helm push` and the "not found" answer of `helm show chart` with
  Helm 4, and that release-please moves `Chart.yaml`'s two marked lines as it moves the Tauri app's `Cargo.toml`. The `version`
  and `chart` jobs ran locally as written, with Helm v4.3.2 (the dry run's outputs, a packaged `another-agentic-system-0.8.0.tgz`
  with the four pins, and the refusals of a bad tag and of a dispatch from another branch).
- Not decided here: `web/package.json`'s version (0.1.0, the `X-Client-Version` the web and the apps state, ADR 0076) does not
  follow the release; moving it would also make every release commit rebuild the web.

## Amended 2026-11-27 (ANR-1197)

`orchestrator.yml` and `web.yml` had `cancel-in-progress: true` on every ref, so the next merge to `main` cancelled a build that was
still running, and the commit a release copies could lose its `sha-<7>` image. The same failure hit the sibling coder repository's
release gate (its pull request #493). The fix differs from theirs in one point: there the release commit's own run is the one a
release needs; here the release commit touches no path of either workflow and has no run, and the image a release copies is the
one of the newest commit that changed those paths (`tools/release-source.sh`), which cannot be known when it is pushed.

- A pull request keeps its group (its ref) and still cancels. Every push to `main` gets a group of its own, its sha, and is never
  cancelled. A shared group with `cancel-in-progress: false` would not do: a group keeps one running and one pending run, and a
  newer pending run cancels the older pending one. The cost is that a burst of merges builds in parallel instead of skipping to
  the newest.
- The gate in `release.yml` already reads the registry, not the run's conclusion, so a run whose image job passed is accepted
  whatever a later job did; a comment now says so. It was not changed otherwise.
- *Unverified until a burst of merges runs it:* that no run of `main` is cancelled.


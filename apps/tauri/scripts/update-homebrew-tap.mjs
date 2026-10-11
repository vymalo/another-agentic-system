import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const branch = "bot/another-agentic-desktop";
const repo = "vymalo/homebrew-tap";
const files = ["Casks/another-agentic.rb", "Formula/another-agentic.rb"];

function compareVersions(left, right) {
  const a = left.split(".").map(BigInt);
  const b = right.split(".").map(BigInt);
  for (let index = 0; index < 3; index++) {
    if (a[index] !== b[index]) return a[index] > b[index] ? 1 : -1;
  }
  return 0;
}

export function updateTap({ cwd, base, version, shas, run }) {
  if (!/^\d+\.\d+\.\d+$/.test(version) || shas.length !== 4 || !shas.every((sha) => /^[a-f0-9]{64}$/.test(sha))) {
    throw new Error("Expected a stable version and four SHA-256 checksums");
  }
  const execute = run ?? ((command, args) => execFileSync(command, args, {
    cwd,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: {
      ...process.env,
      // Pass credentials through the environment, never through logged command arguments.
      ...(process.env.GH_TOKEN ? {
        GIT_CONFIG_COUNT: "1",
        GIT_CONFIG_KEY_0: "http.https://github.com/.extraheader",
        GIT_CONFIG_VALUE_0: `AUTHORIZATION: bearer ${process.env.GH_TOKEN}`,
      } : {}),
    },
  }).trim());
  const git = (...args) => execute("git", args);
  git("check-ref-format", `refs/heads/${base}`);
  if (git("status", "--porcelain")) throw new Error("Tap checkout must be clean before updating");
  git("fetch", "origin", `+refs/heads/${base}:refs/remotes/origin/${base}`);
  const remoteHead = git("ls-remote", "--heads", "origin", `refs/heads/${branch}`).split(/\s/)[0];
  if (remoteHead) git("fetch", "origin", `+refs/heads/${branch}:refs/remotes/origin/${branch}`);
  const refs = [`origin/${base}`, ...(remoteHead ? [`origin/${branch}`] : [])];
  for (const ref of refs) {
    for (const file of files) {
      if (!git("ls-tree", "--name-only", ref, "--", file)) continue;
      const text = git("show", `${ref}:${file}`);
      const existing = text.match(/version "(\d+\.\d+\.\d+)"/)?.[1]
        ?? text.match(/\/download\/v(\d+\.\d+\.\d+)\//)?.[1];
      if (!existing) throw new Error(`Cannot determine the current version in ${ref}:${file}`);
      if (compareVersions(version, existing) < 0) return "stale";
    }
  }
  // Only this workflow owns the bot branch. Rebuild it from the fresh default branch,
  // preserving unrelated tap changes, and lease against the remote SHA observed above.
  git("checkout", "-B", branch, `origin/${base}`);
  execute(process.execPath, [fileURLToPath(new URL("./write-homebrew-tap.mjs", import.meta.url)), cwd, version, ...shas]);
  git("add", "--", ...files);
  if (!git("diff", "--cached", "--name-only", "--", ...files)) return "unchanged";
  const desiredTree = git("write-tree");
  const pendingTree = remoteHead ? git("rev-parse", `${remoteHead}^{tree}`) : "";
  if (desiredTree !== pendingTree) {
    git("-c", "user.name=release-please[bot]", "-c", "user.email=release-please[bot]@users.noreply.github.com", "commit", "-m", `chore: update another-agentic to ${version}`);
    git("push", `--force-with-lease=refs/heads/${branch}:${remoteHead}`, "origin", `HEAD:refs/heads/${branch}`);
  } else {
    // The generated index is exactly the remote tree; keep its existing commit too.
    git("reset", "--soft", remoteHead);
  }
  const existing = JSON.parse(execute("gh", ["pr", "list", "--repo", repo, "--state", "open", "--head", branch, "--base", base, "--json", "url"]));
  if (existing.length > 1) throw new Error("Multiple open tap update pull requests require manual reconciliation");
  const directory = mkdtempSync(join(tmpdir(), "tap-pr-body-"));
  try {
    const bodyFile = join(directory, "body.md");
    const release = `https://github.com/vymalo/another-agentic-system/releases/tag/v${version}`;
    const evidence = process.env.GITHUB_RUN_ID
      ? `https://github.com/vymalo/another-agentic-system/actions/runs/${process.env.GITHUB_RUN_ID}`
      : "https://github.com/vymalo/another-agentic-system/actions/workflows/desktop-release.yml";
    writeFileSync(bodyFile, `## 1. Summary\n\nUpdate another-agentic to ${version} from [the source release](${release}).\n\n## 2. Intent\n\nDistribute the released desktop binaries through Homebrew.\n\n## 3. Scope\n\nmacOS Cask and Linux Formula for arm64 and x64; no Windows or mobile distribution.\n\n## 4. Verification\n\nThe calling [desktop-release workflow](${evidence}) requires all four Homebrew installation smoke jobs to succeed before this update runs. Checksums come from its attached release assets. This script does not itself rebuild or launch the application.\n\n## 5. Screenshots / Evidence\n\n[Release workflow](${evidence}) and [release assets](${release}).\n\n## 6. Risk Assessment\n\nMedium: desktop binaries are unsigned; macOS Gatekeeper may require explicit user approval. Review checksums and architecture mappings before merging.\n\n## 7. AI Usage Declaration\n\n- [x] Generating the release automation implementation and tests\n\nThis update is mechanically generated; no human approval or app-runtime verification is implied.\n\n## 8. Reviewer Focus\n\nVerify version, checksums, architecture mapping, and source release readiness.\n`);
    const options = ["--title", `chore: update another-agentic to ${version}`, "--body-file", bodyFile];
    if (existing.length) {
      execute("gh", ["pr", "edit", existing[0].url, ...options]);
      return "updated";
    }
    execute("gh", ["pr", "create", "--repo", repo, "--base", base, "--head", branch, ...options]);
    return "created";
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [cwd, base, version, ...shas] = process.argv.slice(2);
  console.log(updateTap({ cwd: resolve(cwd), base, version, shas }));
}

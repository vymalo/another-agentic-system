import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { updateTap } from "./update-homebrew-tap.mjs";

const shas = ["a", "b", "c", "d"].map((value) => value.repeat(64));
const writer = new URL("./write-homebrew-tap.mjs", import.meta.url).pathname;

function fixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "tap-lifecycle-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const remote = join(directory, "remote.git");
  const cwd = join(directory, "checkout");
  execFileSync("git", ["init", "--bare", remote], { stdio: "ignore" });
  execFileSync("git", ["clone", remote, cwd], { stdio: "ignore" });
  const git = (...args) => execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  git("config", "user.name", "test");
  git("config", "user.email", "test@example.invalid");
  git("checkout", "-b", "main");
  writeFileSync(join(cwd, "README.md"), "Tap\n");
  git("add", ".");
  git("commit", "-m", "initial");
  git("push", "origin", "main");
  const calls = [];
  let pr = [];
  const run = (command, args) => {
    if (command !== "gh") return execFileSync(command, args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
    calls.push(args);
    if (args[1] === "list") {
      assert.equal(args[args.indexOf("--head") + 1], "bot/another-agentic-desktop");
      return JSON.stringify(pr);
    }
    const body = readFileSync(args[args.indexOf("--body-file") + 1], "utf8");
    assert.match(body, /AI Usage Declaration/);
    assert.match(body, /Verification/);
    return "https://github.com/vymalo/homebrew-tap/pull/1";
  };
  return { cwd, remote, git, run, calls, setPr: (value) => { pr = value; }, update: (version) => updateTap({ cwd, base: "main", version, shas, run }), generate: (version) => execFileSync(process.execPath, [writer, cwd, version, ...shas]) };
}

test("creates the bot branch and exactly one release update PR", (t) => {
  const f = fixture(t);
  assert.equal(f.update("0.4.1"), "created");
  assert.match(f.git("show", "origin/bot/another-agentic-desktop:Casks/another-agentic.rb"), /version "0.4.1"/);
  assert.equal(f.calls.filter((args) => args[1] === "create").length, 1);
});

test("updates the existing PR and preserves unrelated default-branch changes", (t) => {
  const f = fixture(t);
  f.update("0.4.1");
  f.git("checkout", "main");
  writeFileSync(join(f.cwd, "README.md"), "New upstream instructions\n");
  f.git("add", "."); f.git("commit", "-m", "docs"); f.git("push", "origin", "main");
  f.setPr([{ url: "https://github.com/vymalo/homebrew-tap/pull/1" }]);
  assert.equal(f.update("0.5.0"), "updated");
  assert.equal(f.git("show", "origin/bot/another-agentic-desktop:README.md"), "New upstream instructions");
  assert.equal(f.calls.filter((args) => args[1] === "create").length, 1);
});

test("rerunning unchanged pending contents reuses the branch and PR", (t) => {
  const f = fixture(t);
  f.update("0.4.1");
  const previous = f.git("rev-parse", "origin/bot/another-agentic-desktop");
  f.setPr([{ url: "https://github.com/vymalo/homebrew-tap/pull/1" }]);
  assert.equal(f.update("0.4.1"), "updated");
  assert.equal(f.git("rev-parse", "origin/bot/another-agentic-desktop"), previous);
  assert.equal(f.git("status", "--porcelain"), "");
});

test("missing PR for an unchanged pending branch is recovered", (t) => {
  const f = fixture(t);
  f.update("0.4.1");
  assert.equal(f.update("0.4.1"), "created");
});

test("merged same-version release is a no-op with no stale PR retitling", (t) => {
  const f = fixture(t);
  f.update("0.4.1");
  const retainedHead = f.git("rev-parse", "origin/bot/another-agentic-desktop");
  f.git("checkout", "main");
  f.git("merge", "--squash", "origin/bot/another-agentic-desktop");
  f.git("commit", "-m", "squashed release"); f.git("push", "origin", "main");
  f.calls.length = 0;
  f.setPr([]);
  assert.equal(f.update("0.4.1"), "unchanged");
  assert.equal(f.calls.length, 0);
  assert.equal(f.git("rev-parse", "origin/bot/another-agentic-desktop"), retainedHead);
});

test("an older release cannot replace a newer pending or merged version", (t) => {
  const f = fixture(t);
  f.update("0.10.0");
  const previous = f.git("rev-parse", "origin/bot/another-agentic-desktop");
  assert.equal(f.update("0.9.0"), "stale");
  assert.equal(f.git("rev-parse", "origin/bot/another-agentic-desktop"), previous);
  f.git("checkout", "main");
  f.generate("1.0.0"); f.git("add", "."); f.git("commit", "-m", "release"); f.git("push", "origin", "main");
  assert.equal(f.update("0.11.0"), "stale");
});

test("unknown existing tap version fails closed before touching the bot branch", (t) => {
  const f = fixture(t);
  f.generate("0.4.1");
  writeFileSync(join(f.cwd, "Casks/another-agentic.rb"), 'cask "another-agentic" do\n  version :latest\nend\n');
  f.git("add", "."); f.git("commit", "-m", "unknown version"); f.git("push", "origin", "main");
  assert.throws(() => f.update("0.5.0"), /Cannot determine the current version/);
  assert.equal(f.calls.length, 0);
});

test("a competing remote branch writer is rejected by the explicit push lease", (t) => {
  const f = fixture(t);
  f.update("0.4.1");
  f.setPr([{ url: "https://github.com/vymalo/homebrew-tap/pull/1" }]);
  f.calls.length = 0;
  const run = (command, args) => {
    if (command === "git" && args[0] === "push") {
      execFileSync("git", ["--git-dir", f.remote, "update-ref", "refs/heads/bot/another-agentic-desktop", f.git("rev-parse", "origin/main")]);
    }
    return f.run(command, args);
  };
  assert.throws(() => updateTap({ cwd: f.cwd, base: "main", version: "0.5.0", shas, run }), /stale info/);
  assert.equal(f.calls.length, 0);
});

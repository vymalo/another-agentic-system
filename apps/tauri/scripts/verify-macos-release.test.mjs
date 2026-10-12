import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import test from "node:test";

// Only Apple's PlistBuddy is real here. These tests exercise the release helper's
// failure propagation and cleanup, not Apple signature/notarization services.
const boundaryStub = `#!${process.execPath}
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
const root = process.env.FIXTURE_ROOT;
const failure = process.env.FAIL_PHASE;
if (command === "hdiutil") {
  if (args[0] === "verify" && failure === "image") process.exit(1);
  if (args[0] === "attach") {
    const mount = args[args.indexOf("-mountpoint") + 1];
    fs.writeFileSync(path.join(root, "mounted-path"), mount);
    fs.cpSync(path.join(root, "fixture.app"), path.join(mount, "fixture.app"), { recursive: true });
  }
  if (args[0] === "detach") {
    fs.rmSync(path.join(args[1], "fixture.app"), { recursive: true });
    fs.writeFileSync(path.join(root, "detached"), "yes");
  }
} else if (command === "codesign") {
  if (args[0] === "--verify") {
    assert.ok(args.includes("--deep") && args.includes("--strict"));
  }
  if (args[0] === "--verify" && failure === "signature") process.exit(1);
  if (args[0] === "--display") {
    console.error("TeamIdentifier=" + (failure === "team" ? "ZYXWVUTSRQ" : "ABCDEFGHIJ"));
    console.error(failure === "authority" ? "Authority=Apple Distribution: Fixture (ABCDEFGHIJ)" : "Authority=Developer ID Application: Fixture (ABCDEFGHIJ)");
  }
} else if (command === "spctl") {
  assert.ok(args.includes("--assess") && args.includes("execute"));
  if (failure === "gatekeeper") process.exit(1);
} else if (command === "xcrun") {
  assert.deepEqual(args.slice(0, 2), ["stapler", "validate"]);
  if (failure === "ticket") process.exit(1);
}
`;

for (const phase of ["accepted", "signature", "authority", "team", "bundle", "gatekeeper", "ticket", "image"]) {
  test(`actual DMG verification helper ${phase === "accepted" ? "accepts all successful boundaries" : `rejects ${phase} failure`} and cleans its mount`,
    { skip: process.platform !== "darwin" }, () => {
      const root = mkdtempSync(join(tmpdir(), "another-agentic-verification-test-"));
      try {
        const bin = join(root, "bin");
        const bundle = join(root, "bundle");
        mkdirSync(bin);
        mkdirSync(join(bundle, "dmg"), { recursive: true });
        mkdirSync(join(root, "fixture.app", "Contents"), { recursive: true });
        writeFileSync(join(bundle, "dmg", "fixture.dmg"), "controlled image fixture");
        const identifier = phase === "bundle" ? "com.example.wrong" : "com.vymalo.agentic";
        writeFileSync(join(root, "fixture.app", "Contents", "Info.plist"),
          `<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${identifier}</string></dict></plist>`);
        for (const name of ["hdiutil", "codesign", "spctl", "xcrun"]) {
          writeFileSync(join(bin, name), boundaryStub, { mode: 0o700 });
        }
        const result = spawnSync("bash", [fileURLToPath(new URL("./verify-macos-release.sh", import.meta.url)), bundle], {
          env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, TMPDIR: root,
            APPLE_DEVELOPMENT_TEAM: "ABCDEFGHIJ", FIXTURE_ROOT: root, FAIL_PHASE: phase },
          encoding: "utf8",
        });
        if (phase === "accepted") assert.equal(result.status, 0, result.stderr);
        else assert.notEqual(result.status, 0, `helper must reject ${phase}`);
        const mounts = join(root, "mounted-path");
        if (phase !== "image") {
          assert.equal(existsSync(join(root, "detached")), true, "attached images are detached even when validation fails");
          assert.equal(existsSync(readFileSync(mounts, "utf8")), false, "the helper removes its empty mount directory");
        } else {
          assert.equal(existsSync(mounts), false, "invalid images are never mounted");
        }
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    });
}

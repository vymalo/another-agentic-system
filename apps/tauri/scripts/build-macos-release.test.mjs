import assert from "node:assert/strict";
import { generateKeyPairSync } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { execFileSync, spawnSync } from "node:child_process";
import test from "node:test";
import { buildMacosRelease, signingCredentials } from "./build-macos-release.mjs";

function credentials() {
  const { privateKey } = generateKeyPairSync("ec", { namedCurve: "prime256v1" });
  return {
    APPLE_CERTIFICATE: Buffer.from([0x30, 0x01, 0x00]).toString("base64"),
    APPLE_CERTIFICATE_PASSWORD: "test-only-password",
    APPLE_SIGNING_IDENTITY: "Developer ID Application: Test (ABCDEFGHIJ)",
    APPLE_DEVELOPMENT_TEAM: "ABCDEFGHIJ",
    ASC_KEY_ID: "ABC123DEFG",
    ASC_ISSUER_ID: "12345678-1234-1234-1234-123456789abc",
    ASC_API_KEY_BASE64: Buffer.from(privateKey.export({ type: "pkcs8", format: "pem" })).toString("base64"),
  };
}

test("every credential is mandatory; no unsigned release fallback", () => {
  for (const name of Object.keys(credentials())) {
    const env = credentials();
    delete env[name];
    assert.throws(() => signingCredentials(env), new RegExp(name));
  }
});

test("rejects invalid base64, wrong signing team, and non-private API key without echoing secrets", () => {
  for (const [name, value] of [
    ["APPLE_CERTIFICATE", "private-not-base64!"],
    ["ASC_API_KEY_BASE64", Buffer.from("private-not-a-key").toString("base64")],
    ["APPLE_SIGNING_IDENTITY", "Apple Distribution: Test (ABCDEFGHIJ)"],
    ["APPLE_DEVELOPMENT_TEAM", "ZYXWVUTSRQ"],
  ]) {
    const env = { ...credentials(), [name]: value };
    assert.throws(() => signingCredentials(env), (error) => !error.message.includes(value));
  }
});

test("API key is a private temporary file cleaned after a failed build", () => {
  let keyPath;
  const env = credentials();
  assert.throws(() => buildMacosRelease(env, (command, args, options) => {
    assert.equal(command, "pnpm");
    keyPath = options.env.APPLE_API_KEY_PATH;
    assert.equal(statSync(keyPath).mode & 0o777, 0o600);
    assert.match(readFileSync(keyPath, "utf8"), /BEGIN PRIVATE KEY/);
    assert.equal(options.env.APPLE_API_KEY, env.ASC_KEY_ID);
    assert.equal(options.env.APPLE_API_ISSUER, env.ASC_ISSUER_ID);
    assert.equal(options.env.APPLE_SIGNING_IDENTITY, env.APPLE_SIGNING_IDENTITY);
    return { status: 1 };
  }), /macOS release build failed/);
  assert.equal(existsSync(keyPath), false);
});

test("successful builds also remove the private API key", () => {
  let keyPath;
  buildMacosRelease(credentials(), (_command, _args, options) => {
    keyPath = options.env.APPLE_API_KEY_PATH;
    assert.equal(existsSync(keyPath), true);
    return { status: 0 };
  });
  assert.equal(existsSync(keyPath), false);
});

test("invalid certificate base64 fails preflight without invoking any signing subprocess", () => {
  let invoked = false;
  assert.throws(() => buildMacosRelease({ ...credentials(), APPLE_CERTIFICATE: "invalid!" }, () => {
    invoked = true;
  }), /APPLE_CERTIFICATE must contain base64/);
  assert.equal(invoked, false);
});

test("real macOS strict verification rejects an unsealed bundle and later resource tampering", { skip: process.platform !== "darwin" }, () => {
  const directory = mkdtempSync(join(tmpdir(), "another-agentic-signature-test-"));
  try {
    const app = join(directory, "signature-test.app");
    mkdirSync(join(app, "Contents", "MacOS"), { recursive: true });
    mkdirSync(join(app, "Contents", "Resources"));
    copyFileSync("/usr/bin/true", join(app, "Contents", "MacOS", "signature-test"));
    writeFileSync(join(app, "Contents", "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleIdentifier</key><string>com.vymalo.signature-test</string>
<key>CFBundleExecutable</key><string>signature-test</string><key>CFBundlePackageType</key><string>APPL</string></dict></plist>`);
    const resource = join(app, "Contents", "Resources", "fixture.txt");
    writeFileSync(resource, "original");
    const verify = () => spawnSync("codesign", ["--verify", "--deep", "--strict", app], { encoding: "utf8" });
    assert.notEqual(verify().status, 0, "binary signing alone does not seal its .app resources");
    const sign = spawnSync("codesign", ["--force", "--sign", "-", app], { encoding: "utf8" });
    assert.equal(sign.status, 0, sign.stderr);
    assert.equal(verify().status, 0, "complete ad-hoc bundle signing creates a valid resource envelope");
    writeFileSync(resource, "tampered");
    assert.notEqual(verify().status, 0, "the release gate rejects a modified sealed resource");
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

for (const invalid of [false, true]) {
  test(`iOS API key setup ${invalid ? "rejects an invalid private key and cleans up" : "creates a private file and always cleans up"}`, () => {
    const workflow = new URL("../../../.github/workflows/mobile-release.yml", import.meta.url);
    const steps = JSON.parse(execFileSync("ruby", ["-ryaml", "-rjson", "-e", "puts JSON.generate(YAML.load_file(ARGV[0])['jobs']['ios']['steps'])", workflow.pathname], { encoding: "utf8" }));
    const setup = steps.find((step) => step.name === "Configure App Store Connect automatic signing");
    const cleanup = steps.find((step) => step.name === "Remove the temporary Apple API key");
    assert.equal(cleanup.if, "always()");
    const directory = mkdtempSync(join(tmpdir(), "another-agentic-ios-key-test-"));
    try {
      const envFile = join(directory, "github-env");
      writeFileSync(envFile, "");
      const env = { ...process.env, ...credentials(), RUNNER_TEMP: directory, GITHUB_ENV: envFile };
      if (invalid) env.ASC_API_KEY_BASE64 = "invalid!";
      const result = spawnSync("bash", ["-c", setup.run], { env, encoding: "utf8" });
      const key = join(directory, `AuthKey_${env.ASC_KEY_ID}.p8`);
      if (invalid) {
        assert.notEqual(result.status, 0);
        assert.equal(existsSync(key), false);
      } else {
        assert.equal(result.status, 0, result.stderr);
        assert.equal(statSync(key).mode & 0o777, 0o600);
        env.APPLE_API_KEY_PATH = key;
        const cleaned = spawnSync("bash", ["-c", cleanup.run], { env, encoding: "utf8" });
        assert.equal(cleaned.status, 0, cleaned.stderr);
        assert.equal(existsSync(key), false);
      }
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
}

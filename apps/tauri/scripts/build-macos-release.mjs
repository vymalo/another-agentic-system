import { createPrivateKey } from "node:crypto";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { pathToFileURL } from "node:url";

function decode(name, value) {
  const compact = value.replace(/\s/g, "");
  if (!/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(compact)) {
    throw new Error(`${name} must contain base64 file contents`);
  }
  const bytes = Buffer.from(compact, "base64");
  if (!bytes.length || bytes.toString("base64") !== compact) {
    throw new Error(`${name} must contain base64 file contents`);
  }
  return bytes;
}

export function signingCredentials(env) {
  const names = ["APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_SIGNING_IDENTITY",
    "APPLE_DEVELOPMENT_TEAM", "ASC_KEY_ID", "ASC_ISSUER_ID", "ASC_API_KEY_BASE64"];
  for (const name of names) {
    if (!env[name]?.trim()) throw new Error(`Missing required macOS signing credential: ${name}`);
  }
  if (!/^[A-Z0-9]{10}$/.test(env.APPLE_DEVELOPMENT_TEAM)
      || !env.APPLE_SIGNING_IDENTITY.startsWith("Developer ID Application: ")
      || !env.APPLE_SIGNING_IDENTITY.endsWith(` (${env.APPLE_DEVELOPMENT_TEAM})`)) {
    throw new Error("APPLE_SIGNING_IDENTITY must be a Developer ID Application identity for APPLE_DEVELOPMENT_TEAM");
  }
  if (!/^[A-Z0-9]{10}$/.test(env.ASC_KEY_ID)
      || !/^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(env.ASC_ISSUER_ID)) {
    throw new Error("Invalid ASC_KEY_ID or ASC_ISSUER_ID format");
  }
  const certificate = decode("APPLE_CERTIFICATE", env.APPLE_CERTIFICATE);
  const apiKey = decode("ASC_API_KEY_BASE64", env.ASC_API_KEY_BASE64);
  try {
    const key = createPrivateKey(apiKey);
    if (key.asymmetricKeyType !== "ec" || key.asymmetricKeyDetails.namedCurve !== "prime256v1") {
      throw new Error("wrong key type");
    }
  } catch {
    throw new Error("ASC_API_KEY_BASE64 must encode an App Store Connect P-256 private .p8 key");
  }
  return { certificate, apiKey };
}

export function buildMacosRelease(env, run = spawnSync) {
  const { certificate, apiKey } = signingCredentials(env);
  const directory = mkdtempSync(join(tmpdir(), "another-agentic-apple-"));
  chmodSync(directory, 0o700);
  const keyPath = join(directory, `AuthKey_${env.ASC_KEY_ID}.p8`);
  try {
    writeFileSync(keyPath, apiKey, { mode: 0o600 });
    // Let Apple's security import (via Tauri) validate the .p12/password. An
    // OpenSSL precheck can reject valid Apple exports using legacy ciphers.
    // CLI 2.12.1 imports APPLE_CERTIFICATE into a temporary keychain, signs the whole
    // app + DMG, notarizes/staples the app, and deletes the keychain on normal return.
    // https://github.com/tauri-apps/tauri/tree/tauri-cli-v2.12.1/crates/tauri-macos-sign
    const result = run("pnpm", ["tauri", "build", "--bundles", "dmg", "--config", "src-tauri/tauri.deployment.json"], {
      env: { ...env, APPLE_CERTIFICATE: certificate.toString("base64"),
        APPLE_API_KEY: env.ASC_KEY_ID, APPLE_API_ISSUER: env.ASC_ISSUER_ID, APPLE_API_KEY_PATH: keyPath },
      stdio: "inherit",
    });
    if (result.status !== 0) throw new Error("macOS release build failed");
  } finally {
    // This fresh private directory is exclusively owned by this invocation.
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (process.argv.includes("--preflight")) signingCredentials(process.env);
    else buildMacosRelease(process.env);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

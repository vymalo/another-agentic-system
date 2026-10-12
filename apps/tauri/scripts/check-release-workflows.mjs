import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { strict as assert } from "node:assert";
import { execFileSync } from "node:child_process";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = new URL("../../../", import.meta.url);
const read = (file) => readFileSync(new URL(file, root), "utf8");
const parseYaml = (file) => JSON.parse(execFileSync("ruby", ["-ryaml", "-rjson", "-e", "puts JSON.generate(YAML.load_file(ARGV.fetch(0)))", file], { encoding: "utf8" }));

const releasePlease = read(".github/workflows/release-please.yml");
const desktopRelease = read(".github/workflows/desktop-release.yml");
const mobileRelease = read(".github/workflows/mobile-release.yml");
const desktopCi = read(".github/workflows/desktop.yml");
const smoke = read(".github/workflows/homebrew-smoke.yml");
const smokeScript = read("apps/tauri/scripts/homebrew-smoke.sh");
const tapUpdate = read("apps/tauri/scripts/update-homebrew-tap.mjs");
const config = JSON.parse(read("release-please-config.json"));
const manifest = JSON.parse(read(".release-please-manifest.json"));
const tauri = JSON.parse(read("apps/tauri/src-tauri/tauri.conf.json"));
const desktopPackage = JSON.parse(read("apps/tauri/package.json"));
const releasePleaseYaml = parseYaml(fileURLToPath(new URL("../../../.github/workflows/release-please.yml", import.meta.url)));
const desktopReleaseYaml = parseYaml(fileURLToPath(new URL("../../../.github/workflows/desktop-release.yml", import.meta.url)));
const mobileReleaseYaml = parseYaml(fileURLToPath(new URL("../../../.github/workflows/mobile-release.yml", import.meta.url)));
const desktopCiYaml = parseYaml(fileURLToPath(new URL("../../../.github/workflows/desktop.yml", import.meta.url)));

assert.match(releasePlease, /actions\/create-github-app-token@/);
assert.match(releasePlease, /permission-contents: write/);
assert.match(releasePlease, /permission-pull-requests: write/);
assert.match(releasePlease, /googleapis\/release-please-action@/);
assert.doesNotMatch(releasePlease, /uses: [^\n]+@v\d/);
assert.deepEqual(releasePleaseYaml.true.push.branches, ["main"]);
assert.equal(releasePleaseYaml.permissions.contents, "read");
assert.equal(releasePleaseYaml.jobs["release-please"].steps.at(-1).with["manifest-file"], ".release-please-manifest.json");
assert.equal(config["always-update"], true);
assert.match(config["pull-request-header"], /## 4\. Verification/);
assert.match(config["pull-request-header"], /## 7\. AI Usage Declaration/);
assert.match(config["pull-request-header"], /\[x\]/);
assert.match(config["pull-request-header"], /https:\/\/github\.com\/vymalo\/another-agentic-system\/commits\/main\//);
assert.equal(config.packages["."]["include-v-in-tag"], true);
assert.equal(config.packages["."]["include-component-in-tag"], false);
assert.equal(
  config.packages["."]["extra-files"].some((file) => file.path === "apps/tauri/src-tauri/Cargo.lock"),
  true,
);

assert.deepEqual(desktopReleaseYaml.true.release.types, ["published"]);
assert.match(desktopRelease, /macOS Apple Silicon/);
assert.match(desktopRelease, /macOS Intel/);
assert.match(desktopRelease, /Linux x86_64/);
assert.doesNotMatch(desktopRelease, /name: Windows/);
assert.match(desktopRelease, /gh release view/);
assert.match(desktopRelease, /SHA256SUMS/);
assert.match(desktopRelease, /gh release upload/);
assert.match(desktopRelease, /GH_REPO: \$\{\{ github\.repository \}\}/);
assert.doesNotMatch(desktopRelease, /gh release upload[^\n]*\*/);
assert.match(desktopRelease, /update-homebrew-tap/);
assert.match(desktopRelease, /repositories: another-agentic-system,homebrew-tap/);
assert.match(desktopRelease, /needs: publish/);
assert.match(desktopRelease, /update-homebrew-tap\.mjs/);
assert.match(tapUpdate, /--force-with-lease/);
assert.match(tapUpdate, /"--head", branch/);
assert.doesNotMatch(tapUpdate, /"--head", "vymalo:/);
assert.equal(desktopReleaseYaml.jobs["update-homebrew-tap"].concurrency["cancel-in-progress"], false);
assert.equal(desktopReleaseYaml.jobs["update-homebrew-tap"].concurrency.group, "homebrew-tap-another-agentic");
assert.equal(desktopReleaseYaml.jobs["update-homebrew-tap"].concurrency.queue, "max");
assert.doesNotMatch(desktopRelease, /uses: [^\n]+@v\d/);
assert.match(desktopRelease, /github\.event\.release\.tag_name/);
assert.equal(desktopReleaseYaml.jobs.bundle.needs, "release-preflight");
assert.match(desktopReleaseYaml.jobs["release-preflight"].steps[0].run, /AUTHOR/);
assert.match(desktopReleaseYaml.jobs["release-preflight"].steps[0].run, /APP_SLUG/);
assert.equal(desktopReleaseYaml.permissions.contents, "read");
assert.deepEqual(desktopReleaseYaml.jobs.publish.needs, "bundle");
const bundleSteps = desktopReleaseYaml.jobs.bundle.steps;
const signingStep = bundleSteps.find((step) => step.name === "Sign and notarize the macOS release assets");
assert.equal(signingStep.if, "runner.os == 'macOS'");
assert.equal(signingStep.env.APPLE_CERTIFICATE, "${{ secrets.APPLE_CERTIFICATE }}");
assert.equal(signingStep.env.APPLE_SIGNING_IDENTITY, "${{ vars.APPLE_SIGNING_IDENTITY }}");
assert.equal(signingStep.env.ASC_API_KEY_BASE64, "${{ secrets.ASC_API_KEY_BASE64 }}");
const verificationIndex = bundleSteps.findIndex((step) => step.name === "Verify the signed notarized app inside the macOS disk image");
const uploadIndex = bundleSteps.findIndex((step) => step.uses?.startsWith("actions/upload-artifact@"));
assert.ok(verificationIndex > bundleSteps.indexOf(signingStep) && verificationIndex < uploadIndex);
assert.equal(bundleSteps[verificationIndex].if, "runner.os == 'macOS'");
assert.match(bundleSteps[verificationIndex].run, /bash apps\/tauri\/scripts\/verify-macos-release\.sh apps\/tauri\/src-tauri\/target\/release\/bundle/);
assert.equal(bundleSteps.find((step) => step.name === "Bundle the release assets").if, "runner.os == 'Linux'");
const linuxX64Bundle = desktopReleaseYaml.jobs.bundle.strategy.matrix.include.find(({ name }) => name === "Linux x86_64");
assert.equal(linuxX64Bundle.bundles, "deb,appimage");
assert.match(linuxX64Bundle.path, /bundle\/deb\/\*\.deb/);
assert.match(linuxX64Bundle.path, /bundle\/appimage\/\*\.AppImage/);
assert.match(desktopRelease, /another-agentic-linux-x64\/deb\/\*\.deb/);
assert.match(desktopRelease, /another-agentic-linux-x64\/appimage\/\*\.AppImage/);
assert.equal(desktopReleaseYaml.jobs["verify-homebrew"].needs, "publish");
assert.equal(desktopReleaseYaml.jobs["verify-homebrew"].uses, "./.github/workflows/homebrew-smoke.yml");
assert.match(smoke, /macos-26-intel/);
assert.match(smoke, /ubuntu-24.04-arm/);
assert.match(smokeScript, /tap=vymalo\/release-smoke/);
assert.match(smokeScript, /brew install --formula/);
assert.match(smokeScript, /brew install --cask/);
assert.doesNotMatch(smokeScript, /--no-quarantine/);
assert.deepEqual(desktopReleaseYaml.jobs["update-homebrew-tap"].needs, ["publish", "verify-homebrew"]);
assert.deepEqual(Object.keys(desktopReleaseYaml.jobs.publish.outputs).sort(), ["linux_arm64_sha", "linux_x64_sha", "macos_arm64_sha", "macos_x64_sha", "version"]);

assert.deepEqual(mobileReleaseYaml.true.release.types, ["published"]);
assert.match(mobileRelease, /Android AAB and Play internal track/);
assert.match(mobileRelease, /iOS IPA and TestFlight/);
assert.match(mobileRelease, /tauri android build --ci --aab --target aarch64/);
assert.match(mobileRelease, /tauri ios build --ci --export-method app-store-connect/);
assert.match(mobileRelease, /tauri android build[^\n]+--config src-tauri\/tauri\.deployment\.json/);
assert.match(mobileRelease, /tauri ios build[^\n]+--config src-tauri\/tauri\.deployment\.json/);
assert.match(mobileRelease, /APPLE_DEVELOPMENT_TEAM/);
assert.match(mobileRelease, /GH_REPO: \$\{\{ github\.repository \}\}/);
assert.match(mobileRelease, /vymalo\/flutter-tools\/actions\/artifact-upload@842d5f4b6f8af183dcdb41ecb299968a95c0c2c6/);
assert.match(mobileRelease, /vymalo\/flutter-tools\/actions\/play-submit@842d5f4b6f8af183dcdb41ecb299968a95c0c2c6/);
assert.match(mobileRelease, /vymalo\/flutter-tools\/actions\/testflight-submit@842d5f4b6f8af183dcdb41ecb299968a95c0c2c6/);
assert.match(mobileRelease, /gh release view/);
assert.match(mobileRelease, /gh release upload/);
assert.doesNotMatch(mobileRelease, /uses: vymalo\/flutter-tools\/actions\/release-cut@/);
assert.doesNotMatch(mobileRelease, /uses: [^\n]+@v\d/);
assert.match(mobileRelease, /github\.event\.release\.tag_name/);
assert.equal(mobileReleaseYaml.permissions.contents, "read");
assert.match(mobileReleaseYaml.jobs["release-preflight"].steps[0].run, /\^v\[0-9\]\+\\\.\[0-9\]\+\\\.\[0-9\]\+\$/);
assert.match(mobileReleaseYaml.jobs["release-preflight"].steps[0].run, /gh release view/);
assert.match(mobileReleaseYaml.jobs["release-preflight"].steps[0].run, /AUTHOR/);
assert.match(mobileReleaseYaml.jobs["release-preflight"].steps[0].run, /APP_SLUG/);
assert.equal(mobileReleaseYaml.jobs.android.needs, "release-preflight");
assert.equal(mobileReleaseYaml.jobs.ios.needs, "release-preflight");
assert.equal(mobileReleaseYaml.jobs.ios["runs-on"], "macos-26");
assert.match(mobileRelease, /xcrun --sdk iphoneos --show-sdk-version/);
assert.equal(mobileReleaseYaml.jobs.ios.steps.find((step) => step.name?.startsWith("Submit to")).with["project-dir"], "apps/tauri");
for (const [job, clientId] of [["android", "another-agentic-android"], ["ios", "another-agentic-ios"]]) {
  const buildWeb = mobileReleaseYaml.jobs[job].steps.find((step) => step.name === "Build the web for the app");
  assert.equal(buildWeb.env.AGENTIC_SIGN_IN, "claimed");
  assert.equal(buildWeb.env.AGENTIC_CLIENT_ID, clientId);
  const submission = mobileReleaseYaml.jobs[job].steps.find((step) => step.name?.startsWith("Submit to"));
  assert.equal(submission.if, undefined);
}

assert.deepEqual(desktopCiYaml.true.pull_request.paths.includes("apps/tauri/**"), true);
assert.equal(desktopCiYaml.permissions.contents, "read");
assert.doesNotMatch(desktopCi, /uses: [^\n]+@v\d/);

assert.equal(manifest["."], tauri.version);
assert.equal(manifest["."], desktopPackage.version);
assert.match(read("apps/tauri/src-tauri/Cargo.toml"), new RegExp(`version = "${manifest["."]}" # x-release-please-version`));

const tapDirectory = mkdtempSync(join(tmpdir(), "another-agentic-homebrew-"));
try {
  const sha = "a".repeat(64);
  execFileSync(process.execPath, [
    fileURLToPath(new URL("write-homebrew-tap.mjs", import.meta.url)),
    tapDirectory,
    "0.1.2",
    sha,
    sha.replaceAll("a", "b"),
    sha.replaceAll("a", "c"),
    sha.replaceAll("a", "d"),
  ]);
  const cask = readFileSync(join(tapDirectory, "Casks", "another-agentic.rb"), "utf8");
  const formula = readFileSync(join(tapDirectory, "Formula", "another-agentic.rb"), "utf8");
  assert.match(cask, /on_arm/);
  assert.match(cask, /on_intel/);
  assert.match(cask, /another-agentic_0\.1\.2_macos_arm64\.dmg/);
  assert.match(formula, /another-agentic_0\.1\.2_linux_x64\.AppImage/);
  assert.match(formula, /another-agentic_0\.1\.2_linux_arm64\.AppImage/);
  assert.match(formula, /depends_on :linux/);
  assert.match(formula, /on_intel/);
  assert.match(formula, /on_arm/);
  assert.match(formula, /using: :nounzip/);
  assert.match(formula, /bin\.install/);
  assert.match(formula, /chmod 0755, bin\/"another-agentic"/);
  execFileSync("ruby", ["-c", join(tapDirectory, "Casks", "another-agentic.rb")]);
  execFileSync("ruby", ["-c", join(tapDirectory, "Formula", "another-agentic.rb")]);
} finally {
  rmSync(tapDirectory, { recursive: true, force: true });
}

execFileSync(process.execPath, ["--test", fileURLToPath(new URL("update-homebrew-tap.test.mjs", import.meta.url))], { stdio: "inherit" });
execFileSync(process.execPath, ["--test", fileURLToPath(new URL("build-macos-release.test.mjs", import.meta.url))], { stdio: "inherit" });
execFileSync(process.execPath, ["--test", fileURLToPath(new URL("verify-macos-release.test.mjs", import.meta.url))], { stdio: "inherit" });
console.log("desktop and mobile release workflow structure and tap lifecycle are valid");

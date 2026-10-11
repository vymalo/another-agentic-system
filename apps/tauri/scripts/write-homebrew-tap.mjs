import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [tapDirectory, version, macosArm64Sha, macosX64Sha, linuxX64Sha, linuxArm64Sha] = process.argv.slice(2);

if (![tapDirectory, version, macosArm64Sha, macosX64Sha, linuxX64Sha, linuxArm64Sha].every(Boolean)) {
  throw new Error("usage: write-homebrew-tap.mjs <tap-dir> <version> <macos-arm64-sha> <macos-x64-sha> <linux-x64-sha> <linux-arm64-sha>");
}

if (!/^\d+\.\d+\.\d+$/.test(version) || ![macosArm64Sha, macosX64Sha, linuxX64Sha, linuxArm64Sha].every((sha) => /^[a-f0-9]{64}$/.test(sha))) {
  throw new Error("version or SHA-256 input is invalid");
}

const release = `https://github.com/vymalo/another-agentic-system/releases/download/v${version}`;
mkdirSync(join(tapDirectory, "Casks"), { recursive: true });
mkdirSync(join(tapDirectory, "Formula"), { recursive: true });

writeFileSync(
  join(tapDirectory, "Casks", "another-agentic.rb"),
  `cask "another-agentic" do
  version "${version}"

  on_arm do
    url "${release}/another-agentic_${version}_macos_arm64.dmg"
    sha256 "${macosArm64Sha}"
  end

  on_intel do
    url "${release}/another-agentic_${version}_macos_x64.dmg"
    sha256 "${macosX64Sha}"
  end

  name "another-agentic"
  desc "Desktop chat for another-agentic-system"
  homepage "https://github.com/vymalo/another-agentic-system"

  app "another-agentic.app"
end
`,
);

writeFileSync(
  join(tapDirectory, "Formula", "another-agentic.rb"),
  `class AnotherAgentic < Formula
  desc "Desktop chat for another-agentic-system"
  homepage "https://github.com/vymalo/another-agentic-system"
  depends_on :linux
  on_intel do
    url "${release}/another-agentic_${version}_linux_x64.AppImage", using: :nounzip
    sha256 "${linuxX64Sha}"
  end

  on_arm do
    url "${release}/another-agentic_${version}_linux_arm64.AppImage", using: :nounzip
    sha256 "${linuxArm64Sha}"
  end
  license any_of: ["MIT", "Apache-2.0"]

  def install
    bin.install Dir["another-agentic_${version}_linux_*.AppImage"].first => "another-agentic"
  end
end
`,
);

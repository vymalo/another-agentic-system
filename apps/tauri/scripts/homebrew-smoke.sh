#!/usr/bin/env bash
# Proves installation/checksums and executable presence, not GUI startup,
# Gatekeeper acceptance, notarization, or authentication against a live service.
set -euo pipefail

if [[ "$(uname -s)" == Linux ]]; then
  test -x /home/linuxbrew/.linuxbrew/bin/brew
  eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"
  test "$(command -v brew)" = /home/linuxbrew/.linuxbrew/bin/brew
fi
brew --version

# A tap's repository prefix is implicit: vymalo/homebrew-smoke would be
# canonicalized by Homebrew to vymalo/smoke, not the name passed to install.
tap=vymalo/release-smoke
brew tap-new --no-git "$tap"
node apps/tauri/scripts/write-homebrew-tap.mjs "$(brew --repository "$tap")" \
  "$VERSION" "$MACOS_ARM64_SHA" "$MACOS_X64_SHA" "$LINUX_X64_SHA" "$LINUX_ARM64_SHA"

if [[ "$(uname -s)" == Darwin ]]; then
  # Fully qualifying the Cask grants package-specific trust on Homebrew 7.
  # Do not bypass quarantine or mark the entire tap trusted.
  brew install --cask "$tap/another-agentic"
  test -d /Applications/another-agentic.app
  test -x /Applications/another-agentic.app/Contents/MacOS/another-agentic
else
  brew install --formula "$tap/another-agentic"
  test -x "$(brew --prefix)/bin/another-agentic"
fi

#!/bin/bash
# Check the app actually delivered inside the disk image, not just the build tree.
set -euo pipefail
bundle_root="${1:?usage: verify-macos-release.sh bundle-directory}"
expected_team="${APPLE_DEVELOPMENT_TEAM:?APPLE_DEVELOPMENT_TEAM is required}"
shopt -s nullglob
images=("$bundle_root"/dmg/*.dmg)
test "${#images[@]}" -eq 1
mount_dir="$(mktemp -d "${TMPDIR:-/tmp}/another-agentic-verify.XXXXXX")"
mounted=false
cleanup() {
  if "$mounted"; then hdiutil detach "$mount_dir"; fi
  rmdir "$mount_dir"
}
trap cleanup EXIT
hdiutil verify "${images[0]}"
hdiutil attach -readonly -nobrowse -mountpoint "$mount_dir" "${images[0]}"
mounted=true
apps=("$mount_dir"/*.app)
test "${#apps[@]}" -eq 1
app="${apps[0]}"
codesign --verify --deep --strict --verbose=2 "$app"
signature="$(codesign --display --verbose=4 "$app" 2>&1)"
grep -Fx "TeamIdentifier=$expected_team" <<< "$signature"
grep -F 'Authority=Developer ID Application:' <<< "$signature"
test "$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Contents/Info.plist")" = 'com.vymalo.agentic'
spctl --assess --type execute --verbose=2 "$app"
# Tauri staples the notarized .app before constructing the DMG. The DMG itself
# is signed but not independently submitted to notarization by CLI 2.12.1.
xcrun stapler validate "$app"

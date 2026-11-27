#!/bin/sh
# The version files release-please moves (ADR 0106). release-please (release-please-config.json) writes the version of
# `.release-please-manifest.json` and of every file in its `extra-files`: the Tauri app's three (Cargo.toml by its
# `# x-release-please-version` marker, package.json and tauri.conf.json by `$.version`) and the chart's Chart.yaml (`version` and
# `appVersion`, both marked). This script fails when a file is not listed or a marker is missing; it prints every version it found.
# With an argument, every one of them must also be that version: release.yml passes the tag without its `v`, and the release
# pull request (whose manifest release-please has just moved) passes the manifest's.
#
#   sh tools/check-release-version.sh            (from the repository root: the files and their markers)
#   sh tools/check-release-version.sh 0.9.0      (one version everywhere, and that one)
#
# Without an argument the versions may differ: the chart said 0.2.0 when the manifest said 0.8.0, and the first release after its
# markers were added (ADR 0106) is the one that makes them equal. Needs only sh, grep and sed.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
want=${1:-}
fail=0
bad() { echo "FAIL $1"; fail=1; }
semver='[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*'

if [ -n "$want" ] && ! printf '%s\n' "$want" | grep -qx "$semver"; then
  echo "usage: $0 [X.Y.Z]: '$want' is not a version" >&2
  exit 2
fi

config="$root/release-please-config.json"
manifest=$(sed -n 's/^ *"\." *: *"\([^"]*\)".*$/\1/p' "$root/.release-please-manifest.json")
[ -n "$manifest" ] || bad ".release-please-manifest.json has no version for the package '.'"

for f in apps/tauri/src-tauri/Cargo.toml apps/tauri/package.json apps/tauri/src-tauri/tauri.conf.json deploy/chart/Chart.yaml; do
  grep -q "\"path\": \"$f\"" "$config" || bad "release-please-config.json does not list $f in extra-files"
done

# The Tauri app.
cargo=$(sed -n "s/^version = \"\($semver\)\" # x-release-please-version\$/\1/p" "$root/apps/tauri/src-tauri/Cargo.toml")
[ -n "$cargo" ] || bad "apps/tauri/src-tauri/Cargo.toml has no 'version = \"X.Y.Z\" # x-release-please-version' line"
pkg=$(sed -n "s/^  \"version\": \"\($semver\)\",\$/\1/p" "$root/apps/tauri/package.json")
[ -n "$pkg" ] || bad "apps/tauri/package.json has no top-level \"version\""
conf=$(sed -n "s/^  \"version\": \"\($semver\)\",\$/\1/p" "$root/apps/tauri/src-tauri/tauri.conf.json")
[ -n "$conf" ] || bad "apps/tauri/src-tauri/tauri.conf.json has no top-level \"version\""

# The chart: `version` and `appVersion`, both marked, nothing else marked.
chart="$root/deploy/chart/Chart.yaml"
marked=$(grep -c 'x-release-please-version' "$chart" || true)
[ "$marked" -eq 2 ] || bad "deploy/chart/Chart.yaml has $marked lines marked x-release-please-version, want 2 (version, appVersion)"
cv=$(sed -n "s/^version: \($semver\) # x-release-please-version\$/\1/p" "$chart")
ca=$(sed -n "s/^appVersion: \"\($semver\)\" # x-release-please-version\$/\1/p" "$chart")
[ -n "$cv" ] || bad "deploy/chart/Chart.yaml has no 'version: X.Y.Z # x-release-please-version' line"
[ -n "$ca" ] || bad "deploy/chart/Chart.yaml has no 'appVersion: \"X.Y.Z\" # x-release-please-version' line"

versions="manifest=$manifest tauri.cargo=$cargo tauri.package=$pkg tauri.conf=$conf chart.version=$cv chart.appVersion=$ca"
if [ -n "$want" ]; then
  for kv in $versions; do
    [ "${kv#*=}" = "$want" ] || bad "${kv%%=*} is '${kv#*=}', not '$want'"
  done
fi

if [ "$fail" -ne 0 ]; then exit 1; fi
if [ -n "$want" ]; then echo "one version: $want ($versions)"; else echo "every version file is listed and marked ($versions)"; fi

#!/bin/sh
# Compares two sets of stills with ImageMagick (the `screens` target of web/Dockerfile installs it; never run it on a host).
#   sh scripts/screens-compare.sh [--list-failing] <generated-dir> <committed-dir>
# --list-failing: print only the names of the stills that fail the bound, one per line, on stdout (every other line, the
# per-still detail with its pixel count and percentage, goes to stderr) and exit 0 whatever fails; update-screens.yml uses it to
# commit only those. Without it, the output and the exit codes are what `check` has always had.
# The bound (ANR-Q133): a still passes when at most SCREENS_LIMIT_PPM (0.1 % by default) of its pixels differ by more than
# SCREENS_FUZZ_Q/255 (8 by default) (`compare -metric AE -fuzz`). Both are configuration, documented as a default below.
# A still that is missing on either side, or whose dimensions differ, fails; no still is skipped. Exit 1 when any still fails.
# A still that passes but spends more than half of the bound is reported as `near the limit` and counted in the summary, so a
# green run names the stills one benign change away from red (ANR-1351). It changes no exit code.
# Why a bound and not equality: Chromium's sub-pixel rasterisation differs between processes, so two draws of one commit in
# one image differ by a few pixels (web/README.md, "screens-docker.sh").
set -eu
list=0
if [ "${1:-}" = "--list-failing" ]; then list=1; shift; fi
gen="${1:?generated dir}"; com="${2:?committed dir}"
# The bound is a policy number, so it is configuration with a documented default, not a constant (the owner's rule of
# 2026-11-28: a threshold is a setting behind configuration, never a number to ask for). A deployment or a job overrides it:
#   SCREENS_LIMIT_PPM  parts per million of a still's pixels that may differ by more than SCREENS_FUZZ_Q/255
#                      (default 1000: 0.1 %, the owner's answer to ANR-Q149 on 2026-11-27)
#   SCREENS_FUZZ_Q     how far a channel may move before the pixel counts as different, 0..255 (default 8)
LIMIT_PPM="${SCREENS_LIMIT_PPM:-1000}"
FUZZ_Q="${SCREENS_FUZZ_Q:-8}"
case "$LIMIT_PPM" in ''|*[!0-9]*) echo "screens-compare: SCREENS_LIMIT_PPM must be a whole number of parts per million" >&2; exit 2;; esac
case "$FUZZ_Q" in ''|*[!0-9]*) echo "screens-compare: SCREENS_FUZZ_Q must be a whole number from 0 to 255" >&2; exit 2;; esac
if [ "$LIMIT_PPM" -lt 1 ] || [ "$FUZZ_Q" -gt 255 ]; then
  echo "screens-compare: SCREENS_LIMIT_PPM must be at least 1 and SCREENS_FUZZ_Q at most 255" >&2; exit 2
fi
NEAR_PPM=$((LIMIT_PPM / 2))   # more than half of the bound: a still this close is one benign change from red
limit_pct="$(awk "BEGIN { printf \"%.4f\", $LIMIT_PPM / 10000 }")"
fuzz_pct="$(awk "BEGIN { printf \"%.4f\", $FUZZ_Q / 255 * 100 }")%"
total=0; failed=0; near=0
say() { if [ "$list" -eq 1 ]; then echo "$@" >&2; else echo "$@"; fi; }
fail() { failed=$((failed + 1)); [ "$list" -eq 0 ] || echo "$1"; } # name
check() { # name
  n="$1"; total=$((total + 1))
  if [ ! -f "$gen/$n" ]; then say "FAIL not generated: $n"; fail "$n"; return; fi
  if [ ! -f "$com/$n" ]; then say "FAIL new, not committed: $n"; fail "$n"; return; fi
  da="$(identify -format '%wx%h' "$gen/$n[0]")"; db="$(identify -format '%wx%h' "$com/$n[0]")"
  if [ "$da" != "$db" ]; then say "FAIL changed dimensions: $n (generated $da, committed $db)"; fail "$n"; return; fi
  w="${da%x*}"; h="${da#*x}"; px=$((w * h))
  # compare exits 1 when the images differ and 2 on an error; AE (pixels beyond the fuzz) goes to stderr
  set +e; ae="$(compare -metric AE -fuzz "$fuzz_pct" "$gen/$n" "$com/$n" null: 2>&1)"; rc=$?; set -e
  if [ "$rc" -gt 1 ]; then ae="$(echo "$ae" | tr '\n' ' ')"; say "FAIL compare error ($rc): $n: $ae"; fail "$n"; return; fi
  ae="$(echo "$ae" | awk '{ printf "%d", $1 }')"
  [ "$ae" -eq 0 ] && return
  pct="$(awk "BEGIN { printf \"%.4f\", $ae * 100 / $px }")"
  if [ $((ae * 1000000)) -gt $((px * LIMIT_PPM)) ]; then
    say "FAIL $n: $ae of $px pixels (${pct} %) differ by more than $FUZZ_Q/255; the limit is $limit_pct % of the pixels and $FUZZ_Q/255"; fail "$n"
  elif [ $((ae * 1000000)) -gt $((px * NEAR_PPM)) ]; then
    share="$(awk "BEGIN { printf \"%.1f\", $ae * 100000000 / ($px * $LIMIT_PPM) }")"
    near=$((near + 1))
    say "near the limit $n: $ae of $px pixels (${pct} %) differ by more than $FUZZ_Q/255, ${share} % of the bound — the next benign change on this still flips this job red"
  else
    say "ok (within the bound) $n: $ae of $px pixels (${pct} %) differ by more than $FUZZ_Q/255"
  fi
}
names="$(mktemp)"; trap 'rm -f "$names"' EXIT
{ ls "$gen"; ls "$com"; } | grep '\.png$' | sort -u >"$names"
while IFS= read -r n; do check "$n"; done <"$names"
say "$total compared: $failed failed (bound: at most $limit_pct % of a still's pixels differ by more than $FUZZ_Q/255)"
if [ "$near" -gt 0 ]; then
  say "$near of $total stills sit above half of the bound (named above): a green run does not mean the next one is green"
fi
[ "$list" -eq 1 ] || [ "$failed" -eq 0 ]

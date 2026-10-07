#!/usr/bin/env sh
# shellcheck disable=SC2154
# Shared by dev/kagent/up.sh and dev/kagent/check.sh (sourced, never run). The caller sets `upstream` (the path of dev/kagent/UPSTREAM) and
# `tmp` (a directory the charts are pulled into).

pin() { # pin KEY: the value of a `key=value` line of dev/kagent/UPSTREAM
  _v=$(sed -n "s/^$1=//p" "$upstream" | head -n 1)
  if [ -z "$_v" ]; then
    echo "$upstream has no line $1=" >&2
    exit 1
  fi
  printf '%s' "$_v"
}

# pull_chart REF VERSION KEY: pulls oci://REF at VERSION into $tmp, compares the digest with the pin KEY of UPSTREAM (a different one stops the
# caller: the chart moved, or the pin is wrong) and prints the path of the file.
pull_chart() {
  _out=$(helm pull "oci://$1" --version "$2" --destination "$tmp" 2>&1) || { echo "helm pull $1 $2 failed: $_out" >&2; exit 1; }
  _digest=$(printf '%s\n' "$_out" | sed -n 's/^Digest: //p' | head -n 1)
  _want=$(pin "$3")
  if [ "$_digest" != "$_want" ]; then
    echo "oci://$1:$2 has digest '$_digest', dev/kagent/UPSTREAM pins $_want ($3): the chart moved, or the pin is wrong" >&2
    exit 1
  fi
  echo "ok   oci://$1:$2 is $_digest" >&2
  printf '%s/%s-%s.tgz' "$tmp" "${1##*/}" "$2"
}

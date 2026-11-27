#!/bin/sh
# This repository is public, and it names no private repository: neither of the operator's two private
# repositories nor the organisation that holds them, in any tracked file. A fact this repository needs from one
# of them (a wire shape, a header, a route) is written here in neutral words ("the deployment's ledger", "the
# owner's private decision log"), never by its repository, path, crate or decision number.
#
# The names are assembled from pieces so that this file does not match itself. The vendored skill folders and
# their lock file are copied from upstream as they are, and are left out.
#
# Usage (from the repo root): sh tools/check-private-names.sh
# Exits 1 and prints file:line:text for each match; also run in CI (.github/workflows/docs.yml).
set -eu

pattern="another-""backend|another-""context|sky""engpro"

if matches=$(git grep -nIiE "$pattern" -- . \
  ':(exclude).agents/skills' ':(exclude).claude' ':(exclude).goose' ':(exclude).kiro' \
  ':(exclude)skills-lock.json'); then
  printf '%s\n' "$matches" >&2
  count=$(printf '%s\n' "$matches" | wc -l | tr -d ' ')
  printf 'private names: %s line(s) name a private repository or its organisation; use neutral words\n' "$count" >&2
  exit 1
fi
echo "private names: none in any tracked file"

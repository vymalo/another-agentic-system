#!/bin/sh
# Which commit's image a release publishes for one image workflow (ADR 0106, .github/workflows/release.yml).
#
#   sh tools/release-source.sh <.github/workflows/orchestrator.yml|web.yml> <commit>
#
# Prints the full SHA of the newest commit at or before <commit> that changed a path of that workflow's `on.push.paths`: the commit
# whose run on main built and pushed `sha-<7>` for exactly the tree <commit> has under those paths. A release commit touches only
# the changelog and the version files, so its own commit has no image; the image of this commit is the release's. The paths are
# read from the workflow itself, so they can never drift from what main builds. Exit 2 for input it refuses, 1 when no commit
# changed those paths.
#
# Needs git and ruby (its YAML library; the release-contract check uses it too). Run from inside the repository.
set -eu

if [ "$#" -ne 2 ]; then
  echo "usage: $0 <workflow.yml> <commit>" >&2
  exit 2
fi
workflow=$1
commit=$2

[ -f "$workflow" ] || { echo "no such workflow: $workflow" >&2; exit 2; }
sha=$(git rev-parse --verify --quiet "$commit^{commit}") || { echo "not a commit: $commit" >&2; exit 2; }

# `on` is the YAML 1.1 boolean true to ruby's parser; take either key. One path per line; a negated path is refused (its meaning
# in a list of paths depends on order, which a pathspec cannot say).
paths=$(ruby -ryaml -e '
  w = YAML.load_file(ARGV.fetch(0))
  on = w["on"] || w[true] || {}
  push = on.is_a?(Hash) ? on["push"] : nil
  list = push.is_a?(Hash) ? push["paths"] : nil
  abort("no on.push.paths in #{ARGV[0]}") unless list.is_a?(Array) && !list.empty?
  list.each { |p| abort("a negated path (#{p}) is not supported") if p.to_s.start_with?("!"); puts p }
' "$workflow")

# Each path as a glob pathspec (GitHub's `**` and `*`), passed as separate arguments.
set --
while IFS= read -r p; do
  [ -n "$p" ] && set -- "$@" ":(glob)$p"
done <<EOF
$paths
EOF

source=$(git log -1 --format=%H "$sha" -- "$@")
[ -n "$source" ] || { echo "no commit at or before $sha changed the paths of $workflow" >&2; exit 1; }
printf '%s\n' "$source"

#!/usr/bin/env sh
# System-level test of a person's prompt library (ADR 0070, docs/api/prompts-v1.md): `/api/prompts` saves prompts with variables that
# are questions of the UI catalog, renders them on the server without sending anything, and shares them with roles under `prompts.share`.
#
#   dev/prompts-e2e.sh
#
# Start the `app` profile first (the coder image is about 2.9 GB, linux/amd64 only; this scenario does not use it, but the profile
# starts it):
#
#   docker compose --profile app up -d --build --wait
#
# It needs no agent: only the API. The people are users of dev/mock-oidc/users.json that no other scenario signs in as, and the roles of
# dev/orchestrator.yaml:
#   prompts@example.com            the role `prompt-sharer`: a user who also holds `prompts.share` (no other role does: `admin` does not
#                                  imply it)
#   prompts-team@example.com       `user` and `prompt-team`, a value of the roles claim that no `auth.roles` entry names: it grants
#                                  nothing, and it is what a prompt is shared with
#   prompts-outsider@example.com   `user`: no `prompts.share`, and not in the audience
#   admin@example.com              `admin`: reads no prompt that is not its own (ADR 0039)
# The names carry a letters-only run id, and the script deletes every prompt of the three first people before it starts and when it ends
# (the library is the person's own: a rerun on the same database starts clean). It speaks what the web speaks (docs/api/chat-api.yaml):
# `getConfig`, `listPrompts`, `createPrompt`, `getPrompt`, `updatePrompt`, `deletePrompt` and `renderPrompt`.
#
# It prints one ok or FAIL line per check and exits 1 if any failed:
#   * CONFIG: `GET /api/config` says `prompts` with the limit 100;
#   * SAVE: `review`, a prompt with a select and a scale, is created (201, an id `p-` and 12 characters, a revision, `own: true`); the
#     list has it as a summary (`variableCount: 2`, no `body`, no `variables`) and `getPrompt` has the whole;
#   * RENDER: `{focus: security, depth: 4}` gives exactly `Review this PR for Security at depth 4.` (the option's label, not its value),
#     and writes nothing (the revision and the list are the same); a scale answer of 6 is 422 `invalid_answer` (`question: depth`,
#     `rule: above_max`), answers in the wrong order are `unknown`, a `note` is `unexpected`;
#   * REFUSALS OF A SAVE: an undeclared reference (400 `undeclared_variable`), a declared variable nothing uses (`unused_variable`),
#     `{{ focus }}` with spaces (`bad_reference` with its `offset`), a question of the type `images` (`bad_variable`), the same name again
#     (409 `name_taken`), a body that is not JSON (400) and a media type that is not JSON (415);
#   * REVISIONS: an edit that names the revision it read is 200 and has a new one; a second edit with the old one is 409 `stale_revision`;
#   * SHARING: the owner shares with `prompt-team`; that person lists it (`scope=shared`: `own: false`, no `audience`), reads it, renders it to
#     the same text and gets 404 for an edit and a delete; **no answer they got holds the owner's e-mail**;
#   * NOBODY ELSE: a person outside the audience and the administrator do not list it and get 404 for the id, for a read and a render;
#   * THE PERMISSION: a person without `prompts.share` creates a private prompt (201), is 403 `forbidden` for one shared with `*` and for
#     an edit that adds an audience; sharing with `*` lets them list it, and unsharing (an empty audience) hides it at once;
#   * THE LIMIT: a person holds 100 prompts, the 101st is 409 `too_many_prompts`, and `held` says 100;
#   * DELETE: with a stale `revision` it is 409, then 204, then 404;
#   * THE LOG: no word of a name, title, description, body, answer or role of a prompt is in a line of the orchestrator's log (read with
#     `docker compose logs` when docker is there: otherwise this one check is skipped and says so).
# Exit status 0 when every check passed.
#
# Environment (defaults match compose.yaml on one machine):
#   BASE_URL        http://127.0.0.1:${EDGE_PORT:-8080}, the compose `edge`: oauth2-proxy in front of the API (ADR 0033)
#   OWNER_EMAIL     prompts@example.com, who holds `prompts.share`
#   TEAM_EMAIL      prompts-team@example.com, who has the role the prompt is shared with
#   OUTSIDER_EMAIL  prompts-outsider@example.com, who has neither
#   ADMIN_EMAIL     admin@example.com
#   ORCH_SERVICE    orchestrator, the compose service whose log is read
#
# Needs curl and jq. Run by CI in .github/workflows/coder-e2e.yml (no Docker where it was written). Its first CI run (43731759281) passed
# every step but one, `not as theirs`, where `field` read `false` as absent: `field` now drops only `null` and `bool_field` says `true` or `false`.
# shellcheck disable=SC2016 # the single-quoted jq programs hold jq variables ($id, $rev), never the shell's
set -eu

base=${BASE_URL:-http://127.0.0.1:${EDGE_PORT:-8080}}
base=${base%/}
owner_email=${OWNER_EMAIL:-prompts@example.com}
team_email=${TEAM_EMAIL:-prompts-team@example.com}
outsider_email=${OUTSIDER_EMAIL:-prompts-outsider@example.com}
admin_email=${ADMIN_EMAIL:-admin@example.com}
here=$(dirname "$0")
owner=$(sh "$here/auth-header.sh" "$owner_email")
team=$(sh "$here/auth-header.sh" "$team_email")
outsider=$(sh "$here/auth-header.sh" "$outsider_email")
admin=$(sh "$here/auth-header.sh" "$admin_email")
service=${ORCH_SERVICE:-orchestrator}

fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

tmp=$(mktemp -d)

call() { # call HEADER METHOD PATH [BODY [CONTENT-TYPE]]: the HTTP status; the body in $tmp/body
  _h=$1 _m=$2 _p=$3
  if [ $# -ge 4 ]; then
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$_m" "$base$_p" -H "$_h" \
      -H "content-type: ${5:-application/json}" -d "$4"
  else
    curl -sS --max-time 60 -o "$tmp/body" -w '%{http_code}' -X "$_m" "$base$_p" -H "$_h"
  fi
}

purge() { # purge HEADER: delete every prompt the person owns (ignores a stack that is down: the trap runs this too)
  _status=$(call "$1" GET '/api/prompts?scope=mine' 2>/dev/null) || return 0
  [ "$_status" = 200 ] || return 0
  for _id in $(jq -r '.items[].id' "$tmp/body"); do
    call "$1" DELETE "/api/prompts/$_id" >/dev/null 2>&1 || true
  done
}

cleanup() {
  purge "$owner" || true
  purge "$team" || true
  purge "$outsider" || true
  rm -rf "$tmp"
}
trap cleanup EXIT

finish() {
  if [ "$fail" -eq 0 ]; then echo "prompts e2e passed"; else echo "prompts e2e FAILED"; exit 1; fi
}

expect() { # expect DESCRIPTION ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: expected '$3', got '$2'"; fi
}

field() { # field JQ-FILTER: a value of the last body, or nothing when it is absent or null (`false` and `0` are values: `// empty` would drop `false`)
  jq -r "($1) | select(. != null)" "$tmp/body"
}

bool_field() { # bool_field JQ-FILTER: `true` or `false`; anything else (absent, null, a string) is `not-a-boolean`, which no check expects
  jq -r "($1) | if type == \"boolean\" then tostring else \"not-a-boolean\" end" "$tmp/body"
}

# a letters-only id for this run, so that no word of it is a word of anything else
run=$(printf '%s%s' "$$" "$(date +%s)" | tr '0-9' 'a-j')
name="review-$run"
sentinel="zq${run}"

echo "== clean up"
purge "$owner"
purge "$team"
purge "$outsider"

echo "== config"
expect "GET /api/config is 200" "$(call "$owner" GET /api/config)" 200
expect "it says prompts, with the limit" "$(field .prompts.limit)" 100

echo "== save"
review=$(jq -n --arg name "$name" '{
  name: $name, title: "Review", description: "Review a pull request",
  body: "Review this PR for {{focus}} at depth {{depth}}.",
  variables: [
    {id: "focus", question: "Focus?", type: "select", options: [{value: "security", label: "Security"}, {value: "speed", label: "Speed"}]},
    {id: "depth", question: "Depth?", type: "scale", to: 5}
  ]}')
expect "create is 201" "$(call "$owner" POST /api/prompts "$review")" 201
id=$(field .id)
rev1=$(field .revision)
expect "the id is p- and 12 characters" "$(printf '%s' "$id" | grep -c '^p-[a-z0-9]\{12\}$')" 1
expect "the caller owns it" "$(bool_field .own)" true
expect "the list is 200" "$(call "$owner" GET '/api/prompts?scope=mine')" 200
expect "the list has it as a summary with two variables" \
  "$(jq -r --arg id "$id" '.items[] | select(.id == $id) | .variableCount' "$tmp/body")" 2
expect "a summary has no body" \
  "$(jq -r --arg id "$id" '.items[] | select(.id == $id) | has("body") or has("variables")' "$tmp/body")" false
expect "the list says how many the caller holds" "$(field .held)" 1
expect "getPrompt is 200" "$(call "$owner" GET "/api/prompts/$id")" 200
expect "and has the body" "$(field .body)" "Review this PR for {{focus}} at depth {{depth}}."
expect "and the two variables" "$(jq -r '.variables | length' "$tmp/body")" 2

echo "== render"
answers=$(jq -n --arg rev "$rev1" '{revision: $rev, answers: [{id: "focus", values: ["security"]}, {id: "depth", values: ["4"]}]}')
expect "render is 200" "$(call "$owner" POST "/api/prompts/$id/render" "$answers")" 200
expect "it is the label, not the value" "$(field .text)" "Review this PR for Security at depth 4."
expect "and says the revision it used" "$(field .revision)" "$rev1"
expect "it wrote nothing: the revision is the same" "$(call "$owner" GET "/api/prompts/$id" >/dev/null; field .revision)" "$rev1"
expect "a scale answer above the most is 422" \
  "$(call "$owner" POST "/api/prompts/$id/render" '{"answers":[{"id":"focus","values":["security"]},{"id":"depth","values":["6"]}]}')" 422
expect "the problem is invalid_answer" "$(field .code)" invalid_answer
expect "for the question depth" "$(field .question)" depth
expect "by the rule above_max" "$(field .rule)" above_max
expect "answers in the wrong order are 422" \
  "$(call "$owner" POST "/api/prompts/$id/render" '{"answers":[{"id":"depth","values":["4"]},{"id":"focus","values":["security"]}]}')" 422
expect "unknown, for the whole answer" "$(field .rule)/$(field .question)" 'unknown/*'
expect "a note is refused: a prompt has no free box" \
  "$(call "$owner" POST "/api/prompts/$id/render" '{"answers":[{"id":"focus","values":["security"]},{"id":"depth","values":["4"]}],"note":"hi"}')" 422
expect "unexpected" "$(field .rule)" unexpected

echo "== a save that the rules refuse"
refuse() { # refuse DESCRIPTION BODY STATUS CODE
  _s=$(call "$owner" POST /api/prompts "$2")
  expect "$1: status" "$_s" "$3"
  expect "$1: code" "$(field .code)" "$4"
}
refuse "an undeclared variable" '{"name":"x","title":"T","body":"Hi {{who}}"}' 400 undeclared_variable
refuse "a variable nothing uses" \
  '{"name":"x","title":"T","body":"Hi","variables":[{"id":"who","question":"Who?","type":"text"}]}' 400 unused_variable
refuse "spaces in a reference" '{"name":"x","title":"T","body":"Hi {{ who }}"}' 400 bad_reference
expect "and the offset is the one of the {{" "$(field .offset)" 3
refuse "an images question" \
  '{"name":"x","title":"T","body":"{{p}}","variables":[{"id":"p","question":"P?","type":"images","options":[{"value":"a","label":"A"},{"value":"b","label":"B"}]}]}' 400 bad_variable
refuse "the same name again" "$(jq -n --arg name "$name" '{name: $name, title: "T", body: "x"}')" 409 name_taken
refuse "a body that is not JSON" 'not json' 400 bad_request
expect "a media type that is not JSON is 415" "$(call "$owner" POST /api/prompts '{"name":"x","title":"T","body":"x"}' text/plain)" 415

echo "== revisions"
edit=$(jq -n --arg rev "$rev1" '{revision: $rev, title: "Review a change"}')
expect "an edit that names the revision it read is 200" "$(call "$owner" PATCH "/api/prompts/$id" "$edit")" 200
rev2=$(field .revision)
expect "and has a new revision" "$([ "$rev2" != "$rev1" ] && echo yes || echo no)" yes
expect "and the new title" "$(field .title)" "Review a change"
expect "the same edit with the old revision is 409" "$(call "$owner" PATCH "/api/prompts/$id" "$edit")" 409
expect "stale_revision" "$(field .code)" stale_revision

echo "== sharing"
share=$(jq -n --arg rev "$rev2" '{revision: $rev, audience: ["prompt-team"]}')
expect "the owner shares with prompt-team: 200" "$(call "$owner" PATCH "/api/prompts/$id" "$share")" 200
rev3=$(field .revision)
expect "the owner sees the audience" "$(jq -r '.audience | join(",")' "$tmp/body")" prompt-team
: >"$tmp/team-bodies"
team_call() { # team_call METHOD PATH [BODY]: as the team person; every body is kept to be searched for the owner's e-mail
  if [ $# -ge 3 ]; then _c=$(call "$team" "$1" "$2" "$3"); else _c=$(call "$team" "$1" "$2"); fi
  cat "$tmp/body" >>"$tmp/team-bodies"
  printf '%s' "$_c"
}
expect "the team lists shared prompts: 200" "$(team_call GET '/api/prompts?scope=shared')" 200
expect "it is there" "$(jq -r --arg id "$id" '[.items[] | select(.id == $id)] | length' "$tmp/body")" 1
expect "as not their own" "$(jq -r --arg id "$id" '.items[] | select(.id == $id) | .own' "$tmp/body")" false
expect "with no audience" "$(jq -r --arg id "$id" '.items[] | select(.id == $id) | has("audience")' "$tmp/body")" false
expect "the team reads it: 200" "$(team_call GET "/api/prompts/$id")" 200
expect "not as theirs" "$(bool_field .own)" false
expect "with no audience" "$(jq -r 'has("audience")' "$tmp/body")" false
answers3=$(jq -n --arg rev "$rev3" '{revision: $rev, answers: [{id: "focus", values: ["security"]}, {id: "depth", values: ["4"]}]}')
expect "the team renders it: 200" "$(team_call POST "/api/prompts/$id/render" "$answers3")" 200
expect "to the same text" "$(field .text)" "Review this PR for Security at depth 4."
theirs=$(jq -n --arg rev "$rev3" '{revision: $rev, title: "Mine now"}')
expect "an edit by the team is 404" "$(team_call PATCH "/api/prompts/$id" "$theirs")" 404
expect "a delete by the team is 404" "$(team_call DELETE "/api/prompts/$id")" 404
expect "no answer the team got holds the owner's e-mail" "$(grep -c -F "$owner_email" "$tmp/team-bodies" || true)" 0

echo "== nobody else"
for who in outsider admin; do
  case $who in outsider) h=$outsider ;; *) h=$admin ;; esac
  expect "$who: the shared list is 200" "$(call "$h" GET '/api/prompts?scope=shared')" 200
  expect "$who: it is not there" "$(jq -r --arg id "$id" '[.items[] | select(.id == $id)] | length' "$tmp/body")" 0
  expect "$who: a read is 404" "$(call "$h" GET "/api/prompts/$id")" 404
  expect "$who: a render is 404" "$(call "$h" POST "/api/prompts/$id/render" "$answers3")" 404
done

echo "== the permission"
expect "without prompts.share a private prompt is 201" \
  "$(call "$outsider" POST /api/prompts "{\"name\":\"mine-$run\",\"title\":\"Mine\",\"body\":\"x\"}")" 201
mine=$(field .id)
mine_rev=$(field .revision)
expect "one shared with everybody is 403" \
  "$(call "$outsider" POST /api/prompts "{\"name\":\"all-$run\",\"title\":\"All\",\"body\":\"x\",\"audience\":[\"*\"]}")" 403
expect "forbidden" "$(field .code)" forbidden
add=$(jq -n --arg rev "$mine_rev" '{revision: $rev, audience: ["*"]}')
expect "an edit that adds an audience is 403" "$(call "$outsider" PATCH "/api/prompts/$mine" "$add")" 403
# the owner shares it with everybody: the outsider lists it, then it is withdrawn
everybody=$(jq -n --arg rev "$rev3" '{revision: $rev, audience: ["*"]}')
expect "the owner shares with everybody: 200" "$(call "$owner" PATCH "/api/prompts/$id" "$everybody")" 200
rev4=$(field .revision)
expect "the outsider lists it" "$(call "$outsider" GET '/api/prompts?scope=shared')" 200
expect "it is there" "$(jq -r --arg id "$id" '[.items[] | select(.id == $id)] | length' "$tmp/body")" 1
withdraw=$(jq -n --arg rev "$rev4" '{revision: $rev, audience: []}')
expect "unsharing: 200" "$(call "$owner" PATCH "/api/prompts/$id" "$withdraw")" 200
rev5=$(field .revision)
expect "the team does not see it at once" "$(call "$team" GET "/api/prompts/$id")" 404
expect "nor does the outsider" "$(call "$outsider" GET "/api/prompts/$id")" 404

echo "== the limit"
call "$outsider" GET '/api/prompts?scope=mine' >/dev/null
held=$(field .held)
need=$((100 - held))
i=0
while [ "$i" -lt "$need" ]; do
  call "$outsider" POST /api/prompts "{\"name\":\"fill-$run-$i\",\"title\":\"Fill\",\"body\":\"x\"}" >/dev/null
  i=$((i + 1))
done
call "$outsider" GET '/api/prompts?scope=mine' >/dev/null
expect "the person holds 100" "$(field .held)" 100
expect "the 101st is 409" "$(call "$outsider" POST /api/prompts "{\"name\":\"one-more-$run\",\"title\":\"One\",\"body\":\"x\"}")" 409
expect "too_many_prompts" "$(field .code)" too_many_prompts
expect "and the list still says 100" "$(call "$outsider" GET '/api/prompts?scope=mine' >/dev/null; field .held)" 100

echo "== delete"
expect "a delete with a stale revision is 409" "$(call "$owner" DELETE "/api/prompts/$id?revision=$rev1")" 409
expect "stale_revision" "$(field .code)" stale_revision
expect "a delete with the current revision is 204" "$(call "$owner" DELETE "/api/prompts/$id?revision=$rev5")" 204
expect "the second is 404" "$(call "$owner" DELETE "/api/prompts/$id")" 404
expect "a read is 404" "$(call "$owner" GET "/api/prompts/$id")" 404

echo "== the log"
# a prompt whose every word is made up for this run: the log must hold none of it
sentinel_prompt=$(jq -n --arg s "$sentinel" '{
  name: ("log-" + $s), title: ("title" + $s), description: ("description" + $s),
  body: ("body" + $s + " {{x}}"),
  variables: [{id: "x", question: "X?", type: "text"}],
  audience: [("role" + $s)]
}')
expect "a prompt of made-up words is 201" "$(call "$owner" POST /api/prompts "$sentinel_prompt")" 201
log_id=$(field .id)
expect "its render is 200" \
  "$(call "$owner" POST "/api/prompts/$log_id/render" "{\"answers\":[{\"id\":\"x\",\"values\":[],\"text\":\"answer$sentinel\"}]}")" 200
call "$owner" DELETE "/api/prompts/$log_id" >/dev/null
if command -v docker >/dev/null 2>&1 && [ -n "$(docker compose ps -q "$service" 2>/dev/null)" ]; then
  docker compose logs --no-color "$service" >"$tmp/log" 2>&1 || true
  if [ -s "$tmp/log" ]; then
    # The log read is the orchestrator's own (it holds its start-up lines): at the stack's level a request is not a line,
    # so the words are looked for in everything the service wrote.
    expect "the log is the orchestrator's" "$(grep -c 'orchestrator::boot' "$tmp/log" | sed 's/^0$/none/;s/^[1-9][0-9]*$/some/')" some
    for word in "$sentinel" "$run" "Review this PR"; do
      if grep -q -F -- "$word" "$tmp/log"; then bad "'$word' is in the orchestrator's log"; else ok "'$word' is in no line of the log"; fi
    done
  else
    echo "skip the log is empty: the service '$service' is not the one of this stack"
  fi
else
  echo "skip no docker compose service '$service' here: the log is not read"
fi

finish

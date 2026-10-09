#!/usr/bin/env sh
# The web image serves the static export as the chart runs it (ADR 0047): unprivileged, a read-only root file system and an
# emptyDir at /tmp; the shells of a thread and a share link, the runtime configuration, the 404 page; the headers of the old
# Next server and the content security policy's two halves (the header, with the issuer's origin from the environment, and
# each page's meta of script hashes).
#
#   sh web/tests/image-smoke.sh <image>      # CI: .github/workflows/web.yml, before the image is pushed
#
# Needs docker and curl. Prints one ok or FAIL line per check and exits 1 if any failed.
set -eu
image=${1:?usage: image-smoke.sh <image>}
port=${PORT:-38080}
name=web-smoke-$$
fail=0
# check <description> <command...>: ok when the command succeeds
check() {
  what=$1
  shift
  if "$@"; then echo "ok   $what"; else echo "FAIL $what"; fail=1; fi
}
cleanup() { docker rm -f "$name" > /dev/null 2>&1 || true; rm -f "$tmp"; }
tmp=$(mktemp)
trap cleanup EXIT

# as the chart runs it (templates/web.yaml): every capability dropped but the one the caddy binary's file capability needs to exec
docker run -d --name "$name" --read-only --tmpfs /tmp --user 1000:1000 --cap-drop ALL --cap-add NET_BIND_SERVICE --security-opt no-new-privileges \
  -e WEB_CSP_CONNECT_SRC=https://auth.example.com -p "127.0.0.1:$port:3000" "$image" > /dev/null
i=0
until curl -fs -o /dev/null "http://127.0.0.1:$port/"; do
  i=$((i + 1))
  if [ "$i" -gt 30 ]; then
    docker logs "$name" >&2
    echo "FAIL the server did not answer"
    exit 1
  fi
  sleep 1
done

# get <path>: the status line and the headers into $tmp.head, the body into $tmp
get() { curl -sS -D "$tmp.head" -o "$tmp" "http://127.0.0.1:$port$1"; }
status() { head -1 "$tmp.head" | cut -d' ' -f2; }
header() { grep -i "^$1:" "$tmp.head" | head -1 | cut -d' ' -f2- | tr -d '\r'; }

is() { [ "$1" = "$2" ]; }
has_meta() { grep -q '<head[^>]*><meta http-equiv="Content-Security-Policy" content="script-src '"'"'self'"'"' '"'"'sha256-' "$tmp"; }
has_header_half() {
  case "$(header Content-Security-Policy)" in
    *"connect-src 'self' https://auth.example.com;"*"frame-ancestors 'none'"*) return 0 ;;
    *) return 1 ;;
  esac
}
for path in / /threads/0199c0de-0000-7000-8000-000000000001 /s/a-token /auth/callback /signed-in; do
  get "$path"
  check "$path is 200" is "$(status)" 200
  check "$path has its meta of script hashes, first in <head>" has_meta
  check "$path has the header half, with the issuer's origin" has_header_half
  check "$path: nosniff" is "$(header X-Content-Type-Options)" nosniff
  check "$path: Referrer-Policy same-origin" is "$(header Referrer-Policy)" same-origin
done
get /threads/0199c0de-0000-7000-8000-000000000001.txt
check "a thread's data is the shell's (.txt)" is "$(status)" 200
get /config.json
check "/config.json is there" is "$(status)" 200
check "... and read anew every time" is "$(header Cache-Control)" no-cache
get /no/such/page
check "an unknown page is a 404" is "$(status)" 404
check "... with the policy" has_header_half
check "no Server header" is "$(header Server)" ""
rm -f "$tmp.head"

[ "$fail" = 0 ] || exit 1
echo "the web image serves the export"

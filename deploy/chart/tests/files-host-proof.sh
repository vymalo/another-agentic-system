#!/bin/sh
# The public files host, proven on a real RustFS (ADR 0058, A5.2; step 6c). By hand: needs docker (with compose), helm, node, curl and openssl;
# it is not run by CI (render-check.sh holds the guarantees on the render, this proves the Caddyfile does what they say).
#
#   sh deploy/chart/tests/files-host-proof.sh          (from the repository root)
#
# What it does:
#   1. renders the chart (examples/netcup.values.yaml, tests/rustfs.values.yaml, tests/files-host.values.yaml) twice, and takes from the render the
#      proxy's Caddyfile and the image the Deployment names: uploads and CORS on (tests/files-host-uploads.values.yaml, `maxBytes` cut to 1024), and the
#      default shape (uploads off, no CORS);
#   2. starts the dev stack's RustFS (`docker compose --profile s3`, the bucket `orch-test` of its bucket Job) and runs each Caddyfile in the chart's
#      pinned Caddy image, as the chart runs it (uid 65532, read-only root, no capability but NET_BIND_SERVICE), with the chart's Service name pointing at RustFS;
#   3. signs URLs for the PUBLIC host (`agentic-files.test`, a name that exists nowhere) with tests/presign.mjs, and sends them with curl to the proxy,
#      with `Host:` set to that name and the headers Traefik adds: a PUT, a GET with the signed response-* members, a HEAD, a Range, a preflight, and what must
#      be refused: a path outside the key prefix, an unsigned query, an S3 subresource, a header-signed request, DELETE, POST, a PUT while uploads are off or
#      too large, a Host that is not the signed one;
#   4. reads the proxy's own logs: no signature, no credential name, no query string, no Referer.
# The containers it starts are removed at the end, and RustFS is stopped if the script started it. No volume or image is removed.
# FILES_PROOF_PORT (18080) is the loopback port of the proxy.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
chart="$here/.."
repo=$(cd "$chart/../.." && pwd)
# deploy.yml wraps Helm to give positive CI renders test-only mobile identities; this proof renders with the app links off instead.
export AGENTIC_SKIP_MOBILE_LINK_FIXTURE=1
port=${FILES_PROOF_PORT:-18080}
public=agentic-files.test
bucket=orch-test
origin=https://agentic.servers.segning.pro
rustfs_service=another-agentic-rustfs.another-agentic-system.svc
work=$(mktemp -d)
fail=0
passed=0
started_rustfs=0
proxies=""

for tool in docker helm node curl openssl sha256sum; do
  command -v "$tool" >/dev/null 2>&1 || { echo "needs $tool" >&2; exit 2; }
done

cleanup() {
  for c in $proxies; do docker rm -f "$c" >/dev/null 2>&1 || true; done
  if [ "$started_rustfs" -eq 1 ]; then docker compose -f "$repo/compose.yaml" --profile s3 stop rustfs >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

ok() { passed=$((passed + 1)); echo "ok   $1"; }
bad() { fail=1; echo "FAIL $1${2:+ ($2)}"; }
check() { # check <description> <command...>
  desc=$1; shift
  if "$@" >/dev/null 2>&1; then ok "$desc"; else bad "$desc"; fi
}
is() { # is <description> <got> <want>
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "got '$2', wanted '$3'"; fi
}

# ---- 1. the render --------------------------------------------------------------------------------------------------------------------
render() { # render <out dir> [helm args]: Caddyfile and image of the proxy, from the chart's own render
  dir=$1; shift
  mkdir -p "$dir"
  helm template another-agentic-system "$chart" --namespace another-agentic-system \
    -f "$chart/examples/netcup.values.yaml" --set mobileAppLinks.enabled=false \
    -f "$chart/tests/rustfs.values.yaml" -f "$chart/tests/files-host.values.yaml" --set "orchestrator.artifacts.s3.bucket=$bucket" "$@" > "$dir/render.yaml"
  awk '/^kind: ConfigMap$/ { c = 1 } c && /^  name: another-agentic-files$/ { f = 1 } f && $0 == "  Caddyfile: |" { on = 1; next } on && /^---$/ { exit } on { sub(/^    /, ""); print }' \
    "$dir/render.yaml" > "$dir/Caddyfile"
  awk '/^kind: Deployment$/ { d = 1 } d && /^  name: another-agentic-files$/ { f = 1 } f && /^ *image: / { gsub(/"/, ""); print $2; exit }' "$dir/render.yaml" > "$dir/image"
  if [ ! -s "$dir/Caddyfile" ] || [ ! -s "$dir/image" ]; then
    echo "no proxy in the render" >&2
    exit 1
  fi
}
echo "rendering the chart"
render "$work/on" -f "$chart/tests/files-host-uploads.values.yaml" --set orchestrator.artifacts.upload.maxBytes=1024
render "$work/off"
image=$(cat "$work/on/image")
[ "$image" = "$(cat "$work/off/image")" ] || { echo "the two renders name different images" >&2; exit 1; }
echo "proxy image: $image"

# ---- 2. RustFS and the proxies ---------------------------------------------------------------------------------------------------------
compose() { docker compose -f "$repo/compose.yaml" --profile s3 "$@"; }
if [ -z "$(compose ps -q --status running rustfs 2>/dev/null)" ]; then started_rustfs=1; fi
compose up -d --wait rustfs >/dev/null
compose run --rm rustfs-bucket 2>&1 | tail -n 1
rustfs_id=$(compose ps -q rustfs)
rustfs_ip=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$rustfs_id")
network=$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}}{{end}}' "$rustfs_id")

start_proxy() { # start_proxy <name> <Caddyfile> <port> <rustfs address>: the chart's pod, as a container
  docker rm -f "$1" >/dev/null 2>&1 || true
  proxies="$proxies $1"
  docker run -d --name "$1" --user 65532:65532 --read-only --cap-drop ALL --cap-add NET_BIND_SERVICE --security-opt no-new-privileges \
    --tmpfs /data:uid=65532,size=16m --tmpfs /config:uid=65532,size=16m -e XDG_DATA_HOME=/data -e XDG_CONFIG_HOME=/config \
    --network "$network" --add-host "$rustfs_service:$4" -p "127.0.0.1:$3:8080" -v "$2:/etc/caddy/Caddyfile:ro" \
    "$image" caddy run --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
  i=0
  until [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$3/")" = 404 ]; do
    i=$((i + 1)); [ "$i" -lt 40 ] || { docker logs "$1" >&2; echo "the proxy $1 does not answer" >&2; exit 1; }
    sleep 0.5
  done
}

# ---- 3. requests -----------------------------------------------------------------------------------------------------------------------
tid=11111111-2222-3333-4444-555555555555
sig64=$(printf 'a%.0s' $(seq 64))
sign() { # sign <method> <path> [presign.mjs options]: a URL for the public host
  m=$1; p=$2; shift 2
  node "$here/presign.mjs" "$m" "https://$public$p" 300 "$@"
}
# curl as Traefik would send it: the public name in Host, and its forwarding headers.
pub() { curl -sS --max-time 30 --path-as-is -H "Host: $public" -H 'X-Forwarded-Proto: https' -H "X-Forwarded-Host: $public" -H 'X-Forwarded-For: 203.0.113.7' "$@"; }
# The proxy's address for a URL signed for the public host: the same path and query.
local_url() { rest=${1#https://}; printf 'http://127.0.0.1:%s/%s' "$cur_port" "${rest#*/}"; }
hv() { tr -d '\r' < "$1" | awk -v n="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]'):" 'index(tolower($0), n) == 1 { sub(/^[^:]*:[ \t]*/, ""); print }'; }
body_sha() { printf '%s' "$1" | sha256sum | cut -d' ' -f1; }
body_b64() { printf '%s' "$1" | openssl dgst -sha256 -binary | base64; }

# put <body> <extra curl args...>: a presigned PUT of the body at its own hash, with the four headers the orchestrator's slot signs (6a: content-type,
# x-amz-checksum-sha256, x-amz-meta-sha256, x-amz-meta-filename); prints the status.
put() {
  p_body=$1; shift
  p_sha=$(body_sha "$p_body"); p_b64=$(body_b64 "$p_body")
  p_url=$(sign PUT "/$bucket/threads/$tid/$p_sha" --header content-type=text/plain --header "x-amz-checksum-sha256=$p_b64" \
    --header "x-amz-meta-sha256=$p_sha" --header x-amz-meta-filename=report.txt)
  pub -o /dev/null -w '%{http_code}' -X PUT -H 'content-type: text/plain' -H "x-amz-checksum-sha256: $p_b64" -H "x-amz-meta-sha256: $p_sha" -H 'x-amz-meta-filename: report.txt' \
    --data-binary "$p_body" "$@" "$(local_url "$p_url")"
}
key_of() { printf '/%s/threads/%s/%s' "$bucket" "$tid" "$(body_sha "$1")"; }

run_phase() { # run_phase <label> <uploads on|off> <cors on|off>
  label=$1; uploads=$2; cors=$3
  echo "---- $label: uploads $uploads, CORS $cors, the proxy on 127.0.0.1:$cur_port"
  doc="hello files host, $label"
  key=$(key_of "$doc")
  secret=SECRETTOKEN-$label

  # The upload, through the proxy, with the signed Host.
  if [ "$uploads" = on ]; then
    is "$label: a presigned PUT with the public Host, the signed checksum and type, is stored (200)" "$(put "$doc")" 200
    exp=$(head -c 1024 /dev/zero | tr '\0' x)
    is "$label: a PUT body of exactly upload.maxBytes (1024) is stored" "$(put "$exp")" 200
    big=$(head -c 2048 /dev/zero | tr '\0' y)
    is "$label: a PUT body over upload.maxBytes (2048 > 1024) is refused with 413, as sent with a length" "$(put "$big")" 413
    big_url=$(sign GET "$(key_of "$big")")
    is "$label: ... and nothing of it is in the bucket" "$(pub -o /dev/null -w '%{http_code}' "$(local_url "$big_url")")" 404
    is "$label: ... nor when it is sent chunked, with no length to refuse it by" "$(put "$big" -H 'Transfer-Encoding: chunked' -H 'Content-Length:')" 413
    is "$label: a PUT with no signature is refused by the proxy (403)" "$(pub -o /dev/null -w '%{http_code}' -X PUT --data-binary x "$(local_url "https://$public$key")")" 403
  else
    is "$label: a presigned PUT, signed right, is refused: uploads are off (405)" "$(put "$doc")" 405
    # The object read below was put in by the phase that ran first (its seed).
  fi

  # The download, through the proxy.
  disp='attachment; filename="a b.txt"; filename*=UTF-8'"''"'a%20b.txt'
  url=$(sign GET "$key" --query "response-content-disposition=$disp" --query response-content-type=application/octet-stream --query 'response-cache-control=private, max-age=300')
  pub -D "$work/h" -o "$work/b" "$(local_url "$url")"
  is "$label: a presigned GET with the public Host returns the bytes that were put" "$(cat "$work/b")" "$doc"
  is "$label: ... with the signed Content-Disposition" "$(hv "$work/h" content-disposition)" "$disp"
  is "$label: ... the signed Content-Type" "$(hv "$work/h" content-type)" application/octet-stream
  is "$label: ... the signed Cache-Control" "$(hv "$work/h" cache-control)" 'private, max-age=300'
  is "$label: ... X-Content-Type-Options: nosniff, once" "$(hv "$work/h" x-content-type-options)" nosniff
  is "$label: ... Content-Security-Policy: default-src 'none'; sandbox, once" "$(hv "$work/h" content-security-policy)" "default-src 'none'; sandbox"
  is "$label: ... Referrer-Policy: no-referrer, once" "$(hv "$work/h" referrer-policy)" no-referrer
  is "$label: ... no CORS header without an Origin" "$(hv "$work/h" access-control-allow-origin)" ""
  pub -D "$work/h" -o "$work/b" -H 'Range: bytes=0-4' "$(local_url "$(sign GET "$key" --query response-content-type=text/plain)")"
  is "$label: a Range of the same URL is a 206 with the 5 bytes" "$(cat "$work/b")" "$(printf '%s' "$doc" | head -c 5)"
  is "$label: ... and its Content-Range" "$(hv "$work/h" content-range | sed 's#/.*##')" 'bytes 0-4'
  pub -I -D "$work/h" -o /dev/null "$(local_url "$(sign HEAD "$key")")"
  is "$label: a HEAD (a URL signed for HEAD) says the length" "$(hv "$work/h" content-length)" "$(printf '%s' "$doc" | wc -c | tr -d ' ')"
  is "$label: ... with nosniff" "$(hv "$work/h" x-content-type-options)" nosniff
  # The Host is the signed one, with its port.
  port_url=$(node "$here/presign.mjs" GET "https://$public:8443$key" 300)
  is "$label: a URL signed for a Host with a port works only with that Host (kept, port and all)" \
    "$(curl -sS -o /dev/null -w '%{http_code}' -H "Host: $public:8443" -H 'X-Forwarded-Proto: https' "$(local_url "$port_url")")" 200
  is "$label: a URL sent with another Host than the one signed is refused by RustFS (403)" \
    "$(curl -sS -o /dev/null -w '%{http_code}' -H 'Host: other.example' "$(local_url "$url")")" 403

  # What must be refused.
  for p in / "/$bucket" "/$bucket/" "/$bucket/?list-type=2" "/$bucket/threads/" "/$bucket/threads/$tid/" "/other-bucket$(printf %s "$key" | sed "s#^/$bucket##")" "/$bucket/other$(printf %s "$key" | sed "s#^/$bucket##")" \
    /rustfs/admin/v3/info /minio/admin/v3/info /health/live /health/ready /rustfs/console/ /metrics; do
    code=$(pub -o /dev/null -w '%{http_code}' "http://127.0.0.1:$cur_port$p?X-Amz-Signature=$sig64")
    is "$label: $p is a 404" "$code" 404
  done
  list_url=$(sign GET "/$bucket/" --query list-type=2)
  is "$label: the bucket's listing, with a signature that is right, is a 404 (the path is not an artifact key)" "$(pub -o /dev/null -w '%{http_code}' "$(local_url "$list_url")")" 404
  is "$label: a doubled slash is a 404 (not folded into a path that matches)" "$(pub -o /dev/null -w '%{http_code}' "http://127.0.0.1:$cur_port/$key?X-Amz-Signature=$sig64")" 404
  is "$label: a /../ is a 404" "$(pub -o /dev/null -w '%{http_code}' "http://127.0.0.1:$cur_port/$bucket/x/..${key#/"$bucket"}?X-Amz-Signature=$sig64")" 404
  unsigned=$(pub -w '\n%{http_code}' "$(local_url "https://$public$key")" | tr '\n' ' ')
  is "$label: an unsigned request for a key is refused by the proxy, in its words (403)" "$unsigned" 'a signed URL is required 403'
  is "$label: a signature that is not 64 hex digits is the same 403" "$(pub -w '%{http_code}' -o /dev/null "$(local_url "https://$public$key?X-Amz-Signature=zz")")" 403
  sub_url=$(sign GET "$key" --query uploads=)
  is "$label: an S3 subresource (?uploads), signed right, is refused by the proxy (403)" "$(pub -o "$work/b" -w '%{http_code}' "$(local_url "$sub_url")")" 403
  is "$label: ... in the proxy's words, not RustFS's" "$(cat "$work/b")" 'a signed URL is required'
  tag_url=$(sign GET "$key" --query tagging=)
  is "$label: ?tagging is refused the same" "$(pub -o /dev/null -w '%{http_code}' "$(local_url "$tag_url")")" 403
  hdr_signed=$(pub -o /dev/null -w '%{http_code}' --user minioadmin:minioadmin --aws-sigv4 aws:amz:us-east-1:s3 "http://127.0.0.1:$cur_port$key")
  is "$label: a request signed in a header (the S3 API's way for a holder of the key) is refused (403)" "$hdr_signed" 403
  del_url=$(sign DELETE "$key")
  is "$label: DELETE, with a URL signed for DELETE, is a 405" "$(pub -o /dev/null -w '%{http_code}' -X DELETE "$(local_url "$del_url")")" 405
  is "$label: ... and the object is still there" "$(pub "$(local_url "$(sign GET "$key")")")" "$doc"
  is "$label: POST (?uploads, a multipart upload) is a 405" "$(pub -o /dev/null -w '%{http_code}' -X POST "$(local_url "$(sign POST "$key" --query uploads=)")")" 405
  is "$label: PATCH is a 405" "$(pub -o /dev/null -w '%{http_code}' -X PATCH "$(local_url "$url")")" 405
  pub -D "$work/h" -o /dev/null -X DELETE "$(local_url "$del_url")"
  if [ "$uploads" = on ]; then allow='GET, HEAD, OPTIONS, PUT'; else allow='GET, HEAD, OPTIONS'; fi
  is "$label: a refused method is told which are served" "$(hv "$work/h" allow)" "$allow"

  # CORS.
  base=$(local_url "$url")
  if [ "$cors" = on ]; then
    pub -X OPTIONS -D "$work/h" -o /dev/null -H "Origin: $origin" -H 'Access-Control-Request-Method: GET' -H 'Access-Control-Request-Headers: dpop' "$base"
    is "$label: a preflight from a listed origin is a 204 the proxy answers" "$(head -n 1 "$work/h" | tr -d '\r' | cut -d' ' -f2)" 204
    is "$label: ... with that origin, not *" "$(hv "$work/h" access-control-allow-origin)" "$origin"
    is "$label: ... methods GET, HEAD, OPTIONS: never PUT" "$(hv "$work/h" access-control-allow-methods)" 'GET, HEAD, OPTIONS'
    is "$label: ... headers DPoP and Range" "$(hv "$work/h" access-control-allow-headers)" 'DPoP, Range'
    is "$label: ... Vary: Origin" "$(hv "$work/h" vary)" Origin
    is "$label: ... no credentials" "$(hv "$work/h" access-control-allow-credentials)" ""
    pub -X OPTIONS -D "$work/h" -o /dev/null -H 'Origin: tauri://localhost' -H 'Access-Control-Request-Method: PUT' "$base"
    is "$label: a preflight asking for PUT gets methods that do not have it" "$(hv "$work/h" access-control-allow-methods)" 'GET, HEAD, OPTIONS'
    pub -X OPTIONS -D "$work/h" -o /dev/null -H 'Origin: https://evil.example' -H 'Access-Control-Request-Method: GET' "$base"
    is "$label: a preflight from another origin is a 204 with no CORS header at all" "$(grep -ci '^access-control-' "$work/h" || true)" 0
    pub -D "$work/h" -o /dev/null -H "Origin: $origin" "$base"
    is "$label: a read from a listed origin is answered as that origin" "$(hv "$work/h" access-control-allow-origin)" "$origin"
    is "$label: ... exposing Content-Disposition, Content-Length and Content-Range" "$(hv "$work/h" access-control-expose-headers)" 'Content-Disposition, Content-Length, Content-Range'
    is "$label: ... and varying by Origin" "$(hv "$work/h" vary)" Origin
    pub -D "$work/h" -o /dev/null -H 'Origin: https://evil.example' "$base"
    is "$label: a read from another origin has no CORS header" "$(grep -ci '^access-control-' "$work/h" || true)" 0
    is "$label: ... but still varies by Origin (a cache must not give it to a listed one)" "$(hv "$work/h" vary)" Origin
    pub -D "$work/h" -o /dev/null "$base"
    is "$label: a read with no Origin varies by Origin too, and has no CORS header" "$(hv "$work/h" vary):$(grep -ci '^access-control-' "$work/h" || true)" Origin:0
  else
    pub -X OPTIONS -D "$work/h" -o /dev/null -H "Origin: $origin" -H 'Access-Control-Request-Method: GET' "$base"
    is "$label: no origin listed: a preflight is a 204 with no Access-Control header and no Vary" "$(head -n 1 "$work/h" | tr -d '\r' | cut -d' ' -f2):$(grep -ci '^access-control-\|^vary:' "$work/h" || true)" 204:0
    pub -D "$work/h" -o /dev/null -H "Origin: $origin" "$base"
    is "$label: ... and a read from that origin has none either" "$(grep -ci '^access-control-\|^vary:' "$work/h" || true)" 0
  fi
  # A Referer that carries a share-link capability, for the log check.
  pub -o /dev/null -H "Referer: https://agentic.servers.segning.pro/s/$secret" "$base"
}

cur_port=$port
start_proxy files-proof-on "$work/on/Caddyfile" "$port" "$rustfs_ip"
# The file the second phase reads is put in by this one (the second proxy does not take a PUT).
is "seed: the file of the second phase is put in through the first proxy" "$(put "hello files host, defaults")" 200
run_phase "uploads-and-cors" on on
docker logs files-proof-on > "$work/log-on" 2>&1
docker rm -f files-proof-on >/dev/null
start_proxy files-proof-off "$work/off/Caddyfile" "$port" "$rustfs_ip"
run_phase "defaults" off off
docker logs files-proof-off > "$work/log-off" 2>&1

# A proxy whose upstream is down logs its errors with the request in them.
dead=$((port + 1))
cur_port=$dead
start_proxy files-proof-dead "$work/on/Caddyfile" "$dead" 127.0.0.1
is "dead upstream: a signed read is a 502" "$(pub -o /dev/null -w '%{http_code}' -H "Referer: https://agentic.servers.segning.pro/s/SECRETTOKEN-dead" "$(local_url "$(sign GET "$(key_of x)")")")" 502
docker logs files-proof-dead > "$work/log-dead" 2>&1

# ---- 4. the logs -----------------------------------------------------------------------------------------------------------------------
cat "$work/log-on" "$work/log-off" "$work/log-dead" > "$work/log-all"
lines=$(grep -c '"logger":"http.log.access' "$work/log-all" || true)
check "logs: the proxies logged requests ($lines access lines: the checks below are not vacuous)" test "$lines" -gt 60
check "logs: ... and an error with its request" test "$(grep -c '"level":"error"' "$work/log-dead")" -ge 1
leaks=$(grep 'X-Amz-Signature\|X-Amz-Credential\|X-Amz-Algorithm\|X-Amz-Expires\|X-Amz-SignedHeaders\|response-content-\|response-cache-\|AWS4-HMAC\|minioadmin\|SECRETTOKEN\|Referer' "$work/log-all" || true)
is "logs: no signature, no access key id, no signed member of a query, no Referer in any log line (Authorization is Caddy's REDACTED)" "$(printf '%s' "$leaks" | grep -c . || true)" 0
[ -z "$leaks" ] || printf '%s\n' "$leaks" | cut -c1-400 >&2
is "logs: no uri carries a query string" "$(grep -c '"uri":"[^"]*?' "$work/log-all" || true)" 0

echo
if [ "$fail" -eq 0 ]; then echo "files host proof passed ($passed checks)"; else echo "files host proof FAILED ($passed checks passed)"; exit 1; fi

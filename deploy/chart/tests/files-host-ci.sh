#!/bin/sh
# The public files host's Caddyfile, RUN in CI (ADR 0058, A5.2; step 6c). It needs docker, helm, python3, curl and awk, and no RustFS: the proxy is
# put in front of a stub upstream that logs what reaches it (tests/files-host-stub.py).
#
#   sh deploy/chart/tests/files-host-ci.sh          (from the repository root)
#
# render-check.sh asserts the text of the Caddyfile, and a text can be wrong in a way no line shows (a `respond` deleted from a block, the goldens
# regenerated over it). This runs the Caddyfile and asserts what it DOES:
#   1. renders the chart three ways (the default shape; uploads on, `maxBytes` cut to 1024, with two CORS origins; a bucket with a dot and a prefix, with one
#      CORS origin) and takes from each render the proxy's Caddyfile and the image the Deployment names (the edge's, pinned by tag and digest);
#   2. runs `caddy validate` on each Caddyfile in that image, and shows that it fails on a Caddyfile that is wrong;
#   3. starts the stub, and one proxy per shape in that image as the chart runs it (uid 65532, read-only root, no capability but NET_BIND_SERVICE). The only
#      line of the Caddyfile that is changed is the upstream's address, which is the stub's instead of the RustFS Service's (the script checks there is one);
#   4. sends each proxy a matrix of requests and asserts, for each, the status it gets, whether the request REACHED the stub (the stub's log is the evidence),
#      and the three security headers on every answer that Caddy gives: an OPTIONS in any shape never reaches the upstream; a query with a subresource, no
#      signature or a member the presigner does not write is a 403; a path with a % in it, or off the key prefix, is a 404; DELETE, POST, PATCH and (with
#      uploads off) PUT are a 405; a declared length over the limit is a 413 before anything reaches the upstream, and a chunked body over it is cut at the
#      limit; a GET or HEAD with a body is cut at one byte; what Traefik adds about the caller is not passed on; CORS answers a listed origin only;
#   5. stops the stub, and shows that the 502 Caddy gives has the headers too;
#   6. reads the proxies' logs: no signature, no credential name, no query string.
# Containers and the stub are removed at the end. No volume or image is removed.
#
# FILES_CI_PORT (18180, +1, +2: the proxies) and FILES_CI_STUB_PORT (19190) are loopback ports (the stub listens on all interfaces: the proxy reaches it
# through the host gateway). FILES_CI_CHART is the chart to render (default: this one), to run the matrix against a changed copy.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
chart=$(cd "${FILES_CI_CHART:-$here/..}" && pwd)
# deploy.yml wraps Helm to give positive CI renders test-only mobile identities; this renders with the app links off instead.
export AGENTIC_SKIP_MOBILE_LINK_FIXTURE=1
public=agentic-files.test
port_base=${FILES_CI_PORT:-18180}
stub_port=${FILES_CI_STUB_PORT:-19190}
upstream=another-agentic-rustfs.another-agentic-system.svc:9000
work=$(mktemp -d)
tab=$(printf '\t')
fail=0
passed=0
containers=""
stub_pid=""

for tool in docker helm python3 curl awk; do
  command -v "$tool" >/dev/null 2>&1 || { echo "needs $tool" >&2; exit 2; }
done

cleanup() {
  for c in $containers; do docker rm -f "$c" >/dev/null 2>&1 || true; done
  if [ -n "$stub_pid" ]; then kill "$stub_pid" >/dev/null 2>&1 || true; fi
  rm -rf "$work"
}
trap cleanup EXIT

ok() { passed=$((passed + 1)); echo "ok   $1"; }
bad() { fail=1; echo "FAIL $1${2:+ ($2)}"; }
is() { # is <description> <got> <want>
  if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "got '$2', wanted '$3'"; fi
}
check() { # check <description> <command...>: its output is shown only when it fails
  desc=$1; shift
  if "$@" > "$work/check.out" 2>&1; then ok "$desc"; else bad "$desc"; tail -n 5 "$work/check.out" | cut -c1-300 >&2; fi
}
refuses() { # refuses <command...>: succeeds when the command fails
  if "$@"; then return 1; fi
  return 0
}

# ---- 1. the renders --------------------------------------------------------------------------------------------------------------------
render() { # render <name> [helm args]: the Caddyfile and the image of the proxy, from the chart's own render
  name=$1; shift
  mkdir -p "$work/$name"
  helm template another-agentic-system "$chart" --namespace another-agentic-system \
    -f "$chart/examples/netcup.values.yaml" --set mobileAppLinks.enabled=false \
    -f "$chart/tests/rustfs.values.yaml" -f "$chart/tests/files-host.values.yaml" "$@" > "$work/$name/render.yaml"
  awk '/^kind: ConfigMap$/ { c = 1 } c && /^  name: another-agentic-files$/ { f = 1 } f && $0 == "  Caddyfile: |" { on = 1; next } on && /^---$/ { exit } on { sub(/^    /, ""); print }' \
    "$work/$name/render.yaml" > "$work/$name/Caddyfile"
  awk '/^kind: Deployment$/ { d = 1 } d && /^  name: another-agentic-files$/ { f = 1 } f && /^ *image: / { gsub(/"/, ""); print $2; exit }' \
    "$work/$name/render.yaml" > "$work/$name/image"
  if [ ! -s "$work/$name/Caddyfile" ] || [ ! -s "$work/$name/image" ]; then
    echo "no proxy in the render of $name" >&2
    exit 1
  fi
}
echo "rendering the chart"
render default --set orchestrator.artifacts.s3.bucket=ci-bucket
render uploads --set orchestrator.artifacts.s3.bucket=ci-bucket -f "$chart/tests/files-host-uploads.values.yaml" --set orchestrator.artifacts.upload.maxBytes=1024
render prefix --set orchestrator.artifacts.s3.bucket=my.bucket --set orchestrator.artifacts.s3.prefix=/prod//agentic/ --set 'rustfs.ingress.corsOrigins={https://a.example}'
image=$(cat "$work/default/image")
for s in uploads prefix; do
  [ "$image" = "$(cat "$work/$s/image")" ] || { echo "the renders name different images" >&2; exit 1; }
done
echo "proxy image: $image"
case $image in
  *@sha256:????????????????????????????????????????????????????????????????) ok "the proxy runs the edge's image, pinned by tag and digest" ;;
  *) bad "the proxy runs the edge's image, pinned by tag and digest" "got '$image'" ;;
esac

# ---- 2. caddy validate -----------------------------------------------------------------------------------------------------------------
# docker_caddy <Caddyfile> <args...>: the pinned image as the chart runs it, with the Caddyfile at /etc/caddy/Caddyfile.
docker_caddy() {
  dc_file=$1; shift
  docker run --rm --user 65532:65532 --read-only --cap-drop ALL --cap-add NET_BIND_SERVICE --security-opt no-new-privileges \
    --tmpfs /data:uid=65532,size=16m --tmpfs /config:uid=65532,size=16m -e XDG_DATA_HOME=/data -e XDG_CONFIG_HOME=/config \
    -v "$dc_file:/etc/caddy/Caddyfile:ro" "$image" "$@"
}
# The image is pulled first, with retries: a registry that does not answer is not a failure of the Caddyfile.
pulled=0
for attempt in 1 2 3; do
  if docker pull --quiet "$image" >/dev/null 2>&1; then pulled=1; break; fi
  echo "docker pull $image failed (attempt $attempt of 3)" >&2
  sleep 5
done
[ "$pulled" -eq 1 ] || docker image inspect "$image" >/dev/null 2>&1 || { echo "the image $image is neither pulled nor local" >&2; exit 1; }
for s in default uploads prefix; do
  check "caddy validate: the Caddyfile of the $s shape is valid" docker_caddy "$work/$s/Caddyfile" caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile
done
# Not vacuous: a Caddyfile with a directive that does not exist, as the first line of the site, is refused by the same command.
sed 's/^:8080 {$/:8080 {\n\tbogus_directive/' "$work/default/Caddyfile" > "$work/broken.Caddyfile"
if cmp -s "$work/default/Caddyfile" "$work/broken.Caddyfile"; then echo "the broken Caddyfile is the Caddyfile: the site is not :8080 {" >&2; exit 1; fi
check "caddy validate: ... and it refuses a Caddyfile with an unknown directive (so the check above is not vacuous)" refuses docker_caddy "$work/broken.Caddyfile" caddy validate --config /etc/caddy/Caddyfile --adapter caddyfile

# ---- 3. the stub and the proxies -------------------------------------------------------------------------------------------------------
: > "$work/stub.log"
python3 "$here/files-host-stub.py" "$stub_port" "$work/stub.log" &
stub_pid=$!
i=0
until [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$stub_port/")" = 200 ]; do
  i=$((i + 1)); [ "$i" -lt 40 ] || { echo "the stub does not answer" >&2; exit 1; }
  sleep 0.25
done

start_proxy() { # start_proxy <shape> <port>: the chart's pod, as a container, with the stub where RustFS would be
  sp_dir="$work/$1"
  if [ "$(grep -Ec "^[[:space:]]*reverse_proxy $upstream " "$sp_dir/Caddyfile")" -ne 1 ]; then
    echo "$1: the Caddyfile does not name its upstream once (reverse_proxy $upstream)" >&2
    exit 1
  fi
  sed "s#^\\([[:space:]]*reverse_proxy \\)$upstream #\\1stub.test:$stub_port #" "$sp_dir/Caddyfile" > "$sp_dir/run.Caddyfile"
  docker rm -f "files-ci-$1" >/dev/null 2>&1 || true
  containers="$containers files-ci-$1"
  docker run -d --name "files-ci-$1" --user 65532:65532 --read-only --cap-drop ALL --cap-add NET_BIND_SERVICE --security-opt no-new-privileges \
    --tmpfs /data:uid=65532,size=16m --tmpfs /config:uid=65532,size=16m -e XDG_DATA_HOME=/data -e XDG_CONFIG_HOME=/config \
    --add-host "stub.test:host-gateway" -p "127.0.0.1:$2:8080" -v "$sp_dir/run.Caddyfile:/etc/caddy/Caddyfile:ro" \
    "$image" caddy run --config /etc/caddy/Caddyfile --adapter caddyfile >/dev/null
  sp_i=0
  until [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$2/")" = 404 ]; do
    sp_i=$((sp_i + 1))
    if [ "$sp_i" -ge 60 ]; then docker logs "files-ci-$1" >&2; echo "the proxy $1 does not answer" >&2; exit 1; fi
    sleep 0.25
  done
}
start_proxy default "$port_base"
start_proxy uploads $((port_base + 1))
start_proxy prefix $((port_base + 2))
: > "$work/stub.log"

# ---- 4. the matrix ---------------------------------------------------------------------------------------------------------------------
uuid=11111111-2222-3333-4444-555555555555
sha=$(printf '%064d' 0 | tr 0 a)
sig=$(printf '%064d' 0 | tr 0 b)
# A signature in the format of SigV4 and the members the presigner writes: the proxy does not check the signature, RustFS does.
signed="X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential=ak%2F20260101%2Fus-east-1%2Fs3%2Faws4_request&X-Amz-Date=20260101T000000Z&X-Amz-Expires=300&X-Amz-SignedHeaders=host&X-Amz-Signature=$sig"
head -c 1024 /dev/zero > "$work/body1024"
head -c 1025 /dev/zero > "$work/body1025"
head -c 2048 /dev/zero > "$work/body2048"
head -c 4096 /dev/zero > "$work/body4096"
head -c 500 /dev/zero > "$work/body500"
printf 'a body for a GET' > "$work/bodyget"
secret_referer="https://agentic.servers.segning.pro/s/SECRETTOKEN-ci"

n=0
cases="$work/cases"
: > "$cases"

# The headers of the FINAL response in a header file: with `Expect: 100-continue` curl dumps the interim `100 Continue` block first, and Caddy writes its
# deferred headers on that one too (sometimes), which is harmless and not the answer.
final_headers() { tr -d '\r' < "$1" | awk 'BEGIN { RS = "" } { last = $0 } END { print last }'; }
hcount() { # hcount <header file> <name>: how many header lines of the final response have that name
  final_headers "$1" | awk -v n="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]'):" 'index(tolower($0), n) == 1 { c++ } END { print c + 0 }'
}
hv() { # hv <header file> <name>: the value of the header, one line per occurrence
  final_headers "$1" | awk -v n="$(printf '%s' "$2" | tr '[:upper:]' '[:lower:]'):" 'index(tolower($0), n) == 1 { sub(/^[^:]*:[ \t]*/, ""); print }'
}
sec_headers() { # sec_headers <header file>: the three security headers once each, and no Server and no Via
  [ "$(hcount "$1" x-content-type-options)" -eq 1 ] && [ "$(hv "$1" x-content-type-options)" = nosniff ] &&
    [ "$(hcount "$1" content-security-policy)" -eq 1 ] && [ "$(hv "$1" content-security-policy)" = "default-src 'none'; sandbox" ] &&
    [ "$(hcount "$1" referrer-policy)" -eq 1 ] && [ "$(hv "$1" referrer-policy)" = no-referrer ] &&
    [ "$(hcount "$1" server)" -eq 0 ] && [ "$(hcount "$1" via)" -eq 0 ]
}

# req <label> <want status> <reach: yes|no|any> [curl args...]: one request to the proxy of the shape being run. The status and the headers are judged
# at once; whether it reached the stub, once the stub has had time to log it (report). $rid is its id, $rn its number. nosec=1: no headers expected.
nosec=0
req() {
  rlabel=$1; rwant=$2; rreach=$3; shift 3
  n=$((n + 1)); rn=$n; rid="$shape-$n"
  case " $* " in
    *"Expect: 100-continue"*) ;;
    *) set -- "$@" -H 'Expect:' ;;
  esac
  rcode=$(curl -sS -o "$work/b.$rn" -D "$work/h.$rn" --max-time 20 --path-as-is -H "Host: $public" -H "X-Case: $rid" -w '%{http_code}' "$@" 2>"$work/e.$rn") || true
  rsec=skip
  if [ "$nosec" -eq 0 ]; then
    if sec_headers "$work/h.$rn"; then rsec=ok; else rsec=bad; fi
  fi
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$rn" "$rid" "$rwant" "$rcode" "$rreach" "$rsec" "$rlabel" >> "$cases"
}

stub_hits() { grep -cF "\"x-case\":\"$1\"" "$work/stub.log" || true; }
stub_rec() { grep -F "\"x-case\":\"$1\"" "$work/stub.log" | grep -F "\"phase\":\"$2\"" | head -n 1 || true; }
stub_bytes() { stub_rec "$1" end | grep -o '"body_bytes":[0-9]*' | cut -d: -f2; }
stub_complete() { stub_rec "$1" end | grep -o '"complete":[a-z]*' | cut -d: -f2; }
stub_target() { stub_rec "$1" start | grep -o '"target":"[^"]*"' | cut -d'"' -f4; }
stub_header() { stub_rec "$1" start | grep -o "\"$2\":\"[^\"]*\"" | cut -d'"' -f4; }
stub_has_header() { stub_rec "$1" start | grep -q "\"$2\":\""; }

# Words for the checks that compare a string (if/else, never A && B || C).
has_header() { if stub_has_header "$@"; then echo present; else echo none; fi; }
acao_none() { if noacao "$1"; then echo none; else echo some; fi; }
bytes_le() { if [ "$(stub_bytes "$1")" -le "$2" ]; then echo atmost; else echo more; fi; }

report() { # report: the verdict on each request of the shape, after the stub has had time to log
  sleep 0.5
  while IFS=$tab read -r _ cid cwant cgot creach csec clabel; do
    problems=""
    [ "$cgot" = "$cwant" ] || problems="$problems status $cgot, wanted $cwant;"
    hits=$(stub_hits "$cid" | head -n 1)
    case $creach in
      yes) [ "$hits" -ge 1 ] || problems="$problems it did not reach the upstream;" ;;
      no) [ "$hits" -eq 0 ] || problems="$problems it reached the upstream;" ;;
      *) ;;
    esac
    [ "$csec" != bad ] || problems="$problems the security headers are not once each, or there is a Server or Via;"
    reach_text=$creach
    [ "$creach" != no ] || reach_text="not reaching the upstream"
    if [ -z "$problems" ]; then ok "$shape: $clabel ($cgot, $reach_text)"; else bad "$shape: $clabel" "$problems"; fi
  done < "$cases"
  : > "$cases"
}

# For the headers of one answer, by the number req returned in $rn.
noacao() { [ "$(final_headers "$work/h.$1" | grep -ci '^access-control-')" -eq 0 ]; }

run_shape() { # run_shape <shape> <port> <bucket path> <uploads on|off> <listed origins: space-separated, or none>
  shape=$1; cur_port=$2; kp=$3; uploads=$4; origins=$5
  base="http://127.0.0.1:$cur_port"
  key="$kp/threads/$uuid/$sha"
  o1=${origins%% *}
  [ "$o1" != none ] || o1=https://agentic.servers.segning.pro
  evil=https://evil.example
  if [ "$uploads" = on ]; then allow='GET, HEAD, OPTIONS, PUT'; else allow='GET, HEAD, OPTIONS'; fi
  echo "---- $shape: bucket path $kp, uploads $uploads, CORS origins: $origins; the proxy on $base"

  # The controls: a signed read reaches the upstream, as it was sent, and what Traefik adds about the caller does not.
  req "GET a signed key, with the headers a hop in front adds" 200 yes "$base$key?$signed" -H 'Authorization: AWS4-HMAC-SHA256 Credential=ak/20260101/us-east-1/s3/aws4_request' \
    -H 'X-Forwarded-Host: evil.example' -H 'X-Forwarded-Proto: http' -H 'X-Forwarded-Port: 81' -H 'X-Forwarded-Server: evil' -H 'Forwarded: for=9.9.9.9;host=evil.example' \
    -H 'X-Real-Ip: 9.9.9.9' -H 'Cookie: session=1' -H 'Via: 1.1 evil' -H 'X-Forwarded-For: 203.0.113.7' -H "Referer: $secret_referer"
  id_get=$rid; rn_get=$rn
  req "GET a signed key, with no Accept-Encoding" 200 yes "$base$key?$signed"
  id_plain=$rid
  req "HEAD a signed key" 200 yes -I "$base$key?$signed"
  req "GET a signed key from a listed origin" 200 yes "$base$key?$signed" -H "Origin: $o1"
  rn_o1=$rn
  req "GET a signed key from another origin" 200 yes "$base$key?$signed" -H "Origin: $evil"
  rn_evil200=$rn
  req "GET a signed key with a % in the query only (X-Amz-Credential has %2F)" 200 yes "$base$key?$signed"

  # OPTIONS, in every shape: never the upstream's.
  req "OPTIONS, a preflight of a listed origin asking GET" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: $o1" -H 'Access-Control-Request-Method: GET' -H 'Access-Control-Request-Headers: dpop'
  rn_pre=$rn
  req "OPTIONS, a preflight of a listed origin asking PUT" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: $o1" -H 'Access-Control-Request-Method: PUT'
  rn_prePUT=$rn
  req "OPTIONS, a preflight from another origin" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: $evil" -H 'Access-Control-Request-Method: GET'
  rn_preevil=$rn
  req "OPTIONS, no Origin" 204 no -X OPTIONS "$base$key?$signed"
  req "OPTIONS, a listed Origin and no Access-Control-Request-Method" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: $o1"
  rn_nonpre=$rn
  req "OPTIONS, unsigned" 204 no -X OPTIONS "$base$key"
  req "OPTIONS, unsigned, from a listed origin with a requested method" 204 no -X OPTIONS "$base$key" -H "Origin: $o1" -H 'Access-Control-Request-Method: GET'
  req "OPTIONS, with a subresource in the query (?uploads)" 204 no -X OPTIONS "$base$key?$signed&uploads="
  req "OPTIONS, with a body" 204 no -X OPTIONS "$base$key?$signed" -d 'a body'
  req "OPTIONS, off the key prefix: /" 404 no -X OPTIONS "$base/"
  req "OPTIONS, off the key prefix: the bucket" 404 no -X OPTIONS "$base$kp/" -H "Origin: $o1" -H 'Access-Control-Request-Method: GET'
  req "OPTIONS, a key with a %2F in its path" 404 no -X OPTIONS "$base${kp}%2Fthreads/$uuid/$sha?$signed" -H "Origin: $o1" -H 'Access-Control-Request-Method: GET'
  nosec=1
  req "OPTIONS * (answered by Go's HTTP server, before Caddy sees it)" 200 no -X OPTIONS --request-target '*' "$base"
  nosec=0

  # No signature, or one that is not the presigner's: the proxy's own 403, and nothing reaches the upstream.
  req "unsigned" 403 no "$base$key"
  req "an empty query" 403 no "$base$key?"
  req "a signature of 63 digits" 403 no "$base$key?X-Amz-Signature=${sig%?}"
  req "a signature of 65 digits" 403 no "$base$key?X-Amz-Signature=${sig}b"
  req "a signature that is not hex" 403 no "$base$key?X-Amz-Signature=$(printf '%064d' 0 | tr 0 z)"
  req "a signature in capitals" 403 no "$base$key?X-Amz-Signature=$(printf '%064d' 0 | tr 0 B)"
  for sub in uploads= uploads tagging= acl= versionId=1 'partNumber=1&uploadId=abc' response-content-encoding=gzip response-expires=Mon list-type=2 x-amz-request-payer=requester X-Amz-Unknown=1 delete= torrent=; do
    req "a valid-format signature and the member $sub" 403 no "$base$key?$signed&$sub"
  done
  req "a subresource before the signed members" 403 no "$base$key?uploads=&$signed"
  req "an Authorization header and no query (a request signed in a header)" 403 no "$base$key" -H 'Authorization: AWS4-HMAC-SHA256 Credential=ak/20260101/us-east-1/s3/aws4_request, SignedHeaders=host, Signature=abc'
  req "a signature and a member of the query that only looks like one of ours" 403 no "$base$key?X-Amz-Algorithm=AWS4-HMAC-SHA256&Evil=1&X-Amz-Signature=$sig"

  # Paths: a key as the store writes it, and nothing else; not a path with a % in it.
  for p in / "$kp" "$kp/" "$kp/?list-type=2" "$kp/threads/" "$kp/threads/$uuid/" "/elsewhere/threads/$uuid/$sha" "$kp/other/threads/$uuid/$sha" \
    "$kp//threads/$uuid/$sha" "/$kp/threads/$uuid/$sha" "$kp/./threads/$uuid/$sha" "$kp/x/../threads/$uuid/$sha" "$key/" "$key/x" "$key;x=1" \
    "$kp/threads/$uuid/$(printf '%s' "$sha" | tr a A)" "$kp/threads/$uuid/${sha%?}" "$kp/threads/$uuid/${sha}a" "$kp/threads/${uuid%?}z/$sha" "$kp/threads/$uuid" \
    /rustfs/admin/v3/info /minio/admin/v3/info /rustfs/console/ /health/live /health/ready /metrics /.env; do
    req "off the key prefix or not a key: $p" 404 no "$base$p?$signed"
  done
  req "a signed listing of the bucket" 404 no "$base$kp/?list-type=2&$signed"
  first_enc=$(printf '%%%02x' "'$(printf '%s' "$sha" | cut -c1)")
  for p in "${kp}%2Fthreads/$uuid/$sha" "$kp/threads/$uuid%2F$sha" "$kp/threads/$uuid/$first_enc${sha#?}" "$kp/threads/$(printf '%s' "$uuid" | sed 's/-/%2d/g')/$sha" \
    "$kp/threads/%2e%2e/$kp/threads/$uuid/$sha" "$kp/threads/$uuid/%2E%2E/$sha" "$kp/threads/$uuid/$sha/%2e%2e" "$key%2F" "$key%25" "$kp/threads/$uuid/${sha%?}%00" \
    "$kp/threads/$uuid/${sha%?}%61" "$kp/threads/$uuid/%25$sha"; do
    req "a path with a % in it: $p" 404 no "$base$p?$signed"
  done

  # Methods.
  for m in DELETE POST PATCH TRACE PROPFIND FOO; do
    req "$m, with a signed query" 405 no -X "$m" "$base$key?$signed"
  done
  rn_del=$n
  req "DELETE, unsigned" 405 no -X DELETE "$base$key"
  req "POST with a body of 2048 bytes (a multipart upload)" 405 no -X POST --data-binary "@$work/body2048" "$base$key?$signed&uploads="
  req "DELETE off the key prefix" 404 no -X DELETE "$base/"
  if [ "$uploads" = off ]; then
    req "PUT, signed, with uploads off" 405 no -X PUT --data-binary "@$work/body500" "$base$key?$signed"
    req "PUT, chunked and big, with uploads off" 405 no -X PUT -H 'Transfer-Encoding: chunked' --data-binary "@$work/body2048" "$base$key?$signed"
  fi

  # Bodies. A GET or HEAD has none worth the name: one byte may reach the upstream, then 413.
  req "GET with a body" 413 any -X GET --data-binary "@$work/bodyget" "$base$key?$signed"
  id_getbody=$rid
  req "GET with a chunked body" 413 any -X GET -H 'Transfer-Encoding: chunked' --data-binary "@$work/body2048" "$base$key?$signed"
  id_getchunk=$rid
  if [ "$shape" = default ]; then
    # curl, told only -X HEAD, waits for a body the answer to a HEAD does not have: two seconds are enough to read the status and the headers.
    req "HEAD with a body" 413 any -X HEAD --max-time 2 --data-binary "@$work/bodyget" "$base$key?$signed"
    id_headbody=$rid
  fi
  req "GET with a body, unsigned (the signature is judged first)" 403 no -X GET --data-binary "@$work/bodyget" "$base$key"
  if [ "$uploads" = on ]; then
    req "PUT of exactly maxBytes (1024), declared" 200 yes -X PUT --data-binary "@$work/body1024" "$base$key?$signed" -H 'content-type: text/plain' -H "x-amz-checksum-sha256: $sig"
    id_put_ok=$rid
    req "PUT of 1025 bytes, declared (maxBytes is 1024)" 413 no -X PUT --data-binary "@$work/body1025" "$base$key?$signed"
    rn_put1025=$rn
    req "PUT of 4096 bytes, declared" 413 no -X PUT --data-binary "@$work/body4096" "$base$key?$signed"
    req "PUT of 2048 bytes, declared, with Expect: 100-continue (the 413 comes first, no 100 is sent)" 413 no -X PUT --data-binary "@$work/body2048" "$base$key?$signed" -H 'Expect: 100-continue'
    rn_put100=$rn
    req "PUT of 2048 bytes, chunked (no length to refuse it by)" 413 yes -X PUT -H 'Transfer-Encoding: chunked' --data-binary "@$work/body2048" "$base$key?$signed"
    id_put_chunk=$rid
    req "PUT of 4096 bytes, chunked, with Expect: 100-continue" 413 yes -X PUT -H 'Transfer-Encoding: chunked' --data-binary "@$work/body4096" "$base$key?$signed" -H 'Expect: 100-continue'
    id_put_chunk100=$rid
    req "PUT of 500 bytes, chunked, within the limit" 200 yes -X PUT -H 'Transfer-Encoding: chunked' --data-binary "@$work/body500" "$base$key?$signed"
    id_put_chunk_ok=$rid
    req "PUT of 1024 bytes, chunked, exactly the limit" 200 yes -X PUT -H 'Transfer-Encoding: chunked' --data-binary "@$work/body1024" "$base$key?$signed"
    req "PUT of 2048 bytes, unsigned" 403 no -X PUT --data-binary "@$work/body2048" "$base$key"
    req "PUT of 2048 bytes, with a subresource" 403 no -X PUT --data-binary "@$work/body2048" "$base$key?$signed&uploads="
    req "PUT of 2048 bytes to a path that is not a key" 404 no -X PUT --data-binary "@$work/body2048" "$base$kp/threads/$uuid/x?$signed"
    req "PUT of 2048 bytes to a path with a % in it" 404 no -X PUT --data-binary "@$work/body2048" "$base$kp/threads/$uuid/$first_enc${sha#?}?$signed"
    req "PUT from another origin, over the limit" 413 no -X PUT --data-binary "@$work/body2048" "$base$key?$signed" -H "Origin: $evil"
    rn_put_evil=$rn
  fi

  # What Go's server answers before Caddy sees the request: it never gets to the upstream, and has no headers of Caddy's.
  nosec=1
  req "a header name with a space (Go's server answers 400 itself)" 400 no -H 'Bad Header: x' "$base$key?$signed"
  nosec=0

  # Errors with an origin that is not listed: no CORS header, whatever the status.
  req "403 for an origin that is not listed" 403 no "$base$key" -H "Origin: $evil"
  rn_e403=$rn
  req "404 for an origin that is not listed" 404 no "$base/" -H "Origin: $evil"
  rn_e404=$rn
  req "405 for an origin that is not listed" 405 no -X DELETE "$base$key?$signed" -H "Origin: $evil"
  rn_e405=$rn
  req "GET with a body for an origin that is not listed" 413 any -X GET --data-binary "@$work/bodyget" "$base$key?$signed" -H "Origin: $evil"
  rn_e413=$rn

  report

  # ---- what the upstream was sent and what the client got, by request ------------------------------------------------------------
  is "$shape: a signed GET is sent to the upstream as it came (the path and the query, untouched)" "$(stub_target "$id_get")" "$key?$signed"
  is "$shape: ... with the Host the client signed" "$(stub_header "$id_get" host)" "$public"
  is "$shape: ... with no Authorization (only a signature in the query authenticates)" "$(has_header "$id_get" authorization)" none
  for h in x-forwarded-host x-forwarded-proto x-forwarded-port x-forwarded-server forwarded x-real-ip cookie via; do
    is "$shape: ... with no $h (what a hop in front says is not passed on)" "$(has_header "$id_get" "$h")" none
  done
  is "$shape: ... and X-Forwarded-For kept, the client's address first and Caddy's peer added" "$(stub_header "$id_get" x-forwarded-for | cut -d, -f1)" 203.0.113.7
  is "$shape: a GET with no Accept-Encoding is sent none (no compression added on the way)" "$(has_header "$id_plain" accept-encoding)" none
  is "$shape: the upstream's Server and Access-Control-Allow-Origin are not passed on" "$(hcount "$work/h.$rn_get" server):$(hcount "$work/h.$rn_get" access-control-allow-origin):$(hcount "$work/h.$rn_get" access-control-allow-credentials)" 0:0:0
  is "$shape: ... and its body is" "$(cat "$work/b.$rn_get")" stub-upstream-reached
  is "$shape: a GET with a body reaches the upstream with one byte of it at most" "$(stub_bytes "$id_getbody")" 1
  is "$shape: ... as does a chunked one" "$(stub_bytes "$id_getchunk")" 1
  is "$shape: ... and none of them ended as declared" "$(stub_complete "$id_getbody")$(stub_complete "$id_getchunk")" falsefalse
  if [ "$shape" = default ]; then
    is "$shape: ... and a HEAD's" "$(stub_bytes "$id_headbody"):$(stub_complete "$id_headbody")" 1:false
  fi

  hdr_allow() { hv "$work/h.$1" allow; }
  is "$shape: a refused method says which are served" "$(hdr_allow "$rn_del")" "$allow"

  if [ "$uploads" = on ]; then
    is "$shape: a PUT of exactly maxBytes reaches the upstream whole" "$(stub_bytes "$id_put_ok"):$(stub_complete "$id_put_ok")" 1024:true
    is "$shape: ... with its signed headers" "$(stub_header "$id_put_ok" content-type)" text/plain
    is "$shape: a chunked PUT over the limit reaches the upstream with at most maxBytes, cut" "$(bytes_le "$id_put_chunk" 1024):$(stub_complete "$id_put_chunk")" atmost:false
    is "$shape: ... also with Expect: 100-continue and a body of 4096" "$(bytes_le "$id_put_chunk100" 1024):$(stub_complete "$id_put_chunk100")" atmost:false
    is "$shape: a chunked PUT within the limit reaches the upstream whole" "$(stub_bytes "$id_put_chunk_ok"):$(stub_complete "$id_put_chunk_ok")" 500:true
    is "$shape: a declared PUT over the limit is refused before any 100 Continue" "$(grep -c '^HTTP/1.1 100' "$work/h.$rn_put100" || true)" 0
    is "$shape: ... in the proxy's words" "$(cat "$work/b.$rn_put1025")" 'request body too large'
    is "$shape: ... with no CORS header, whatever the origin" "$(acao_none "$rn_put_evil")" none
    is "$shape: a PUT is told it is served (Allow has PUT)" "$(hdr_allow "$rn_del" | grep -c PUT || true)" 1
  else
    is "$shape: a PUT is not served (Allow has no PUT)" "$(hdr_allow "$rn_del" | grep -c PUT || true)" 0
  fi

  # CORS.
  is "$shape: a GET from an origin that is not listed has no Access-Control header" "$(acao_none "$rn_evil200")" none
  is "$shape: ... nor a 403, 404, 405 or 413 to it" "$(for r in "$rn_e403" "$rn_e404" "$rn_e405" "$rn_e413"; do if noacao "$r"; then printf n; else printf s; fi; done)" nnnn
  if [ "$origins" != none ]; then
    is "$shape: a GET from a listed origin is answered as that origin (never *)" "$(hv "$work/h.$rn_o1" access-control-allow-origin)" "$o1"
    is "$shape: ... exposing Content-Disposition, Content-Length and Content-Range, varying by Origin" "$(hv "$work/h.$rn_o1" access-control-expose-headers):$(hv "$work/h.$rn_o1" vary)" 'Content-Disposition, Content-Length, Content-Range:Origin'
    is "$shape: a preflight of a listed origin is answered as that origin" "$(hv "$work/h.$rn_pre" access-control-allow-origin)" "$o1"
    is "$shape: ... with GET, HEAD, OPTIONS (never PUT), DPoP and Range, ten minutes, no credentials" \
      "$(hv "$work/h.$rn_pre" access-control-allow-methods):$(hv "$work/h.$rn_pre" access-control-allow-headers):$(hv "$work/h.$rn_pre" access-control-max-age):$(hcount "$work/h.$rn_pre" access-control-allow-credentials)" \
      'GET, HEAD, OPTIONS:DPoP, Range:600:0'
    is "$shape: a preflight that asks for PUT is answered with methods that do not have it" "$(hv "$work/h.$rn_prePUT" access-control-allow-methods)" 'GET, HEAD, OPTIONS'
    is "$shape: an OPTIONS of a listed origin with no requested method is not a preflight: no Access-Control-Allow-Methods" "$(hcount "$work/h.$rn_nonpre" access-control-allow-methods)" 0
    for o in $origins; do
      req "OPTIONS, a preflight of $o" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: $o" -H 'Access-Control-Request-Method: GET'
      is "$shape: ... is answered as $o" "$(hv "$work/h.$rn" access-control-allow-origin)" "$o"
      req "GET from $o" 200 yes "$base$key?$signed" -H "Origin: $o"
      is "$shape: ... is answered as $o" "$(hv "$work/h.$rn" access-control-allow-origin)" "$o"
    done
    req "OPTIONS, a preflight of an origin that only starts like a listed one" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: $o1.evil.example" -H 'Access-Control-Request-Method: GET'
    is "$shape: ... has no Access-Control header" "$(acao_none "$rn")" none
    req "OPTIONS, a preflight of an origin that contains a listed one" 204 no -X OPTIONS "$base$key?$signed" -H "Origin: x$o1" -H 'Access-Control-Request-Method: GET'
    is "$shape: ... has no Access-Control header" "$(acao_none "$rn")" none
  else
    is "$shape: with no origin listed, a GET from an origin has no Access-Control header and no Vary" "$(hcount "$work/h.$rn_o1" access-control-allow-origin):$(hcount "$work/h.$rn_o1" vary)" 0:0
    is "$shape: ... nor a preflight" "$(acao_none "$rn_pre"):$(hcount "$work/h.$rn_pre" vary)" none:0
  fi
  is "$shape: a preflight from an origin that is not listed has no Access-Control header" "$(acao_none "$rn_preevil")" none
  report
}

run_shape default "$port_base" /ci-bucket off none
run_shape uploads $((port_base + 1)) /ci-bucket on "https://agentic.servers.segning.pro tauri://localhost"
run_shape prefix $((port_base + 2)) /my.bucket/prod/agentic off "https://a.example"
shape=prefix
is "prefix: the prefix as the store joins it (/prod//agentic/ is prod/agentic) is the route: a key under another prefix is a 404" "$(curl -s -o /dev/null -w '%{http_code}' --path-as-is -H "Host: $public" "http://127.0.0.1:$((port_base + 2))/my.bucket/prod/threads/$uuid/$sha?$signed")" 404
is "prefix: ... and the bucket without its prefix" "$(curl -s -o /dev/null -w '%{http_code}' --path-as-is -H "Host: $public" "http://127.0.0.1:$((port_base + 2))/my.bucket/threads/$uuid/$sha?$signed")" 404

# ---- 5. the 502 ------------------------------------------------------------------------------------------------------------------------
# A RustFS that is down: Caddy answers 502 itself, from handle_errors.
kill "$stub_pid" >/dev/null 2>&1 || true
wait "$stub_pid" 2>/dev/null || true
stub_pid=""
for s in default uploads prefix; do
  case $s in default) p=$port_base; kp=/ci-bucket ;; uploads) p=$((port_base + 1)); kp=/ci-bucket ;; *) p=$((port_base + 2)); kp=/my.bucket/prod/agentic ;; esac
  shape=$s
  req "a signed GET with the upstream down (Caddy's own 502)" 502 no "http://127.0.0.1:$p$kp/threads/$uuid/$sha?$signed" -H "Referer: $secret_referer"
  req "a signed GET from a listed or unlisted origin with the upstream down has no CORS header" 502 no "http://127.0.0.1:$p$kp/threads/$uuid/$sha?$signed" -H 'Origin: https://evil.example'
  is "$s: ... the 502 has no Access-Control header" "$(acao_none "$rn")" none
done
# The cases are judged against the stub's log of each shape: the stub is down, and no case can have reached it.
report

is "no answer of any shape carries the upstream's CORS origin (the stub says https://evil.example) or its credentials flag" "$(cat "$work"/h.* | grep -ci 'evil\.example\|access-control-allow-credentials' || true)" 0

# ---- 6. the logs -----------------------------------------------------------------------------------------------------------------------
for s in default uploads prefix; do docker logs "files-ci-$s" > "$work/log-$s" 2>&1; done
cat "$work/log-default" "$work/log-uploads" "$work/log-prefix" > "$work/log-all"
lines=$(grep -c '"logger":"http.log.access' "$work/log-all" || true)
check "logs: the proxies logged requests ($lines access lines: the checks below are not vacuous)" test "$lines" -gt 100
check "logs: ... and errors with their request (the 413s and the 502s)" test "$(grep -c '"level":"error"' "$work/log-all")" -ge 3
leaks=$(grep 'X-Amz-Signature\|X-Amz-Credential\|X-Amz-Algorithm\|X-Amz-Expires\|X-Amz-SignedHeaders\|response-content-\|AWS4-HMAC\|SECRETTOKEN\|Referer' "$work/log-all" || true)
is "logs: no signature, no access key id, no signed member of a query, no Referer, no Authorization in any log line" "$(printf '%s' "$leaks" | grep -c . || true)" 0
[ -z "$leaks" ] || printf '%s\n' "$leaks" | cut -c1-300 >&2
is "logs: no uri carries a query string" "$(grep -c '"uri":"[^"]*?' "$work/log-all" || true)" 0
is "logs: no response header named Server or Via in an access line (resp_headers)" "$(grep -o '"resp_headers":{[^}]*}' "$work/log-all" | grep -c '"Server":\|"Via":' || true)" 0
check "logs: ... and the resp_headers are there to be read (security headers in them)" test "$(grep -o '"resp_headers":{[^}]*}' "$work/log-all" | grep -c 'X-Content-Type-Options' || true)" -gt 100

echo
if [ "$fail" -eq 0 ]; then
  echo "files host CI matrix passed ($passed checks)"
else
  for s in default uploads prefix; do echo "---- the log of the $s proxy, last lines" >&2; tail -n 8 "$work/log-$s" | cut -c1-400 >&2; done
  echo "---- the stub's log, last lines" >&2; tail -n 4 "$work/stub.log" | cut -c1-400 >&2
  echo "files host CI matrix FAILED ($passed checks passed)"
  exit 1
fi

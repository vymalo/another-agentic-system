#!/bin/sh
# Assertions on the rendered chart: the properties the design depends on, so a careless edit to the templates or values
# fails CI instead of exposing the deployment. Needs only `helm`, `grep`, `awk`, `sed` and `cmp`.
#
#   sh deploy/chart/tests/render-check.sh            (from the repository root)
#
# The base render is examples/netcup.values.yaml: the values of the Application on netcup, with placeholders for the model.
set -eu

# deploy.yml wraps Helm to give positive CI renders test-only mobile identities. This suite deliberately checks that a
# deployment without those identities is refused, so every invocation here must reach Helm directly.
export AGENTIC_SKIP_MOBILE_LINK_FIXTURE=1

here=$(cd "$(dirname "$0")" && pwd)
chart="$here/.."
repo=$(cd "$chart/../.." && pwd)
base_source="$chart/examples/netcup.values.yaml"
# Most cases below prove an optional subsystem is absent. The production default is the opposite: mobile links are live
# and deliberately refuse a render until the owner supplies real app identities. Keep that baseline proof below while
# layering the explicit off fixture for unrelated chart cases and their historical goldens.
base=$(mktemp)
cp "$base_source" "$base"
printf '\nmobileAppLinks:\n  enabled: false\n' >> "$base"
out=$(mktemp)
cfg=$(mktemp)
sec=$(mktemp)
sec2=$(mktemp)
fcf=$(mktemp)
jcfg=$(mktemp)
trap 'rm -f "$base" "$out" "$cfg" "$sec" "$sec2" "$fcf" "$jcfg"' EXIT
fail=0

check() { # check <description> <command...>
  desc=$1; shift
  if "$@" >/dev/null 2>&1; then
    echo "ok   $desc"
  else
    echo "FAIL $desc"
    fail=1
  fi
}
has() { grep -Eq -- "$1" "$out"; }
lacks() { ! grep -Eq -- "$1" "$out"; }
count() { [ "$(grep -Ec -- "$1" "$out")" -eq "$2" ]; }
cfg_has() { grep -Eq -- "$1" "$cfg"; }
cfg_lacks() { ! grep -Eq -- "$1" "$cfg"; }
fails() { ! "$@"; }
# all_in <file> <pattern>...: every pattern has a match in the file.
all_in() { f=$1; shift; for p in "$@"; do grep -Eq -- "$p" "$f" || return 1; done; }
out_all() { all_in "$out" "$@"; }
cfg_all() { all_in "$cfg" "$@"; }
sec_all() { all_in "$sec" "$@"; }
sec2_all() { all_in "$sec2" "$@"; }
# line_after <file> <anchor> <n> <pattern>: the line <n> lines after EVERY line that matches <anchor> matches <pattern>, and there is such an anchor.
# all_in (above) says only that each pattern is somewhere, so it passes when two values are swapped between their keys; this says which key holds which.
line_after() { awk -v a="$2" -v n="$3" -v p="$4" '{ l[NR] = $0 } $0 ~ a { seen = 1; want[NR + n] = 1 } END { for (i in want) if (l[i] !~ p) bad = 1; exit !(seen && !bad) }' "$1"; }
# render [helm args]: the base render into $out; render_fails: it must not render.
render() { helm template another-agentic-system "$chart" --namespace another-agentic-system -f "$base" "$@" > "$out"; }
renders() { helm template another-agentic-system "$chart" --namespace another-agentic-system -f "$base" "$@" >/dev/null 2>&1; }
# doc <Kind> [name]: the YAML document(s) of one kind (and name) from the render.
doc() {
  awk -v k="$1" -v n="${2:-}" '
    function flush() {
      if (buf ~ ("(^|\n)kind: " k "\n") && (n == "" || buf ~ ("(^|\n)  name: " n "\n"))) printf "%s", buf
      buf = ""
    }
    /^---$/ { flush(); next }
    { buf = buf $0 "\n" }
    END { flush() }' "$out"
}
dhas() { doc "$1" "$2" | grep -Eq -- "$3"; }
dlacks() { ! doc "$1" "$2" | grep -Eq -- "$3"; }
# The orchestrator's config.yaml and agents.yaml from the ConfigMap, to a file.
config_of() { # config_of <key> <file>
  doc ConfigMap another-agentic-orchestrator | awk -v k="  $1: |" '
    $0 == k { on = 1; next }
    on && /^  [^ ]/ { on = 0 }
    on { sub(/^    /, ""); print }' > "$2"
}

render
config_of config.yaml "$cfg"

# The chart must not produce a broken mobile release from placeholders: the deployer supplies the actual Android
# certificate fingerprint and Apple Team ID, and the test fixture only makes non-mobile cases renderable.
check "mobile app links are off by default" sh -c "grep -A3 '^mobileAppLinks:' '$chart/values.yaml' | grep -q '^  enabled: false'"
check "a deployment that turns mobile app links on without real identities is refused" fails helm template another-agentic-system "$chart" --namespace another-agentic-system -f "$base_source" --set mobileAppLinks.enabled=true

# ---- No secret value anywhere ---------------------------------------------------------------------------------------
check "no Secret object is rendered (every secret is an ExternalSecret)" lacks '^kind: Secret$'
check "no token-looking value in the render" lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
check "no secret on a command line (oauth2-proxy reads its secrets from its environment)" lacks '(--client-secret|--cookie-secret|--basic-auth-password|--redis-password)'
# An environment variable whose name says secret takes its value from a Secret, never a literal.
literal_secret_env() {
  awk '
    /^ *- name: / { name = $3; next }
    /^ *value: / && name ~ /(SECRET|TOKEN|PASSWORD|KEY|BEARER|DATABASE_URL)/ { print name; bad = 1 }
    { if ($0 !~ /^ *(value|valueFrom):/) name = name }
    END { exit bad ? 0 : 1 }' "$out"
}
check "no secret-named environment variable has a literal value" fails literal_secret_env
# The orchestrator's configuration holds only references where a secret goes (it refuses a plain string at startup too).
plain_secret_in_config() {
  grep -E '^ *(secret|apiKey|bearer|token|password|accessKeyId|secretAccessKey|previousSecret): ' "$cfg" | grep -Ev '\{ (file|env): ' || true
}
check "every secret key of the orchestrator's config is a { file } or { env } reference" test -z "$(plain_secret_in_config)"
check "the database url is a file reference" cfg_has '^  url: \{ file: /run/secrets/db/uri \}$'
check "four ExternalSecrets (orchestrator, oauth2-proxy, chat, the chat agent's database role), all on the ClusterSecretStore ssegning-aws" count '^kind: ExternalSecret$' 4
check "every ExternalSecret reads the AWS secret prod/another-agentic/env" count '^        key: prod/another-agentic/env$' 9
check "the store is ssegning-aws (ClusterSecretStore)" count '^    name: ssegning-aws$' 4
check "the store kind is ClusterSecretStore" count '^    kind: ClusterSecretStore$' 4
check "the thread tools' key is a file" cfg_has 'secret: \{ file: /run/secrets/orchestrator/thread-tools-secret \}'
check "the ExternalSecrets create Secrets the pods name (orchestrator, oauth2-proxy, chat)" has '^    name: another-agentic-(orchestrator|oauth2-proxy|chat)$'
check "no ExternalSecret property is a value of this chart (all come from the AWS secret)" lacks 'value: .*(thread_tools_secret|oauth2_client_secret|oauth2_cookie_secret)'
chat_prop=$(doc ExternalSecret another-agentic-chat | awk '/secretKey: A2A_BEARER_TOKENS/ { getline; getline; getline; print $2 }')
orch_prop=$(doc ExternalSecret another-agentic-orchestrator | awk '/secretKey: CHAT_A2A_TOKEN/ { getline; getline; getline; print $2 }')
check "the chat agent and the orchestrator read the chat's bearer from the same property" test -n "$chat_prop" -a "$chat_prop" = "$orch_prop"

# ---- The configuration is a production one, fail closed ------------------------------------------------------------
check "server.environment is production" cfg_has '^  environment: production$'
check "auth.mode is jwt" cfg_has '^  mode: jwt$'
check "no proxy header mode anywhere" lacks 'proxy_header'
check "defaultRole is null: a person with no role is refused" cfg_has '^  defaultRole: null$'
check "the issuer is https" cfg_has '^    issuer: "https://'
check "the audience is the client id" cfg_has '^      - another-agentic$'
check "the e-mail is the user claim and the roles come from agentic_roles" cfg_has '^    rolesClaim: "agentic_roles"$'
check "no role may read or act on another person's thread (no scope any)" cfg_lacks '(scope: any|read: any|write: any)'
check "every role is scope own" cfg_has '^      scope: own$'
# ADR 0043: a deployment that lists its roles does not get thread.delete by itself, so the chart's roles list it
check "both roles hold thread.delete, so a person can erase their own threads (ADR 0043)" \
  sh -c "[ \"\$(grep -Ec '^ +- thread\\.delete\$' \"$cfg\")\" -eq 2 ]"
check "agui and thread-tools are mounted" cfg_has '^    - agui$'
check "thread-tools is mounted" cfg_has '^    - thread-tools$'
check "no MCP or webhook surface" cfg_lacks '(^|[ -])(mcp|webhook-generic|webhook-github)([^a-z]|$)'
check "the public URL is the host over https" cfg_has '^  publicUrl: "https://agentic.servers.segning.pro"$'
check "the artifact store is a directory on the volume" cfg_has '^  fs: \{ root: /var/lib/orchestrator/artifacts \}$'
check "the default render writes no history key: the orchestrator's own defaults stand, and the pinned image need not know them (ADR 0059)" cfg_lacks 'history'
check "the thread tools are reached in-cluster, over the service's name" cfg_has '^  url: "http://another-agentic-orchestrator.another-agentic-system.svc:8080"$'
check "the agents file lists the coder first, by its card" grep -q 'cardUrl: http://coder.another-agentic-system.svc:8080/.well-known/agent-card.json' "$out"
check "the coder is gated on its own checks" dhas ConfigMap another-agentic-orchestrator 'agent-checks'
check "a bearer variable is named for each agent" has '^            - name: (CODER|CHAT)_A2A_TOKEN$'

# ---- Exposure --------------------------------------------------------------------------------------------------------
check "one Ingress, on traefik, for the host" count '^kind: Ingress$' 1
check "the Ingress host is the deployment's" has '^    - host: "agentic.servers.segning.pro"$'
check "the Ingress has TLS from the cert-cloudflare issuer" has 'cert-manager.io/cluster-issuer: "cert-cloudflare"'
check "the Ingress class is traefik" has '^  ingressClassName: traefik$'
check "the Ingress backend is the edge, nothing else" dhas Ingress another-agentic 'name: another-agentic-edge'
check "no HTTPRoute, Gateway or IngressRoute (the Gateway API provider is off on netcup)" lacks '^kind: (HTTPRoute|Gateway|IngressRoute)$'
check "no LoadBalancer or NodePort" lacks 'type: (LoadBalancer|NodePort)'
check "five Services, all ClusterIP" count '^  type: ClusterIP$' 5
check "the edge does not route the thread tools" has 'handle /thread-tools/\*'
check "the edge routes no MCP and no webhook" lacks 'handle (/mcp|/webhooks)'
check "every protected route replaces Authorization from oauth2-proxy (forward_auth + copy_headers)" count '^\s+copy_headers Authorization$' 3
check "the edge trusts only private ranges for X-Forwarded-*" has 'trusted_proxies static private_ranges'
check "the AG-UI route buffers the request body (the Go reverse-proxy body-close bug, ADR 0033)" has 'request_buffers 8MiB'
check "oauth2-proxy requires the client role" has -- '--allowed-role=another-agentic:user'
check "oauth2-proxy sets cookies Secure, SameSite lax, with PKCE" has -- '--cookie-secure=true'
check "oauth2-proxy uses the keycloak-oidc provider against the issuer" has -- '--provider=keycloak-oidc'
check "oauth2-proxy sends the ID token on (set-authorization-header)" has -- '--set-authorization-header=true'
check "oauth2-proxy's sign-out ends the issuer's session too (backend logout with the session's ID token)" has -- '--backend-logout-url=https://auth.verif.fyi/realms/vymalo/protocol/openid-connect/logout\?id_token_hint=\{id_token\}'
check "the redirect URL is the host's /oauth2/callback" has -- '--redirect-url=https://agentic.servers.segning.pro/oauth2/callback'

# ---- Images: first party by commit tag, third party by tag and digest, never latest -----------------------------------
check "no latest tag" lacks 'image: .*:latest'
images_ok() {
  grep -E '^ *image: ' "$out" | sed 's/^ *image: //; s/"//g' | while read -r img; do
    case "$img" in
      ghcr.io/vymalo/another-agentic-system/*:sha-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
      *:*@sha256:????????????????????????????????????????????????????????????????) ;;
      *) echo "unpinned: $img" >&2; exit 1 ;;
    esac
  done
}
check "every image is a commit tag of ours or a tag with a digest" images_ok
check "four images are rendered (orchestrator, web, oauth2-proxy, caddy) and the chat agent's: five" count '^ *image: ' 5
check "the orchestrator and the web are ours, by commit" has 'image: "ghcr.io/vymalo/another-agentic-system/(orchestrator|web):sha-[0-9a-f]{7}"'

# ---- The pods ---------------------------------------------------------------------------------------------------------
check "five Deployments" count '^kind: Deployment$' 5
check "no pod mounts a service account token" count 'automountServiceAccountToken: false' 5
check "every pod is non-root with a seccomp profile" count 'runAsNonRoot: true' 5
check "no container escalates privileges" count 'allowPrivilegeEscalation: false' 5
check "every container drops all capabilities" count 'drop: \["ALL"\]' 5
check "four containers have a read-only root (the chat agent's image is unverified for it)" count 'readOnlyRootFilesystem: true' 4
# The web is its static export served by Caddy (ADR 0047): port 3000, /tmp its only writable place (Caddy's XDG directories),
# no Next cache, and NET_BIND_SERVICE kept as the edge keeps it (the binary's file capability needs it to exec).
web_static_ok() {
  d=$(doc Deployment another-agentic-web)
  printf '%s\n' "$d" | grep -Fq 'mountPath: /tmp' &&
    printf '%s\n' "$d" | grep -Fq 'add: ["NET_BIND_SERVICE"]' &&
    printf '%s\n' "$d" | grep -Fq 'containerPort: 3000'
}
check "web: the static export's Caddy on 3000, with /tmp and NET_BIND_SERVICE to exec" web_static_ok
# The rollout gap: the Next.js image of before the static export runs with this spec until web.yml bumps web.image.tag. Remove these
# two checks with the volume, in the release after that bump.
web_old_image_ok() {
  d=$(doc Deployment another-agentic-web)
  printf '%s\n' "$d" | grep -Fq 'mountPath: /app/.next/cache' &&
    printf '%s\n' "$d" | grep -Fq 'sizeLimit: 256Mi' &&
    printf '%s\n' "$d" | grep -Fq 'sizeLimit: 64Mi'
}
check "web: the old Next.js image still has its writable cache and /tmp until the bump (a read-only root otherwise)" web_old_image_ok
check "the edge may bind: the caddy binary has a file capability" has 'add: \["NET_BIND_SERVICE"\]'
check "the orchestrator runs as the distroless nonroot user" dhas Deployment another-agentic-orchestrator 'runAsUser: 65532'
check "the orchestrator is one replica, recreated (a ReadWriteOnce directory store)" dhas Deployment another-agentic-orchestrator 'type: Recreate'
check "the orchestrator probes /healthz and /readyz" dhas Deployment another-agentic-orchestrator 'path: /readyz'
check "the orchestrator reads its secrets from files (0440 with an fsGroup), not 0400" dhas Deployment another-agentic-orchestrator 'defaultMode: 0440'
check "the orchestrator restarts on a new configuration" dhas Deployment another-agentic-orchestrator 'checksum/config:'
check "the database URL is the CNPG app secret's uri, mounted as a file" dhas Deployment another-agentic-orchestrator 'secretName: another-agentic-db-app'
check "the artifacts claim is kept by Helm and Argo CD" dhas PersistentVolumeClaim another-agentic-artifacts 'helm.sh/resource-policy: keep'
check "one CNPG Cluster: the orchestrator's, which holds the chat agent's database too (README.md, \"One database cluster\")" count '^kind: Cluster$' 1
check "the chat agent runs adam-agent, with the thread tools allowed over http" dhas Deployment another-agentic-chat 'MCP_ALLOW_INSECURE'
chat_db_url() { doc Deployment another-agentic-chat | awk '/- name: DATABASE_URL$/ { getline; getline; getline; n = $2; getline; k = $2 } END { exit (n == "another-agentic-db-agent" && k == "uri") ? 0 : 1 }'; }
check "the chat agent's database URL is the key uri of the Secret its role's ExternalSecret templates" chat_db_url
check "no model is named by the chart: the chat model is the value's" dhas Deployment another-agentic-chat 'value: "chat"'
check "no context window by default: the chat renders no MODEL_CONTEXT_WINDOW (the ring shows totals)" fails dhas Deployment another-agentic-chat 'MODEL_CONTEXT_WINDOW'

# ---- Network policies ---------------------------------------------------------------------------------------------------
check "five NetworkPolicies (edge, web, oauth2-proxy, orchestrator, chat)" count '^kind: NetworkPolicy$' 5
check "no NetworkPolicy restricts egress" lacks '^    - Egress$'
check "the orchestrator accepts the edge, the chat agent and the coder" dhas NetworkPolicy another-agentic-orchestrator 'app.kubernetes.io/name: coder'

# ---- Values drive what they should ----------------------------------------------------------------------------------
render --set host=agentic.example.org --set ingress.clusterIssuer=cert-other --set externalSecrets.key=prod/other/env
check "the host is values-driven" has '^    - host: "agentic.example.org"$'
check "the issuer is values-driven" has 'cert-manager.io/cluster-issuer: "cert-other"'
check "the AWS secret is values-driven" has '^        key: prod/other/env$'
check "the redirect URL follows the host" has -- '--redirect-url=https://agentic.example.org/oauth2/callback'
helm template another-agentic-system "$chart" --namespace other-ns -f "$base" > "$out"
check "the coder's card URL follows the release's namespace" has 'http://coder.other-ns.svc:8080/.well-known/agent-card.json'
check "the names do not follow the release (fullnameOverride)" count '^  name: another-agentic-orchestrator$' 5
helm template renamed "$chart" --namespace another-agentic-system -f "$base" > "$out"
check "a differently named release keeps the same object names" has '^  name: another-agentic-orchestrator$'
render --set chat.contextWindow=1000000
check "chat.contextWindow is the chat's MODEL_CONTEXT_WINDOW, an integer" dhas Deployment another-agentic-chat 'value: "1000000"'
check "and it is named MODEL_CONTEXT_WINDOW" dhas Deployment another-agentic-chat 'name: MODEL_CONTEXT_WINDOW'
for bad in 0 -1 1.5 9007199254740992 '"a lot"'; do
  check "chat.contextWindow=$bad is refused" fails renders --set-json "chat.contextWindow=$bad"
done
# ADR 0059: the history read has a kill switch and bounds, written only when they are not the orchestrator's defaults
render --set orchestrator.history.windowed=false
config_of config.yaml "$cfg"
check "history off: ui.history.windowed is false and is the only key of it" sh -c "awk '/^ui:\$/ {u=1; next} u && /^[^ ]/ {u=0} u' '$cfg' | tr -d '\\n' | grep -Eq '^  showDescriptions: true  history:    windowed: false\$'"
check "history off: no server.history, the bounds stay the orchestrator's" cfg_lacks 'maxTurns|maxPageBytes'
render --set orchestrator.history.windowed=true --set orchestrator.history.initialTurns=5 --set orchestrator.history.pageTurns=10 --set orchestrator.history.maxTurns=50 --set orchestrator.history.maxPageBytes=1048576
config_of config.yaml "$cfg"
check "history counts: ui.history keeps the switch on and names the counts" cfg_all '^  history:$' '^    windowed: true$' '^    initialTurns: 5$' '^    pageTurns: 10$'
check "history bounds: server.history names both, as integers" cfg_all '^  history:$' '^    maxTurns: 50$' '^    maxPageBytes: 1048576$'
render --set orchestrator.history.maxPageBytes=65536
config_of config.yaml "$cfg"
check "a bound alone writes server.history and no ui.history" sh -c "grep -Eq '^    maxPageBytes: 65536\$' '$cfg' && ! grep -Eq 'windowed' '$cfg'"
for bad in 'windowed="false"' 'windowed=0' 'initialTurns=0' 'initialTurns=101' 'pageTurns=1.5' 'pageTurns="a few"' 'maxTurns=0' 'maxTurns=1001' 'maxPageBytes=1024' 'maxPageBytes=67108865'; do
  check "orchestrator.history.$bad is refused" fails renders --set-json "orchestrator.history.$bad"
done
# a key Helm dropped (a null in the values; Argo CD's Helm drops the chart's own nulls too) is the orchestrator's default, not an error
null_counts=$(mktemp)
printf 'orchestrator:\n  history:\n    initialTurns: null\n    pageTurns: null\n    maxTurns: null\n    maxPageBytes: null\n' > "$null_counts"
check "history counts and bounds dropped by Helm are the orchestrator's defaults" renders -f "$null_counts"
render -f "$null_counts"
config_of config.yaml "$cfg"
check "history counts dropped by Helm write no history key" cfg_lacks 'history'
rm -f "$null_counts"
check "a first page above the largest page is refused" fails renders --set orchestrator.history.initialTurns=60 --set orchestrator.history.maxTurns=50
check "a first page above the default largest page is refused" fails renders --set orchestrator.history.pageTurns=100 --set orchestrator.history.maxTurns=99
check "a first page at the largest page is accepted" renders --set orchestrator.history.initialTurns=50 --set orchestrator.history.maxTurns=50
render --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c' --set 'agents[0].tokenEnv=CODER_A2A_TOKEN'
check "without the chat agent: no chat Deployment, database or ExternalSecret" lacks 'another-agentic-chat'
check "without the chat agent the other four pods remain" count '^kind: Deployment$' 4
render --set externalSecrets.enabled=false
check "without ExternalSecrets none is rendered (and no Secret either)" lacks '^kind: (ExternalSecret|Secret)$'
render --set networkPolicy.enabled=false
check "NetworkPolicies can be turned off" lacks '^kind: NetworkPolicy$'
render --set networkPolicy.ingressControllerNamespace=kube-system
check "the edge can be limited to the ingress controller's namespace" dhas NetworkPolicy another-agentic-edge 'kubernetes.io/metadata.name: kube-system'
render --set model.baseUrl= --set chat.enabled=false --set 'orchestrator.tasks.title.model=' --set 'orchestrator.tasks.description.model=' --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c' --set 'agents[0].tokenEnv=CODER_A2A_TOKEN'
config_of config.yaml "$cfg"
check "with no model there is no models section, no model key mounted and no model-api-key" lacks 'model-api-key|model_api_key'
check "with no model the configuration has no tasks" cfg_lacks '^(models|tasks):'
render --set 'orchestrator.tasks.title.model=' --set 'orchestrator.tasks.description.model='
config_of config.yaml "$cfg"
check "a model with no task model: the configuration has models and no tasks key (an empty one is null, refused)" sh -c "grep -Eq '^models:' '$cfg' && ! grep -Eq '^tasks:' '$cfg'"
render --set 'orchestrator.tasks.title.model=title' --set 'orchestrator.tasks.description.model='
config_of config.yaml "$cfg"
check "a title model alone: tasks has the title and no description" cfg_all '^tasks:$' '^  title: \{ endpoint: default, model: "title" \}$'
check "a title model alone: no description task" cfg_lacks '^  description:'
render
config_of config.yaml "$cfg"

# ---- The model's address from the AWS secret (model.baseUrlFromSecret): off by default, on by values -----------------------
# Off (the default render, which the pinned orchestrator image reads): the address is the value, in the configuration and in
# the chat agent's environment, and no property of the AWS secret is read for it.
chat_base_url_literal() { doc Deployment another-agentic-chat | awk '/- name: MODEL_BASE_URL$/ { getline; if ($0 ~ /^ *value: /) found = 1 } END { exit found ? 0 : 1 }'; }
chat_base_url_secret() { doc Deployment another-agentic-chat | awk '/- name: MODEL_BASE_URL$/ { getline; if ($0 ~ /^ *valueFrom:/) { getline; getline; getline; if ($0 ~ /key: MODEL_BASE_URL$/) found = 1 } } END { exit found ? 0 : 1 }'; }
render
config_of config.yaml "$cfg"
check "values.yaml: model.baseUrlFromSecret is false by default" sh -c "awk '/^model:/{m=1} m && /^  baseUrlFromSecret:/{print \$2; exit}' \"$chart/values.yaml\" | grep -qx false"
check "address off (default): the configuration writes the address as text" cfg_has '^      baseUrl: "https://gateway.example.invalid/v1"$'
check "address off (default): no { file } reference to an address anywhere in the configuration" cfg_lacks 'model-base-url'
check "address off (default): the chat agent's MODEL_BASE_URL is a literal value" chat_base_url_literal
check "address off (default): nothing of the address's property in the render" lacks 'model_base_url|model-base-url'
check "address off (default): the orchestrator's ExternalSecret reads model_api_key alone for the model" dhas ExternalSecret another-agentic-orchestrator 'property: model_api_key$'

# On: no address in the values, so none in the render; the orchestrator reads a file, the chat agent a Secret.
render --set model.baseUrl= --set model.baseUrlFromSecret=true
config_of config.yaml "$cfg"
check "address on: the configuration's baseUrl is a { file } reference under the orchestrator's secrets" cfg_has '^      baseUrl: \{ file: /run/secrets/orchestrator/model-base-url \}$'
check "address on: the key is still a { file } reference, the endpoint is still the default and the tasks follow" cfg_all '^    default:$' '^      apiKey: \{ file: /run/secrets/orchestrator/model-api-key \}$' '^  title: \{ endpoint: default, model: "title" \}$'
check "address on: no address written in the render (the netcup placeholder is not there, and no baseUrl is quoted text)" sh -c "! grep -Eq 'gateway.example.invalid|baseUrl: \"' '$out'"
check "address on: the chat agent's MODEL_BASE_URL comes from its own Secret, key MODEL_BASE_URL" chat_base_url_secret
check "address on: the chat agent has no literal MODEL_BASE_URL" fails chat_base_url_literal
check "address on: no secret-named variable has a literal value" fails literal_secret_env
check "address on: every secret key of the config is still a reference" test -z "$(plain_secret_in_config)"
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "address on: the orchestrator's ExternalSecret reads model_base_url as the key model-base-url, beside the API key" sec_all 'secretKey: model-base-url$' 'property: model_base_url$' 'secretKey: model-api-key$' 'property: model_api_key$'
doc ExternalSecret another-agentic-chat > "$sec"
check "address on: the chat agent's ExternalSecret reads the same property as MODEL_BASE_URL" sec_all 'secretKey: MODEL_BASE_URL$' 'property: model_base_url$' 'secretKey: MODEL_API_KEY$'
check "address on: both ExternalSecrets read model_base_url from the AWS secret, ssegning-aws, and no other does" sh -c "
  [ \"\$(grep -Ec 'property: model_base_url\$' '$out')\" -eq 2 ] && [ \"\$(grep -Ec '^kind: ExternalSecret\$' '$out')\" -eq 4 ]"
doc Deployment another-agentic-orchestrator > "$sec"
check "address on: the orchestrator mounts the address as a file of its Secret, beside the key" sec_all 'key: model-base-url$' 'path: model-base-url$' 'path: model-api-key$'
check "address on: still no Secret object, still the production configuration" sh -c "
  ! grep -Eq '^kind: Secret\$' '$out' && grep -Eq '^  environment: production\$' '$cfg' && grep -Eq '^  mode: jwt\$' '$cfg'"
check "address on: every image is ours by commit or a tag with a digest" images_ok
check "address on: the property is a value (a rename is a values change)" sh -c "
  helm template x '$chart' -n a -f '$base' --set model.baseUrl= --set model.baseUrlFromSecret=true --set externalSecrets.properties.modelBaseUrl=other_url | grep -Ec 'property: other_url\$' | grep -qx 2"
check "address on with the chat agent off: the orchestrator alone reads it" sh -c "
  helm template x '$chart' -n a -f '$base' --set model.baseUrl= --set model.baseUrlFromSecret=true --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c' --set 'agents[0].tokenEnv=CODER_A2A_TOKEN' | grep -Ec 'property: model_base_url\$' | grep -qx 1"
check "address on, ExternalSecrets off: none is rendered, the Deployments still name their Secrets" sh -c "
  helm template x '$chart' -n a -f '$base' --set model.baseUrl= --set model.baseUrlFromSecret=true --set externalSecrets.enabled=false > '$sec' &&
  ! grep -Eq '^kind: ExternalSecret\$' '$sec' && grep -Eq 'key: model-base-url\$' '$sec' && grep -Eq 'key: MODEL_BASE_URL\$' '$sec'"
render
config_of config.yaml "$cfg"

# ---- Web search and the tool servers: off by default, on by values --------------------------------------------------------
ws_values="$chart/tests/web-search.values.yaml"
ws_on="--set webSearch.enabled=true --set webSearch.image.tag=sha-abc1234"

render
config_of config.yaml "$cfg"
check "off by default: no search pod, Service, policy or Secret" lacks 'websearch|search-mcp|search_mcp|brave|searxng'
check "off by default: no Context7 anywhere in the render" lacks 'context7'
check "off by default: the configuration has no toolServers key (the pinned image need not know it)" cfg_lacks 'toolServers'
check "values.yaml pins the search image by a built sha-<7> tag (CI bumps it)" sh -c "awk '/^webSearch:/{w=1} w && /^    tag:/{print; exit}' \"$chart/values.yaml\" | grep -Eq '^    tag: sha-[0-9a-f]{7}\$'"

# The search pod alone: the tool servers stay off, so the configuration is the default one.
# shellcheck disable=SC2086
render $ws_on
config_of config.yaml "$cfg"
check "search pod on: six Deployments, six Services, six NetworkPolicies, five ExternalSecrets" sh -c "
  [ \"\$(grep -Ec '^kind: Deployment\$' '$out')\" -eq 6 ] && [ \"\$(grep -Ec '^kind: Service\$' '$out')\" -eq 6 ] &&
  [ \"\$(grep -Ec '^kind: NetworkPolicy\$' '$out')\" -eq 6 ] && [ \"\$(grep -Ec '^kind: ExternalSecret\$' '$out')\" -eq 5 ]"
check "search pod on: still no Secret object" lacks '^kind: Secret$'
check "search pod on: a Service another-agentic-websearch, ClusterIP, 8080" dhas Service another-agentic-websearch 'port: 8080'
check "search pod on: its image is ours, by commit" dhas Deployment another-agentic-websearch 'image: "ghcr.io/vymalo/another-agentic-system/searxng-mcp:sha-abc1234"'
check "search pod on: Brave is the provider" dhas Deployment another-agentic-websearch 'value: brave'
check "search pod on: no secret-named variable has a literal value (the Brave key and the bearer are secretKeyRefs)" fails literal_secret_env
doc Deployment another-agentic-websearch > "$sec"
check "search pod on: BRAVE_API_KEY and SEARCH_MCP_TOKEN come from its own Secret" sec_all 'name: BRAVE_API_KEY$' 'key: BRAVE_API_KEY$' 'name: SEARCH_MCP_TOKEN$' 'key: SEARCH_MCP_TOKEN$' 'name: another-agentic-websearch$'
check "search pod on: uid 1000, non-root, RuntimeDefault seccomp" sec_all 'runAsUser: 1000$' 'runAsNonRoot: true$' 'type: RuntimeDefault$'
check "search pod on: no escalation, all capabilities dropped, read-only root, no service account token" sec_all 'allowPrivilegeEscalation: false$' 'drop: \["ALL"\]$' 'readOnlyRootFilesystem: true$' 'automountServiceAccountToken: false$'
check "search pod on: startup, liveness and readiness probes, all on /healthz" sh -c "[ \"\$(grep -Ec 'path: /healthz\$' '$sec')\" -eq 3 ]"
check "search pod on: resource requests and a memory limit" sec_all 'requests:$' 'limits:$' 'memory: 256Mi$'
doc ExternalSecret another-agentic-websearch > "$sec"
check "search pod on: its ExternalSecret reads brave_api_key and search_mcp_token from the AWS secret, on ssegning-aws" sec_all 'property: brave_api_key$' 'property: search_mcp_token$' 'key: prod/another-agentic/env$' 'name: ssegning-aws$'
check "search pod on: the Brave key is read by that ExternalSecret alone" count 'property: brave_api_key$' 1
check "search pod on: the orchestrator's ExternalSecret reads neither the Brave key nor the bearer" dlacks ExternalSecret another-agentic-orchestrator 'brave|search'
check "search pod on, no tool server: the configuration is still the default one" cfg_lacks 'toolServers'
check "search pod on: every image is ours by commit or a tag with a digest" images_ok
doc NetworkPolicy another-agentic-websearch > "$sec"
check "search pod on: its policy covers ingress and egress" sec_all '^    - Ingress$' '^    - Egress$'
check "search pod on: ingress from the orchestrator's pods" sec_all 'app.kubernetes.io/component: orchestrator$'
check "search pod on: ingress from the coder (instance: coder), on 8080 only" sec_all 'app.kubernetes.io/instance: coder$' 'port: 8080$'
check "search pod on: not from the edge, the web or oauth2-proxy" fails sec_all 'component: (edge|web|oauth2-proxy)$'
check "search pod on: from the chat agent, whose researcher sub-agent searches (ADR 0050)" sec_all 'app.kubernetes.io/component: chat$'
# The chat's folder: the sub-agents are files of the ConfigMap, the researcher's `mcp.json` names the search pod.
doc ConfigMap another-agentic-chat-agent > "$sec2"
check "search pod on: the chat's folder has its three sub-agents, and the researcher's mcp.json" sec2_all '^  subagent-planner.md: \|$' '^  subagent-writer.md: \|$' '^  subagent-researcher.md: \|$' '^  subagent-researcher-mcp.json: \|$'
check "search pod on: the researcher's mcp.json is the search pod's Service on /mcp, the bearer a variable, no value" sec2_all '"url": "http://another-agentic-websearch.another-agentic-system.svc:8080/mcp"' '"Authorization": "Bearer \$\{SEARCH_MCP_TOKEN\}"'
check "search pod on: the researcher keeps its tools pattern" sec2_all 'tools: \["search__\*"\]'
doc Deployment another-agentic-chat > "$sec2"
check "search pod on: the chat has the bearer from its own Secret, and the sub-agent files at their paths" sec2_all 'name: SEARCH_MCP_TOKEN$' 'key: SEARCH_MCP_TOKEN$' 'path: subagents/planner.md }$' 'path: subagents/writer.md }$' 'path: subagents/researcher/instructions.md }$' 'path: subagents/researcher/mcp.json }$'
doc ExternalSecret another-agentic-chat > "$sec2"
check "search pod on: the chat's ExternalSecret reads search_mcp_token" sec2_all 'secretKey: SEARCH_MCP_TOKEN$' 'property: search_mcp_token$'
check "search pod on: egress to DNS, and to the public internet except private and special-purpose ranges and the metadata address" sec_all 'port: 53$' 'cidr: 0.0.0.0/0' '10.0.0.0/8' '100.64.0.0/10' '127.0.0.0/8' '169.254.0.0/16' '172.16.0.0/12' '192.0.0.0/24' '192.168.0.0/16' '198.18.0.0/15' 'port: 443$'
check "search pod on: the IPv6 exceptions include both NAT64 prefixes and 6to4" sec_all 'cidr: ::/0' 'fc00::/7' 'fe80::/10' '64:ff9b::/96' '64:ff9b:1::/48' '2002::/16'
check "search pod on: no IPv4-mapped range, which the API server refuses in an ipBlock" sh -c "! grep -q '::ffff:' '$out'"
check "search pod on: its policy is the only one that restricts egress" count '^    - Egress$' 1
# shellcheck disable=SC2086
render $ws_on --set networkPolicy.enabled=false
check "search pod on, NetworkPolicies off: none is rendered" lacks '^kind: NetworkPolicy$'
# shellcheck disable=SC2086
render $ws_on --set 'webSearch.allowFrom[0].namespaceSelector.matchLabels.kubernetes\.io/metadata\.name=agents'
doc NetworkPolicy another-agentic-websearch > "$sec"
check "search pod on: who may call it besides the orchestrator is a value (a namespace here, no longer the coder)" sh -c "
  grep -Eq 'kubernetes.io/metadata.name: agents\$' '$sec' && ! grep -Eq 'instance: coder' '$sec'"
# shellcheck disable=SC2086
render $ws_on --set 'webSearch.egressExcept={10.0.0.0/8,203.0.113.0/24}' --set 'webSearch.egressExceptV6={}'
doc NetworkPolicy another-agentic-websearch > "$sec"
check "search pod on: the egress exceptions are values (a node range is added, the IPv6 list can be emptied)" sh -c "
  grep -Eq '203.0.113.0/24\$' '$sec' && ! grep -Eq '192.168.0.0/16' '$sec' && ! grep -Eq 'fc00::/7' '$sec'"
# shellcheck disable=SC2086
render $ws_on --set externalSecrets.enabled=false
check "search pod on, ExternalSecrets off: none is rendered, the Deployment still names its Secret" sh -c "
  ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq 'name: another-agentic-websearch\$' '$out'"

# Both tool servers, through the orchestrator.
render -f "$ws_values"
config_of config.yaml "$cfg"
check "tool servers on: the configuration lists websearch and context7 under toolServers" cfg_all '^toolServers:$' 'id: websearch$' 'id: context7$'
check "tool servers on: websearch is the search pod's Service, by its name, on /mcp" cfg_has 'url: http://another-agentic-websearch.another-agentic-system.svc:8080/mcp$'
check "tool servers on: context7 is the hosted https endpoint" cfg_has 'url: https://mcp.context7.com/mcp$'
check "tool servers on: each bearer is a file under the orchestrator's secrets" cfg_all 'file: /run/secrets/orchestrator/search-mcp-token$' 'file: /run/secrets/orchestrator/context7-api-key$'
check "tool servers on: every secret key of the config is still a reference" test -z "$(plain_secret_in_config)"
check "tool servers on: no URL carries a credential, a query or a fragment" fails cfg_has 'url: https?://[^ ]*[@?#]'
check "tool servers on: the tools the relay may expose are each server's own" cfg_all '- web_search$' '- fetch$' '- resolve-library-id$' '- query-docs$'
check "tool servers on: an icon is a data URI, never a URL" sh -c "grep -Eq 'icon: data:image/svg\+xml;base64,' '$cfg' && ! grep -Eq 'icon: https?:' '$cfg'"
check "tool servers on: the agents they are offered to are listed" cfg_all '- chat$' '- coder$'
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "tool servers on: the orchestrator's ExternalSecret reads search_mcp_token and context7_api_key" sec_all 'property: search_mcp_token$' 'property: context7_api_key$'
check "tool servers on: ... as the keys search-mcp-token and context7-api-key, which it mounts as files" sec_all 'secretKey: search-mcp-token$' 'secretKey: context7-api-key$'
doc Deployment another-agentic-orchestrator > "$sec"
check "tool servers on: the search pod's bearer is one property read by the orchestrator, the search pod and the chat agent" count 'property: search_mcp_token$' 3
check "tool servers on: the Brave key is still read by the search pod alone" count 'property: brave_api_key$' 1
check "tool servers on: the orchestrator mounts both keys as files" sec_all 'path: search-mcp-token$' 'path: context7-api-key$'
check "tool servers on: and passes no key as a variable (the agents' bearers are the only ones)" fails sec_all 'name: (SEARCH_MCP_TOKEN|CONTEXT7_API_KEY|BRAVE_API_KEY)$' 
check "tool servers on: no secret-named variable has a literal value" fails literal_secret_env
check "tool servers on: no token-looking value" lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
check "tool servers on: still the production configuration, fail closed" cfg_all '^  environment: production$' '^  mode: jwt$' '^  defaultRole: null$'
check "tool servers on: the roles still hold thread.delete (ADR 0043)" sh -c "[ \"\$(grep -Ec '^ +- thread\\.delete\$' \"$cfg\")\" -eq 2 ]"
check "tool servers on: still no MCP or webhook surface" cfg_lacks '^    - (mcp|webhook-generic|webhook-github)$'
# Context7 alone: no search pod needed, and no search key anywhere.
render --set orchestrator.toolServers.context7.enabled=true
config_of config.yaml "$cfg"
check "Context7 alone: its server in the configuration, no websearch" sh -c "grep -Eq 'id: context7\$' '$cfg' && ! grep -Eq 'websearch' '$cfg'"
check "Context7 alone: nothing of the search pod or its key in the render" lacks 'websearch|search-mcp|search_mcp|brave'
check "Context7 alone: its key is read by the orchestrator's ExternalSecret, still four ExternalSecrets" sh -c "
  [ \"\$(grep -Ec 'property: context7_api_key\$' '$out')\" -eq 1 ] && [ \"\$(grep -Ec '^kind: ExternalSecret\$' '$out')\" -eq 4 ]"
# The properties and a server's settings are values.
render -f "$ws_values" --set externalSecrets.properties.searchMcpToken=other_token --set externalSecrets.properties.context7ApiKey=other_c7 --set externalSecrets.properties.braveApiKey=other_brave
check "the three new AWS properties are values (a rename is a values change)" out_all 'property: other_token$' 'property: other_c7$' 'property: other_brave$'
render --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.url=https://context7.example.org/mcp --set 'orchestrator.toolServers.context7.agents={chat}' --set orchestrator.toolServers.context7.timeoutSecs=30
config_of config.yaml "$cfg"
check "a tool server's URL, agents and timeout are values" cfg_all 'url: https://context7.example.org/mcp$' 'timeoutSecs: 30$'
check "tool server timeouts of 1 and 600 are accepted" sh -c "
  helm template x '$chart' -n a -f '$base' --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.timeoutSecs=1 >/dev/null &&
  helm template x '$chart' -n a -f '$base' --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.timeoutSecs=600 >/dev/null"
check "a disabled tool server's settings are not checked (nothing of it is written)" renders --set orchestrator.toolServers.context7.timeoutSecs=900
render
config_of config.yaml "$cfg"

# ---- Refusals: what _validate.tpl stops ------------------------------------------------------------------------------
refused() { # refused <description> <helm args...>
  desc=$1; shift
  if renders "$@"; then echo "FAIL refused: $desc"; fail=1; else echo "ok   refused: $desc"; fi
}
nohost=$(mktemp); printf 'auth:\n  issuer: https://auth.example.org/realms/x\n' > "$nohost"
if helm template x "$chart" -f "$nohost" >/dev/null 2>&1; then echo "FAIL refused: no host"; fail=1; else echo "ok   refused: no host"; fi
rm -f "$nohost"
refused "a host with a scheme" --set host=https://agentic.example.org
refused "no issuer" --set auth.issuer=
refused "a plain http issuer" --set auth.issuer=http://auth.example.org/realms/x
refused "an issuer with a trailing slash" --set auth.issuer=https://auth.example.org/realms/x/
refused "no client id" --set auth.clientId=
refused "a role that reads any thread" --set auth.roles.admin.scope=any
refused "a role that reads any thread, in the long form" --set auth.roles.admin.scope.read=any
refused "no roles claim" --set auth.rolesClaim=
refused "an MCP surface" --set 'orchestrator.surfaces={agui,mcp}'
refused "a webhook surface" --set 'orchestrator.surfaces={agui,webhook-github}'
refused "no agui surface" --set 'orchestrator.surfaces={thread-tools}'
refused "a first-party image tag that is latest" --set orchestrator.image.tag=latest
refused "a first-party image tag that is not a commit" --set web.image.tag=v1.2.3
refused "a third-party image without a digest" --set edge.image.digest=
refused "a third-party image with a short digest" --set oauth2Proxy.image.digest=sha256:abc
refused "the chat agent without a model name" --set chat.model=
refused "the chat agent without a model endpoint" --set model.baseUrl=
refused "the address both written and kept in the AWS secret" --set model.baseUrlFromSecret=true
refused "the address from the AWS secret with no property for it" --set model.baseUrl= --set model.baseUrlFromSecret=true --set externalSecrets.properties.modelBaseUrl=
refused "model.baseUrlFromSecret as a string (the string false would be on)" --set model.baseUrl= --set-string model.baseUrlFromSecret=false
refused "a title model with neither an address nor the secret" --set model.baseUrl= --set model.baseUrlFromSecret=false --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c'
check "the address from the AWS secret with ExternalSecrets off needs no property name (the Secrets are the deployment's)" renders --set model.baseUrl= --set model.baseUrlFromSecret=true --set externalSecrets.enabled=false --set externalSecrets.properties.modelBaseUrl=
check "the address from the AWS secret is accepted alone (the refusals above are the two ways it is not)" renders --set model.baseUrl= --set model.baseUrlFromSecret=true
refused "a title model without an endpoint" --set model.baseUrl= --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c'
refused "an agent whose bearer has no AWS property" --set 'agents[1].tokenEnv=NOPE_TOKEN'
refused "no agents" --set 'agents=null'
refused "no AWS secret" --set externalSecrets.key=
refused "no database" --set database.instances=0
refused "the search pod with the placeholder image tag (no image has been built)" --set webSearch.enabled=true --set webSearch.image.tag=sha-0000000
refused "the search pod on a tag that is not a commit" --set webSearch.enabled=true --set webSearch.image.tag=latest
refused "the search pod with no AWS property for the Brave key" --set webSearch.enabled=true --set webSearch.image.tag=sha-abc1234 --set externalSecrets.properties.braveApiKey=
refused "the search pod with no AWS property for the bearer" --set webSearch.enabled=true --set webSearch.image.tag=sha-abc1234 --set externalSecrets.properties.searchMcpToken=
refused "the websearch tool server without the search pod" --set orchestrator.toolServers.websearch.enabled=true
refused "the websearch tool server with no property for its bearer" -f "$ws_values" --set externalSecrets.properties.searchMcpToken=
refused "the Context7 tool server with no property for its key" --set orchestrator.toolServers.context7.enabled=true --set externalSecrets.properties.context7ApiKey=
refused "the Context7 tool server over plain http (the key would travel in clear)" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.url=http://mcp.context7.com/mcp
refused "a Context7 URL with a credential in it" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.url=https://user:key@mcp.context7.com/mcp
refused "a Context7 URL with a query" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.url=https://mcp.context7.com/mcp?key=x'
refused "a tool server without the thread-tools surface (the relay is one of its providers)" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.surfaces={agui}'
# What the pinned orchestrator refuses at startup (exit 78, an outage with Recreate) is refused by the render.
refused "a tool server offered to an agent that is not in agents (the orchestrator exits 78)" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.agents={researcher}'
refused "a tool server's agent listed twice" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.agents={chat,chat}'
refused "a tool server timeout of 900 s (1 to 600)" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.timeoutSecs=900
refused "a tool server timeout of 0 (not silently dropped)" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.timeoutSecs=0
refused "a fractional tool server timeout" --set orchestrator.toolServers.context7.enabled=true --set-json orchestrator.toolServers.context7.timeoutSecs=1.5
refused "a tool server with an empty name" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.name=
refused "a tool server with a blank name" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.name=" "
refused "a tool server with a name over 80 characters" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.name=0123456789012345678901234567890123456789012345678901234567890123456789012345678901
refused "a tool name that starts with an underscore" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.tools={_x}'
refused "a tool name the relay cannot expose" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.tools={a b}'
refused "a tool listed twice" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.tools={query-docs,query-docs}'
refused "a tool server icon at a URL (never fetched)" --set orchestrator.toolServers.context7.enabled=true --set orchestrator.toolServers.context7.icon=https://example.org/icon.svg
refused "a tool server icon that is not base64" --set orchestrator.toolServers.context7.enabled=true --set 'orchestrator.toolServers.context7.icon=data:image/svg+xml;base64,not base64!'

# ---- One database cluster, three databases (README.md, "One database cluster") --------------------------------------------
nkind() { [ "$(grep -Ec "^kind: $1\$" "$out")" -eq "$2" ]; }
# Default (the netcup values): the orchestrator's Cluster, the chat agent's role and database, no coder.
render
doc Cluster another-agentic-db > "$sec"
check "db: the Cluster is the orchestrator's, with its own database and owner kept" sec_all '^  name: another-agentic-db$' '^      database: orchestrator$' '^      owner: orchestrator$'
check "db: no second cluster for the chat agent (the old another-agentic-chat-db is gone)" lacks 'chat-db'
check "db: one managed role, agent, with login, its password from the Secret another-agentic-db-agent" sec_all '^      - name: agent$' '^        login: true$' '^          name: another-agentic-db-agent$'
check "db: no coder role while sharedDatabase.coder is off" fails grep -Eq 'name: coder$' "$sec"
db_default() {
  nkind Database 1 && dhas Database another-agentic-db-agent '^  name: agent$' && dhas Database another-agentic-db-agent '^  owner: agent$' &&
    dhas Database another-agentic-db-agent '^    name: another-agentic-db$'
}
check "db: one Database object (agent, owned by agent, on the orchestrator's cluster)" db_default
doc ExternalSecret another-agentic-db-agent > "$sec"
check "db: the agent role's ExternalSecret makes a basic-auth Secret CNPG reloads, from the AWS property agent_db_password" sec_all \
  '^    name: another-agentic-db-agent$' '^      type: kubernetes.io/basic-auth$' '^          cnpg.io/reload: "true"$' '^    - secretKey: password$' '^        property: agent_db_password$'
check "db: the URI is templated from the password and the cluster's read-write Service, never written" sec_all \
  '^        uri: "postgresql://agent:\{\{ \.password \| urlquery \}\}@another-agentic-db-rw\.another-agentic-system\.svc:5432/agent"$' '^        password: "\{\{ \.password \}\}"$'
check "db: no connection URI in the render carries a literal password" lacks 'postgresql://[A-Za-z0-9_-]+:[^{ ]'
check "db: no coder Secret, role or database by default" lacks 'coder-db|^      - name: coder$|db-coder'
check "db: the orchestrator still reads its own database from the CNPG app Secret" dhas Deployment another-agentic-orchestrator 'secretName: another-agentic-db-app'
chat_database_value() { awk '/^chat:/ { c = 1; next } c && /^[^ #]/ { c = 0 } c && /^  database:/ { f = 1 } END { exit f ? 0 : 1 }' "$chart/values.yaml"; }
check "db: values.yaml has no chat.database (the chat agent has no cluster, size or storage class of its own)" fails chat_database_value
render -f "$chart/tests/coder-db.values.yaml"
doc Cluster another-agentic-db > "$sec"
db_two_roles() {
  [ "$(grep -Ec '^      - name: (agent|coder)$' "$sec")" -eq 2 ] && [ "$(grep -Ec '^        login: true$' "$sec")" -eq 2 ] &&
    grep -Eq '^          name: another-agentic-db-agent$' "$sec" && grep -Eq '^          name: coder-db-uri$' "$sec"
}
check "db, coder on: two managed roles, agent and coder, each with login and its own password Secret" db_two_roles
db_two_databases() {
  nkind Database 2 && [ "$(grep -Ec '^    name: another-agentic-db$' "$out")" -eq 2 ] &&
    dhas Database another-agentic-db-coder '^  name: coder$' && dhas Database another-agentic-db-coder '^  owner: coder$'
}
check "db, coder on: two Database objects, one per role, on the one cluster" db_two_databases
doc ExternalSecret coder-db-uri > "$sec"
check "db, coder on: the coder's Secret coder-db-uri has the key uri to the database coder, from the AWS property coder_db_password" sec_all \
  '^    name: coder-db-uri$' '^        username: coder$' '^        uri: "postgresql://coder:\{\{ \.password \| urlquery \}\}@another-agentic-db-rw\.another-agentic-system\.svc:5432/coder"$' '^        property: coder_db_password$'
check "db, coder on: still no literal password and no Secret object" sh -c "! grep -Eq 'postgresql://[A-Za-z0-9_-]+:[^{ ]|^kind: Secret\$' '$out'"
check "db, coder on: still one Cluster object (three databases, one cluster)" nkind Cluster 1
render -f "$chart/tests/coder-db.values.yaml" --set sharedDatabase.coder.secretName=coder-pg --set externalSecrets.properties.coderDbPassword=other_pw
check "db, coder on: the Secret's name and the AWS property are values" dhas ExternalSecret coder-pg 'property: other_pw$'
render -f "$chart/tests/coder-db.values.yaml" --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c' --set 'agents[0].tokenEnv=CODER_A2A_TOKEN'
check "db, coder on and the chat agent off: only the coder's role and database remain" sh -c "
  [ \"\$(grep -Ec '^kind: Database\$' '$out')\" -eq 1 ] && ! grep -Eq 'name: agent\$|db-agent' '$out' && grep -Eq '^      - name: coder\$' '$out'"
render --set externalSecrets.enabled=false -f "$chart/tests/coder-db.values.yaml"
check "db, ExternalSecrets off: the roles and databases remain, the Secrets are the deployment's own" sh -c "
  [ \"\$(grep -Ec '^kind: Database\$' '$out')\" -eq 2 ] && ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq '^          name: coder-db-uri\$' '$out'"
render

# ---- Several coders, one per GitHub owner (README.md, "Several coders, one per GitHub owner") -----------------------------
# Backward compatibility, proven two ways. (1) The documents that make the databases (the Cluster, each Database, each database
# Secret's ExternalSecret) of the older values are the ones tests/golden/*.yaml hold, rendered from the chart as it was before
# `sharedDatabase.coders` existed (origin/main at e5da0a4): `sharedDatabase.coder.enabled` still renders exactly as before. A golden is
# made with `helm template` of that chart through tests/golden/db-objects.sh; it moves only when those documents are meant to.
db_objects() { sh "$chart/tests/golden/db-objects.sh" < "$out"; }
golden_default() { db_objects | cmp -s - "$chart/tests/golden/db-default.yaml"; }
golden_coder() { db_objects | cmp -s - "$chart/tests/golden/db-coder.yaml"; }
render
check "coders, older values: with sharedDatabase off, the database documents are the golden's, byte for byte" golden_default
render -f "$chart/tests/coder-db.values.yaml"
check "coders, older values: with sharedDatabase.coder.enabled, the database documents are the golden's, byte for byte" golden_coder
cp "$out" "$out.legacy"
# (2) The new form of the same thing is the same render: the whole of it, not only the databases.
render --set 'sharedDatabase.coders[0].name=coder'
check "coders: a list of one entry named coder (secretName and property defaulted) renders the whole chart as sharedDatabase.coder.enabled does" cmp -s "$out" "$out.legacy"
render --set 'sharedDatabase.coders[0].name=coder' --set 'sharedDatabase.coders[0].secretName=coder-pg' --set 'sharedDatabase.coders[0].passwordProperty=other_pw'
check "coders: an entry names its Secret and its AWS property" dhas ExternalSecret coder-pg 'property: other_pw$'
rm -f "$out.legacy"

# Two coders: two roles, two Databases, two Secrets, each with its own name, password property and token.
render -f "$chart/tests/coders.values.yaml"
doc Cluster another-agentic-db > "$sec"
two_roles() {
  [ "$(grep -Ec '^      - name: ' "$sec")" -eq 3 ] && [ "$(grep -Ec '^        login: true$' "$sec")" -eq 3 ] &&
    sec_all '^      - name: agent$' '^      - name: codervymalo$' '^      - name: coderstephane$' \
      '^          name: another-agentic-db-agent$' '^          name: coder-vymalo-db-uri$' '^          name: coder-stephane-db-uri$'
}
check "coders, two: three managed roles (agent and the two coders), each with login and a password Secret of its own" two_roles
check "coders, two: still one Cluster, and the databases are the chat agent's and the two coders'" sh -c "
  [ \"\$(grep -Ec '^kind: Cluster\$' '$out')\" -eq 1 ] && [ \"\$(grep -Ec '^kind: Database\$' '$out')\" -eq 3 ]"
two_databases() {
  for n in codervymalo coderstephane; do
    dhas Database "another-agentic-db-$n" "^  name: $n\$" && dhas Database "another-agentic-db-$n" "^  owner: $n\$" &&
      dhas Database "another-agentic-db-$n" '^    name: another-agentic-db$' || return 1
  done
}
check "coders, two: a Database per coder (object another-agentic-db-<name>), owned by its role, on the one cluster" two_databases
doc ExternalSecret coder-vymalo-db-uri > "$sec"
check "coders, two: the first coder's Secret has its own name, role, database and AWS property" sec_all \
  '^    name: coder-vymalo-db-uri$' '^      type: kubernetes.io/basic-auth$' '^          cnpg.io/reload: "true"$' '^        username: codervymalo$' \
  '^        uri: "postgresql://codervymalo:\{\{ \.password \| urlquery \}\}@another-agentic-db-rw\.another-agentic-system\.svc:5432/codervymalo"$' '^        property: coder_vymalo_db_password$'
doc ExternalSecret coder-stephane-db-uri > "$sec"
check "coders, two: the second coder's Secret has its own name, role, database and AWS property" sec_all \
  '^    name: coder-stephane-db-uri$' '^        username: coderstephane$' \
  '^        uri: "postgresql://coderstephane:\{\{ \.password \| urlquery \}\}@another-agentic-db-rw\.another-agentic-system\.svc:5432/coderstephane"$' '^        property: coder_stephane_db_password$'
check "coders, two: no literal password in a URI and no Secret object" sh -c "! grep -Eq 'postgresql://[A-Za-z0-9_-]+:[^{ ]|^kind: Secret\$' '$out'"
check "coders, two: the two coders' passwords are two different AWS properties" sh -c "
  [ \"\$(grep -Ec '^        property: (coder_vymalo_db_password|coder_stephane_db_password)\$' '$out')\" -eq 2 ]"
check "coders, two: each coder has an A2A token of its own, in the orchestrator's Secret and in its environment" sh -c "
  [ \"\$(grep -Ec '^    - secretKey: CODER_(VYMALO|STEPHANE)_A2A_TOKEN\$' '$out')\" -eq 2 ] &&
  grep -Eq '^        property: coder_vymalo_a2a_token\$' '$out' && grep -Eq '^        property: coder_stephane_a2a_token\$' '$out' &&
  [ \"\$(grep -Ec '^            - name: CODER_(VYMALO|STEPHANE)_A2A_TOKEN\$' '$out')\" -eq 2 ]"
config_of agents.yaml "$cfg"
check "coders, two: the agents file names both coders by card URL and token variable, chat first (the default agent)" sh -c "
  [ \"\$(grep -Ec '^  *- cardUrl: |^- cardUrl: ' '$cfg')\" -eq 3 ] && grep -Eq 'cardUrl: http://coder-vymalo\.another-agentic-system\.svc:8080/' '$cfg' &&
  grep -Eq 'cardUrl: http://coder-stephane\.another-agentic-system\.svc:8080/' '$cfg' && grep -Eq 'tokenEnv: CODER_VYMALO_A2A_TOKEN' '$cfg' &&
  grep -Eq 'tokenEnv: CODER_STEPHANE_A2A_TOKEN' '$cfg' && awk '/id: /{print \$NF; exit}' '$cfg' | grep -qx chat"
config_of config.yaml "$cfg"
check "coders, two: the roles name the coders: user holds chat and researcher, each coder role only its coder" sh -c "
  awk '/^    coder-vymalo:/{f=1;next} f&&/^    [^ ]/{f=0} f' '$cfg' | grep -Eq '^ +- coder-vymalo\$' &&
  awk '/^    coder-stephane:/{f=1;next} f&&/^    [^ ]/{f=0} f' '$cfg' | grep -Eq '^ +- coder-stephane\$' &&
  ! awk '/^    user:/{f=1;next} f&&/^    [^ ]/{f=0} f' '$cfg' | grep -Eq 'coder|\"\\*\"'"
check "coders, two: every image is pinned" images_ok
check "coders, two: no secret-named variable has a literal value" fails literal_secret_env
render --set externalSecrets.enabled=false -f "$chart/tests/coders.values.yaml"
check "coders, two, ExternalSecrets off: the roles and databases remain, the Secrets are the deployment's own" sh -c "
  [ \"\$(grep -Ec '^kind: Database\$' '$out')\" -eq 3 ] && ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq '^          name: coder-vymalo-db-uri\$' '$out' &&
  grep -Eq '^          name: coder-stephane-db-uri\$' '$out'"
render -f "$chart/tests/coders.values.yaml" --set chat.enabled=false --set-json 'agents=[{"id":"coder-vymalo","name":"V","cardUrl":"http://coder-vymalo.x.svc:8080/c","tokenEnv":"CODER_VYMALO_A2A_TOKEN"}]'
check "coders, two, no chat agent: the coders' roles and databases remain, the agent's go" sh -c "
  [ \"\$(grep -Ec '^kind: Database\$' '$out')\" -eq 2 ] && ! grep -Eq 'name: agent\$|db-agent' '$out' && grep -Eq '^      - name: codervymalo\$' '$out'"
render
check "coders: none by default (the render has no coder role, database or Secret)" lacks 'coder-db|coder-vymalo|^      - name: coder|db-coder'

refused "sharedDatabase.coder.enabled beside sharedDatabase.coders" -f "$chart/tests/coders.values.yaml" --set sharedDatabase.coder.enabled=true
refused "a coder named agent (the chat agent's database and role)" --set 'sharedDatabase.coders[0].name=agent' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a coder named like the orchestrator's database" --set 'sharedDatabase.coders[0].name=orchestrator' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a coder named postgres" --set 'sharedDatabase.coders[0].name=postgres' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a coder named twice" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p1' --set 'sharedDatabase.coders[1].name=coderone' --set 'sharedDatabase.coders[1].passwordProperty=p2' --set 'sharedDatabase.coders[1].secretName=other-uri'
refused "a coder with no name" --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a name with a hyphen (not a Postgres identifier without quotes)" --set 'sharedDatabase.coders[0].name=coder-one' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a name with an underscore (not a DNS label)" --set 'sharedDatabase.coders[0].name=coder_one' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a name with a capital" --set 'sharedDatabase.coders[0].name=Coder' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a name that starts with a digit" --set 'sharedDatabase.coders[0].name=1coder' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a name that is too long for the Database object" --set 'sharedDatabase.coders[0].name=codercodercodercodercodercodercodercodercodercodercodercoder' --set 'sharedDatabase.coders[0].passwordProperty=p'
refused "a name that is a number" --set-json 'sharedDatabase.coders=[{"name":5,"passwordProperty":"p"}]'
refused "a coder other than coder with no AWS property for its password" --set 'sharedDatabase.coders[0].name=coderone'
refused "two coders on one Secret name" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p1' --set 'sharedDatabase.coders[0].secretName=same-uri' --set 'sharedDatabase.coders[1].name=codertwo' --set 'sharedDatabase.coders[1].passwordProperty=p2' --set 'sharedDatabase.coders[1].secretName=same-uri'
refused "two coders on one AWS property (each role has a password of its own)" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=same' --set 'sharedDatabase.coders[1].name=codertwo' --set 'sharedDatabase.coders[1].passwordProperty=same'
refused "a coder on the chat agent's password property" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=agent_db_password'
refused "a coder Secret named like the chat agent's" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p' --set 'sharedDatabase.coders[0].secretName=another-agentic-db-agent'
refused "a coder Secret name that is not a Secret name" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p' --set 'sharedDatabase.coders[0].secretName=Coder_DB'
refused "a key that is a typo (it would be ignored)" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProp=p'
refused "an entry that is not a map" --set 'sharedDatabase.coders={coderone}'
refused "sharedDatabase.coders as a map" --set 'sharedDatabase.coders.name=coderone'
check "a coder with ExternalSecrets off needs no AWS property (the Secrets are the deployment's)" renders --set externalSecrets.enabled=false --set 'sharedDatabase.coders[0].name=coderone'
check "the entry named coder takes externalSecrets.properties.coderDbPassword by default" renders --set 'sharedDatabase.coders[0].name=coder'
refused "the entry named coder with that property emptied" --set 'sharedDatabase.coders[0].name=coder' --set externalSecrets.properties.coderDbPassword=
check "the refusal says which entry and what is wrong (a name that is taken)" sh -c "helm template x '$chart' -n x -f '$base' --set 'sharedDatabase.coders[0].name=agent' --set 'sharedDatabase.coders[0].passwordProperty=p' 2>&1 | grep -q 'sharedDatabase.coders\[0\] (agent): the name is taken'"
check "the refusal says which entry and what is wrong (a name that is not an identifier)" sh -c "helm template x '$chart' -n x -f '$base' --set 'sharedDatabase.coders[0].name=coder-one' --set 'sharedDatabase.coders[0].passwordProperty=p' 2>&1 | grep -q 'sharedDatabase.coders\[0\] (coder-one): the name must be a Postgres identifier and a DNS label'"
check "the refusal says which entry and what is wrong (a name listed twice)" sh -c "helm template x '$chart' -n x -f '$base' --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p1' --set 'sharedDatabase.coders[1].name=coderone' --set 'sharedDatabase.coders[1].passwordProperty=p2' --set 'sharedDatabase.coders[1].secretName=o-uri' 2>&1 | grep -q 'sharedDatabase.coders\[1\] (coderone): the name is listed twice'"
check "the refusal of both keys names both" sh -c "helm template x '$chart' -n x -f '$base' -f '$chart/tests/coders.values.yaml' --set sharedDatabase.coder.enabled=true 2>&1 | grep -q 'sharedDatabase.coder.enabled and sharedDatabase.coders are both set'"

# ---- Sharing a thread (ADR 0040): off by default, internal and public by values -------------------------------------------
# role_has <role> <permission>: the permission is in that role's list in the configuration read into $cfg.
role_has() {
  awk -v r="    $1:" -v p="      - $2" '
    $0 == r { on = 1; next }
    on && /^    [^ ]/ { on = 0 }
    on && $0 == p { found = 1 }
    END { exit found ? 0 : 1 }' "$cfg"
}
T=$(printf '\t')
# The edge's Caddyfile, from its ConfigMap (the lines are indented by four spaces; Caddy's own indent is tabs).
# Comments are left out: they say what the routes are, in the words the checks look for.
caddyfile() { doc ConfigMap another-agentic-edge | sed -n 's/^    //p' | grep -Ev "^${T}*#"; }
n_handles() { caddyfile | grep -Ec "^${T}handle " || true; }
# handle_block <matcher>: one `handle` block, up to the closing brace at its own indent.
handle_block() {
  caddyfile | awk -v h="$1" '
    $0 == "\thandle " h " {" { on = 1 }
    on { print }
    on && $0 == "\t}" { on = 0 }'
}
# The one route of every edge that asks for no sign-in: the web's runtime configuration (`@runtimeConfig`); it is left out here.
edge_has_no_public_route() { ! caddyfile | awk '/^\t(@runtimeConfig|handle @runtimeConfig) \{$/ { skip = 1 } !skip { print } skip && $0 == "\t}" { skip = 0 }' | grep -Eq 'header_up -Authorization|public/shared|handle @public|/s/\*'; }
runtime_config_public() {
  b=$(handle_block @runtimeConfig)
  printf '%s\n' "$b" | grep -Fq 'another-agentic-web:3000' && printf '%s\n' "$b" | grep -Fq 'header_up -Authorization' &&
    ! printf '%s\n' "$b" | grep -Eq 'forward_auth|copy_headers|oauth2-proxy' &&
    m=$(caddyfile | awk -v t="$T" '$0 == t "@runtimeConfig {" { on = 1; next } on && $0 == t "}" { on = 0 } on { print }') &&
    printf '%s\n' "$m" | grep -Fq 'method GET HEAD' && printf '%s\n' "$m" | grep -Fq 'path /config.json'
}
sharing_off_everywhere() { ! grep -Ev '^ *#' "$out" | grep -Eq 'sharing_secret|sharing-secret|thread\.share|public/shared'; }
public_paths() { caddyfile | grep -E "^${T}${T}path " | sed "s/^${T}${T}path //" | sort | tr '\n' '|'; }
public_blocks_ok() {
  for m in @publicApi @publicAgui @publicWeb; do
    b=$(handle_block "$m")
    [ -n "$b" ] || return 1
    printf '%s\n' "$b" | grep -Fq 'header_up -Authorization' || return 1
    printf '%s\n' "$b" | grep -Fq 'header_up -X-Auth-Request-Email' || return 1
    if printf '%s\n' "$b" | grep -Eq 'forward_auth|copy_headers'; then return 1; fi
  done
}
public_first() {
  caddyfile | grep -E "^${T}handle " | awk '{ o = o $2 "|" } END { exit (o ~ /@publicApi.*@publicAgui.*@publicWeb.*\/api\/\*.*\/agui\/\*/) ? 0 : 1 }'
}
public_targets() {
  handle_block @publicApi | grep -Fq 'another-agentic-orchestrator:8080' && handle_block @publicAgui | grep -Fq 'another-agentic-orchestrator:8080' &&
    handle_block @publicWeb | grep -Fq 'another-agentic-web:3000' && ! handle_block @publicWeb | grep -Fq 'oauth2-proxy'
}
no_other_surface() { caddyfile | grep -Fq 'handle /thread-tools/*' && ! caddyfile | grep -Eq 'handle (/mcp|/webhooks)'; }
# block <matcher or empty>: one `handle` of the Caddyfile (`handle {` for the catch-all), up to its closing tab-brace.
block() {
  caddyfile | awk -v h="$1" -v t="$T" '
    $0 == t "handle " (h == "" ? "{" : h " {") { on = 1 }
    on { print }
    on && $0 == t "}" { on = 0 }'
}
# in_block <matcher> <fixed string>: the block has that text. out_of_block: it has none of the extended pattern.
in_block() { block "$1" | grep -Fq -- "$2"; }
not_in_block() { ! block "$1" | grep -Eq -- "$2"; }
# matcher_has <@name> <fixed string>: the named matcher (`@name {` ... `}`) has that line.
matcher_has() {
  caddyfile | awk -v m="$1" -v t="$T" '$0 == t m " {" { on = 1; next } on && $0 == t "}" { on = 0 } on { print }' | grep -Fq -- "$2"
}
# handles_by_auth with|without: the matchers of the `handle` blocks that do (or do not) ask oauth2-proxy (`forward_auth`), in the order of the
# file and each followed by a bar (the catch-all is `(web)`). Every route that skips the sign-in is NAMED by the checks that use it, so a route added
# to the Caddyfile later fails them until its place in the list is argued: nothing else skips the sign-in.
handles_by_auth() {
  caddyfile | awk -v want="$1" -v t="$T" '
    $0 ~ "^" t "handle " { h = ($2 == "{") ? "(web)" : $2; auth = 0; on = 1; next }
    on && $0 ~ "^" t t "forward_auth " { auth = 1 }
    on && $0 == t "}" { on = 0; if ((want == "with") == (auth == 1)) o = o h "|" }
    END { print o }'
}
# The three public blocks, from `@publicApi {` to the end of `handle @publicWeb`, with the upstream names made one: dev/Caddyfile (the edge that
# dev/share-e2e.sh runs against) and the chart's say the same, line for line.
public_region() {
  awk -v t="$T" '
    $0 == t "@publicApi {" { on = 1 }
    on { print }
    on && $0 == t "handle @publicWeb {" { web = 1 }
    on && web && $0 == t "}" { exit }'
}
chart_public_region() {
  caddyfile | grep -v '^$' | public_region | sed -e 's/another-agentic-orchestrator:8080/orchestrator:8080/' -e 's/another-agentic-web:3000/web:3000/'
}
dev_public_region() { grep -Ev "^${T}*#" "$repo/dev/Caddyfile" | grep -v '^$' | public_region; }
dev_has_the_chart_public_blocks() {
  _chart=$(chart_public_region) && [ "$(printf '%s\n' "$_chart" | grep -c .)" -gt 20 ] && [ "$_chart" = "$(dev_public_region)" ]
}
# matcher_is <@name> <line>...: the named matcher (`@name {` ... `}`) is exactly these lines, in this order.
matcher_is() {
  _name=$1
  shift
  _want=$(printf '%s\n' "$@")
  _got=$(caddyfile | awk -v m="$_name" -v t="$T" '$0 == t m " {" { on = 1; next } on && $0 == t "}" { on = 0 } on { sub("^" t t, ""); print }')
  [ "$_got" = "$_want" ]
}
public_streams_unbuffered() {
  for m in @publicApi @publicAgui; do
    in_block "$m" 'flush_interval -1' && in_block "$m" 'header_up -Authorization' && in_block "$m" 'header_up -X-Auth-Request-Email' || return 1
  done
}
public_buffering() {
  in_block @publicAgui 'request_buffers 8MiB' && not_in_block @publicApi 'request_buffers' && not_in_block @publicWeb 'request_buffers|flush_interval'
}

# Off (the default): the render has no trace of sharing.
render
config_of config.yaml "$cfg"
check "sharing off (default): the configuration has no sharing key and no role holds thread.share" cfg_lacks '^sharing:|thread.share'
check "sharing off (default): no sharing secret, permission or public route anywhere in the render" sharing_off_everywhere
check "sharing off (default): the edge has no public route (no header stripped, no /s/, no public/shared)" edge_has_no_public_route
check "sharing off (default): seven handle blocks in the Caddyfile (health, oauth2, thread-tools, config.json, api, agui, the web)" test "$(n_handles)" -eq 7
check "sharing off (default): only the health check, oauth2-proxy's own routes, the thread tools' 404 and config.json skip forward_auth" test "$(handles_by_auth without)" = '/edge-healthz|/oauth2/*|/thread-tools/*|@runtimeConfig|'
check "sharing off (default): the API, the AG-UI route and the web are what asks oauth2-proxy" test "$(handles_by_auth with)" = '/api/*|/agui/*|(web)|'
check "edge mode: /config.json (the web's runtime configuration) is read with no sign-in, GET and HEAD only, from the web" runtime_config_public
check "values.yaml: sharing.mode is disabled by default" sh -c "awk '/^sharing:/{m=1} m && /^  mode:/{print \$2; exit}' '$chart/values.yaml' | grep -qx disabled"
cp "$out" "$out.off"

# Internal: signed-in readers. A key, a secret file and the permission; no edge change (the page stays behind sign-in).
render --set sharing.mode=internal
config_of config.yaml "$cfg"
check "sharing internal: the cap is internal with its secret a { file } reference" cfg_all '^sharing:$' '^  mode: internal$' '^  secret: \{ file: /run/secrets/orchestrator/sharing-secret \}$'
check "sharing internal: no public section (the orchestrator refuses a key that does nothing)" cfg_lacks '^  public:'
roles_share_both() { role_has user thread.share && role_has admin thread.share && role_has user thread.delete && role_has admin admin; }
check "sharing internal: the roles of sharing.roles (user, admin) hold thread.share beside their other permissions" roles_share_both
check "sharing internal: every secret key of the configuration is still a reference" test -z "$(plain_secret_in_config)"
check "sharing internal: no role reads another person's thread" cfg_lacks '(scope: any|read: any|write: any)'
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "sharing internal: the orchestrator's ExternalSecret reads sharing_secret as sharing-secret, a property apart from the thread tools'" sec_all '^    - secretKey: sharing-secret$' '^        property: sharing_secret$' '^        property: thread_tools_secret$'
doc Deployment another-agentic-orchestrator > "$sec"
check "sharing internal: the orchestrator mounts it as a file (path sharing-secret)" sec_all '^              - key: sharing-secret$' '^                path: sharing-secret$'
check "sharing internal: no Secret object, no secret-named literal, still production and jwt" sh -c "
  ! grep -Eq '^kind: Secret\$' '$out' && grep -Eq '^  environment: production\$' '$cfg' && grep -Eq '^  mode: jwt\$' '$cfg'"
check "sharing internal: no secret-named environment variable has a literal value" fails literal_secret_env
check "sharing internal: the edge is the default one (the page and the API stay behind sign-in)" edge_has_no_public_route
check "sharing internal: seven handle blocks, as with sharing off" test "$(n_handles)" -eq 7
check "sharing internal: the same four handles skip forward_auth as without sharing (no public link at this cap)" test "$(handles_by_auth without)" = '/edge-healthz|/oauth2/*|/thread-tools/*|@runtimeConfig|'
render --set sharing.mode=internal --set 'sharing.roles={user}'
config_of config.yaml "$cfg"
user_only() { role_has user thread.share && ! role_has admin thread.share; }
check "sharing.roles lists who may share: admin is not given thread.share when it is left out" user_only
render --set sharing.mode=internal --set 'auth.roles.user.permissions={agent.read,agent.invoke,thread.read,thread.write,thread.share}'
config_of config.yaml "$cfg"
check "sharing: a role that already lists thread.share is not given it twice" test "$(grep -Ec '^      - thread.share$' "$cfg")" -eq 2

# Public: the page and the public API without sign-in, and exactly those.
render --set sharing.mode=public
config_of config.yaml "$cfg"
check "sharing public: the cap is public, with the public reader's step input and files off" cfg_all '^  mode: public$' '^  public:$' '^    stepIo: false$' '^    files: false$'
check "sharing public: the secret is a { file } reference" cfg_has '^  secret: \{ file: /run/secrets/orchestrator/sharing-secret \}$'
check "sharing public: every secret key of the configuration is a reference" test -z "$(plain_secret_in_config)"
check "sharing public: no token-looking value in the render, no secret-named literal" sh -c "! grep -Eq '(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{20})' '$out'"
check "sharing public: no secret-named environment variable has a literal value" fails literal_secret_env
check "sharing public: ten handle blocks: the seven, and the three public ones, no fourth" test "$(n_handles)" -eq 10
check "sharing public: the public routes are exactly the page and its files, /api/public/shared/* and /agui/public/shared/*" test "$(public_paths)" = '/agui/public/shared/*|/api/public/shared/*|/config.json|/s/* /_next/static/* /favicon.ico /icon.svg /apple-icon.png /manifest.webmanifest /brand/*|'
check "sharing public: three public blocks and config.json's, each GET and HEAD only" test "$(caddyfile | grep -Ec "^${T}${T}method GET HEAD\$")" -eq 4
check "sharing public: each public block drops the client's Authorization and X-Auth-Request-Email, and asks no sign-in" public_blocks_ok
check "sharing public: the public blocks come before /api/*, /agui/* and the catch-all (Caddy takes the first match)" public_first
check "sharing public: the three protected routes still ask oauth2-proxy (forward_auth, copy_headers: three, as without sharing)" count '^\s+copy_headers Authorization$' 3
check "sharing public: the public API goes to the orchestrator and the page to the web, nothing to oauth2-proxy" public_targets
check "sharing public: the thread tools are still not routed, nor /mcp, nor webhooks" no_other_surface
check "sharing public: ONLY the health check, oauth2-proxy's own routes, the thread tools' 404, config.json and the three public links skip forward_auth" test "$(handles_by_auth without)" = '/edge-healthz|/oauth2/*|/thread-tools/*|@runtimeConfig|@publicApi|@publicAgui|@publicWeb|'
check "sharing public: the API, the AG-UI route and the web are still what asks oauth2-proxy" test "$(handles_by_auth with)" = '/api/*|/agui/*|(web)|'
check "sharing public: @publicApi is GET and HEAD of /api/public/shared/* and of nothing else" matcher_is @publicApi 'method GET HEAD' 'path /api/public/shared/*'
check "sharing public: @publicAgui is GET and HEAD of /agui/public/shared/* and of nothing else" matcher_is @publicAgui 'method GET HEAD' 'path /agui/public/shared/*'
check "sharing public: @publicWeb is GET and HEAD of the page and the web's files and of nothing else" matcher_is @publicWeb 'method GET HEAD' 'path /s/* /_next/static/* /favicon.ico /icon.svg /apple-icon.png /manifest.webmanifest /brand/*'
check "sharing public: the public API and the stream drop both identity headers and are not buffered (flush_interval -1)" public_streams_unbuffered
check "sharing public: the stream's request is buffered like the other stream routes' (request_buffers 8MiB), the page's block has none of that" public_buffering
check "sharing public: dev/Caddyfile has the same three public blocks, line for line (the dev edge of dev/share-e2e.sh)" dev_has_the_chart_public_blocks
check "sharing public: the roles of sharing.roles hold thread.share" roles_share_both
render --set sharing.mode=public --set sharing.public.stepIo=true --set sharing.public.files=true
config_of config.yaml "$cfg"
check "sharing public: stepIo and files are values (off by default)" cfg_all '^    stepIo: true$' '^    files: true$'
render --set sharing.mode=public --set externalSecrets.enabled=false
check "sharing public, ExternalSecrets off: none rendered, the Deployment still mounts its Secret's sharing-secret" sh -c "
  ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq 'path: sharing-secret\$' '$out'"
render --set sharing.mode=public --set externalSecrets.properties.sharingSecret=other_share
check "sharing: the AWS property is a value" has 'property: other_share$'
render
check "sharing: turning it off again gives back the default render" cmp -s "$out" "$out.off"
rm -f "$out.off"

refused "sharing.mode that is not one of disabled, internal, public" --set sharing.mode=everyone
refused "sharing.mode as a boolean" --set sharing.mode=true
refused "sharing on with no AWS property for its secret" --set sharing.mode=internal --set externalSecrets.properties.sharingSecret=
refused "sharing on with no role to share" --set sharing.mode=internal --set 'sharing.roles={}'
refused "sharing on naming a role that does not exist" --set sharing.mode=internal --set 'sharing.roles={nobody}'
refused "public reader options without the public cap (the orchestrator refuses a key that does nothing)" --set sharing.mode=internal --set sharing.public.stepIo=true
refused "public reader options while sharing is disabled" --set sharing.public.files=true
refused "sharing and a role that reads any thread" --set sharing.mode=public --set auth.roles.admin.scope=any
check "sharing on with ExternalSecrets off needs no property name (the Secrets are the deployment's)" renders --set sharing.mode=internal --set externalSecrets.enabled=false --set externalSecrets.properties.sharingSecret=
refused "the chat agent with no AWS property for its database password" --set externalSecrets.properties.agentDbPassword=
refused "the coder's database with no AWS property for its password" --set sharedDatabase.coder.enabled=true --set externalSecrets.properties.coderDbPassword=
refused "sharedDatabase.coder.enabled as a string" --set-string sharedDatabase.coder.enabled=false
refused "a coder Secret name that is not a Secret name" --set sharedDatabase.coder.enabled=true --set sharedDatabase.coder.secretName=Coder_DB
check "no chat agent: its database password's property is not asked for" renders --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c' --set 'agents[0].tokenEnv=CODER_A2A_TOKEN' --set externalSecrets.properties.agentDbPassword=

# ---- oauth2-proxy's sessions in Redis (README.md, "Sessions in Redis"): cookie by default, redis by values -------------------------
redis_values="$chart/tests/redis.values.yaml"
oauth_args() { doc Deployment another-agentic-oauth2-proxy; }
redis_name=another-agentic-oauth2-redis

# Off (the default): the cookie store, as before. The counts and shapes asserted all through this file are the default render's; here
# is the absence of everything new.
render
cp "$out" "$out.cookie"
check "values.yaml: oauth2Proxy.sessionStore is cookie by default" sh -c "awk '/^oauth2Proxy:/{m=1} m && /^  sessionStore:/{print \$2; exit}' '$chart/values.yaml' | grep -qx cookie"
check "cookie (default): nothing of Redis in the render (no flag, no variable, no pod, no property, no policy)" lacks 'redis|REDIS'
check "cookie (default): oauth2-proxy has no --session-store-type flag (the cookie store is the program's default)" lacks 'session-store-type'
check "cookie (default): the session Redis's password property is not read" lacks 'oauth2_redis_password'

# On: the flags, the password from a Secret, a Redis that is a small hardened pod, a policy that lets oauth2-proxy in and nobody else.
render -f "$redis_values"
cp "$out" "$out.redis"
check "redis: oauth2-proxy is told the redis store and the Service's address, with no credential in the URL" \
  sh -c "grep -Fq -- '--session-store-type=redis' '$out' && grep -Eq -- '\"--redis-connection-url=redis://$redis_name\.another-agentic-system\.svc:6379\"\$' '$out'"
check "redis: no password on any command line (no --redis-password, no user:password@ in a URL)" sh -c "! grep -Eq -- '--redis-password|redis://[^ ]*@' '$out'"
doc Deployment another-agentic-oauth2-proxy > "$sec"
check "redis: oauth2-proxy reads OAUTH2_PROXY_REDIS_PASSWORD from its own Secret (a secretKeyRef, key of the same name)" sec_all \
  '^            - name: OAUTH2_PROXY_REDIS_PASSWORD$' '^                  name: another-agentic-oauth2-proxy$' '^                  key: OAUTH2_PROXY_REDIS_PASSWORD$'
check "redis: no secret-named environment variable has a literal value" fails literal_secret_env
check "redis: still no Secret object, no token-looking value" sh -c "! grep -Eq '^kind: Secret\$|(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{20})' '$out'"
check "redis: the only place the word requirepass is followed by a value is the printf that writes it from the environment" \
  sh -c "! grep -E 'requirepass' '$out' | grep -Ev '^ *#' | grep -Ev 'printf .requirepass \"%s\"\\\\n. \"\\\$REDIS_PASSWORD\"'"
check "redis: its ConfigMap holds no password (no requirepass, no masterauth, no user line)" dlacks ConfigMap "$redis_name" 'requirepass|masterauth|^ *user '
doc ConfigMap "$redis_name" > "$sec"
check "redis: the configuration reads the password from a file by include, bounds the memory, keeps no snapshot and no append-only file" sec_all \
  '^    include /run/redis-auth/auth.conf$' '^    maxmemory 64mb$' '^    maxmemory-policy volatile-lru$' '^    save ""$' '^    appendonly no$' '^    protected-mode yes$'
doc Deployment "$redis_name" > "$sec"
check "redis: one replica, recreated, uid 999 (the image's redis user), non-root, RuntimeDefault seccomp" sec_all \
  '^  replicas: 1$' '^    type: Recreate$' '^        runAsUser: 999$' '^        runAsNonRoot: true$' '^          type: RuntimeDefault$'
check "redis: no escalation, all capabilities dropped, read-only root, no service account token" sec_all \
  '^            allowPrivilegeEscalation: false$' '^              drop: \["ALL"\]$' '^            readOnlyRootFilesystem: true$' '^      automountServiceAccountToken: false$'
check "redis: the password is an environment variable from its own Secret (REDIS_PASSWORD and, for redis-cli, REDISCLI_AUTH), never a literal" sec_all \
  '^            - name: REDIS_PASSWORD$' '^            - name: REDISCLI_AUTH$' '^                  name: another-agentic-oauth2-redis$' '^                  key: REDIS_PASSWORD$'
# shellcheck disable=SC2016
check "redis: the server is started by a shell that writes the password to a file in memory, with no argument that holds it" sec_all \
  'umask 077' 'printf .requirepass "%s"\\n. "\$REDIS_PASSWORD" > /run/redis-auth/auth.conf' 'exec redis-server /etc/redis/redis.conf' 'medium: Memory'
check "redis: three probes, each reads PONG (redis-cli ping exits 0 when refused)" sh -c "[ \"\$(grep -Fc 'redis-cli ping | grep -q PONG' '$sec')\" -eq 3 ] && grep -Eq 'startupProbe:' '$sec' && grep -Eq 'livenessProbe:' '$sec' && grep -Eq 'readinessProbe:' '$sec'"
check "redis: resource requests and a memory limit, in the values" sec_all '^                memory: 64Mi$|^              memory: 64Mi$' '^              memory: 128Mi$'
check "redis: no persistence by default: the data directory is an emptyDir and no claim is made for it" sh -c "
  grep -Eq '^        - name: data\$' '$sec' && ! grep -Eq 'persistentVolumeClaim' '$sec' && [ \"\$(grep -Ec '^kind: PersistentVolumeClaim\$' '$out')\" -eq 1 ]"
check "redis: the image is the one pinned by tag and digest" has 'image: "redis:8.8.3-alpine@sha256:[0-9a-f]{64}"'
check "redis: every image is ours by commit or a tag with a digest, and one more image than by default (six)" sh -c "
  count() { [ \"\$(grep -Ec -- \"\$1\" '$out')\" -eq \"\$2\" ]; }; count '^ *image: ' 6"
check "redis: every image is ours by commit or a tag with a digest (the check of the default render)" images_ok
check "redis: a Service on 6379, ClusterIP; six Deployments, six Services, six NetworkPolicies, five ExternalSecrets" sh -c "
  [ \"\$(grep -Ec '^kind: Deployment\$' '$out')\" -eq 6 ] && [ \"\$(grep -Ec '^kind: Service\$' '$out')\" -eq 6 ] &&
  [ \"\$(grep -Ec '^kind: NetworkPolicy\$' '$out')\" -eq 6 ] && [ \"\$(grep -Ec '^kind: ExternalSecret\$' '$out')\" -eq 5 ]"
check "redis: the Service is the one the URL names" dhas Service "$redis_name" 'port: 6379'
check "redis: no route to it from the edge, no Ingress backend, no new public object" sh -c "
  [ \"\$(grep -Ec '^kind: Ingress\$' '$out')\" -eq 1 ] && ! grep -Eq 'type: (LoadBalancer|NodePort)' '$out' && ! grep -Eq 'redis' '$chart/files/Caddyfile'"
doc NetworkPolicy "$redis_name" > "$sec"
check "redis: its policy is ingress only, from oauth2-proxy's pods, on 6379 only" sec_all \
  '^    - Ingress$' 'app.kubernetes.io/component: oauth2-proxy$' '^          port: 6379$'
check "redis: ... and not from the edge, the web, the orchestrator or the chat agent" fails sec_all 'component: (edge|web|orchestrator|chat)$'
check "redis: ... it restricts no egress: the policy of the search pod is still the only one that would" fails sec_all '^    - Egress$'
doc ExternalSecret "$redis_name" > "$sec"
check "redis: its ExternalSecret reads the property oauth2_redis_password as REDIS_PASSWORD, from the AWS secret on ssegning-aws" sec_all \
  '^    name: another-agentic-oauth2-redis$' '^    - secretKey: REDIS_PASSWORD$' '^        property: oauth2_redis_password$' '^        key: prod/another-agentic/env$' '^    name: ssegning-aws$'
doc ExternalSecret another-agentic-oauth2-proxy > "$sec"
check "redis: oauth2-proxy's ExternalSecret reads the same property as OAUTH2_PROXY_REDIS_PASSWORD, beside the client and cookie secrets" sec_all \
  '^    - secretKey: OAUTH2_PROXY_REDIS_PASSWORD$' '^        property: oauth2_redis_password$' '^        property: oauth2_client_secret$' '^        property: oauth2_cookie_secret$'
check "redis: the password is read from AWS by those two ExternalSecrets and by no other" count 'property: oauth2_redis_password$' 2
check "redis: its pod restarts when its configuration changes or the secrets change shape (checksums)" sh -c "
  awk '/^kind: Deployment\$/{d=1} /^---\$/{d=0} d' '$out' | awk '/^  name: another-agentic-oauth2-redis\$/{r=1} r' | grep -Eq 'checksum/config:'"
check "redis: oauth2-proxy's pod and the Redis's both carry the checksum of the secrets" sh -c "
  [ \"\$(grep -Ec 'checksum/secrets:' '$out')\" -eq 4 ]"
check "redis: the password's property is a value (a rename is a values change)" sh -c "
  helm template x '$chart' -n a -f '$base' -f '$redis_values' --set externalSecrets.properties.oauth2RedisPassword=other_pw | grep -Ec 'property: other_pw\$' | grep -qx 2"
# The configuration of the orchestrator and the rest of the render are not touched.
config_of config.yaml "$cfg"
check "redis: the orchestrator's configuration is the default one (a production, jwt, fail-closed configuration)" cfg_all '^  environment: production$' '^  mode: jwt$' '^  defaultRole: null$'
check "redis: every other flag of oauth2-proxy is still there (role, secure cookie, PKCE, refresh)" out_all -- '--allowed-role=another-agentic:user' '--cookie-secure=true' '--code-challenge-method=S256' '--cookie-refresh=10m' '--cookie-expire=12h'

# Persistence is a value: the append-only file on a volume, not kept when the release goes.
render -f "$redis_values" --set oauth2Proxy.redis.persistence.enabled=true --set oauth2Proxy.redis.persistence.size=2Gi --set oauth2Proxy.redis.persistence.storageClass=fast
doc PersistentVolumeClaim another-agentic-oauth2-redis-data > "$sec"
check "redis with persistence: a claim of the size and the class in the values" sec_all '^      storage: "2Gi"$' '^  storageClassName: "fast"$'
check "redis with persistence: the pod mounts that claim as its data directory" dhas Deployment "$redis_name" 'claimName: another-agentic-oauth2-redis-data$'
doc ConfigMap "$redis_name" > "$sec"
check "redis with persistence: an append-only file, still no snapshot" sec_all '^    appendonly yes$' '^    appendfsync everysec$' '^    save ""$'
check "redis with persistence: its claim is not kept by Helm or Argo CD (a session is disposable), unlike the artifacts'" dlacks PersistentVolumeClaim another-agentic-oauth2-redis-data 'resource-policy|argocd'
render -f "$redis_values" --set oauth2Proxy.redis.maxMemory=1gb --set oauth2Proxy.redis.resources.limits.memory=2Gi
check "redis: the memory bound and the resources are values" sh -c "
  grep -Eq '^    maxmemory 1gb\$' '$out' && grep -Eq '^              memory: 2Gi\$' '$out'"

# The other shapes the values allow.
render -f "$redis_values" --set networkPolicy.enabled=false
check "redis, NetworkPolicies off: none is rendered" lacks '^kind: NetworkPolicy$'
render -f "$redis_values" --set externalSecrets.enabled=false
check "redis, ExternalSecrets off: none rendered, the pods still name the Secrets (the deployment's own: oauth2-proxy's and the Redis's)" sh -c "
  ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq 'key: OAUTH2_PROXY_REDIS_PASSWORD\$' '$out' && grep -Eq 'name: another-agentic-oauth2-redis\$' '$out'"
render -f "$redis_values" -f "$chart/tests/web-search.values.yaml" -f "$chart/tests/sharing.values.yaml" -f "$chart/tests/coder-db.values.yaml"
check "redis beside the search pod, sharing and the coder's database: still no Secret object" lacks '^kind: Secret$'
check "redis beside the other options: every image is pinned" images_ok
check "redis beside the other options: no secret-named variable has a literal value" fails literal_secret_env
render
check "redis: turning it off again gives back the default render" cmp -s "$out" "$out.cookie"
rm -f "$out.cookie" "$out.redis"

refused "a session store that is not cookie or redis" --set oauth2Proxy.sessionStore=memcached
refused "a session store spelled with a capital (a closed set)" --set oauth2Proxy.sessionStore=Redis
refused "the redis store with no AWS property for its password (add it to the AWS secret first)" -f "$redis_values" --set externalSecrets.properties.oauth2RedisPassword=
refused "the redis store with an image that has no digest" -f "$redis_values" --set oauth2Proxy.redis.image.digest=
refused "the redis store with a short digest" -f "$redis_values" --set oauth2Proxy.redis.image.digest=sha256:abc
refused "the redis store on the latest tag" -f "$redis_values" --set oauth2Proxy.redis.image.tag=latest
refused "the redis store with a memory bound that is not one" -f "$redis_values" --set oauth2Proxy.redis.maxMemory=lots
refused "the redis store with a memory bound that is a line of configuration" -f "$redis_values" --set 'oauth2Proxy.redis.maxMemory=64mb\nrequirepass x'
refused "the redis store with persistence as a string (the string false would be on)" -f "$redis_values" --set-string oauth2Proxy.redis.persistence.enabled=false
check "the redis store with ExternalSecrets off needs no property name (the Secrets are the deployment's)" renders -f "$redis_values" --set externalSecrets.enabled=false --set externalSecrets.properties.oauth2RedisPassword=
check "the cookie store does not ask for the Redis's property, image or memory bound" renders --set externalSecrets.properties.oauth2RedisPassword= --set oauth2Proxy.redis.image.digest= --set oauth2Proxy.redis.maxMemory=lots

# ---- Tokens in the browser (ADR 0054): `auth.browser.enabled`, off by default -------------------------------------------------------
# Off: the render is the one of a chart that has never heard of it (the checks here say what is absent). On: the web's pages are
# served with no sign-in, a DPoP request goes to the orchestrator without oauth2-proxy, everything else on /api and /agui stays
# behind it.
web_args() { awk '/^kind: Deployment$/ { d = 1 } /^---$/ { d = 0 } d' "$out" | awk '/^  name: another-agentic-web$/ { w = 1 } w'; }
browser_order() { # the public auth route and the DPoP routes are declared before the routes they would otherwise fall into
  caddyfile | grep -E "^${T}handle " | awk '{ o = o $2 "|" } END { exit (o ~ /@publicAuth.*@dpopApi.*@dpopAgui.*\/api\/\*.*\/agui\/\*/) ? 0 : 1 }'
}
dpop_blocks_ok() { # to the orchestrator, no sign-in, X-Auth-Request-Email removed, Authorization and DPoP left alone
  for m in @dpopApi @dpopAgui; do
    in_block "$m" 'another-agentic-orchestrator:8080' || return 1
    in_block "$m" 'header_up -X-Auth-Request-Email' || return 1
    not_in_block "$m" 'forward_auth|copy_headers|header_up -?(Authorization|DPoP)|oauth2-proxy' || return 1
  done
}
catch_all_is_web() { block '' | grep -Fq 'another-agentic-web:3000'; }

render
config_of config.yaml "$cfg"
check "browser off (default): the configuration has no dpop and no browser section" cfg_lacks '^  (dpop|browser):'
check "browser off (default): no DPoP matcher and no public auth route in the render" lacks 'DPoP|dpop|public/auth'
check "browser off (default): the web has no WEB_CSP_CONNECT_SRC" lacks 'WEB_CSP_CONNECT_SRC'
check "browser off (default): seven handle blocks in the Caddyfile" test "$(n_handles)" -eq 7
check "browser off (default): the web's catch-all goes to the web" catch_all_is_web
check "browser off (default): ... after asking oauth2-proxy (forward_auth), a 401 sent to sign in" in_block '' 'forward_auth'
check "browser off (default): ... a 401 sent to /oauth2/start" in_block '' '/oauth2/start'
check "values.yaml: auth.browser.enabled is false by default" sh -c "awk '/^  browser:/{m=1} m && /^    enabled:/{print \$2; exit}' '$chart/values.yaml' | grep -qx false"
cp "$out" "$out.browser-off"

render --set auth.browser.enabled=true
config_of config.yaml "$cfg"
check "browser on: the orchestrator accepts DPoP for the host's origin only, within 60 s past and 5 s ahead" cfg_all \
  '^    publicOrigins:$' '^      - "https://agentic.servers.segning.pro"$' '^    maxAgeSeconds: 60$' '^    futureSkewSeconds: 5$'
check "browser on: exactly one public origin" test "$(grep -Ec '^      - "https://' "$cfg")" -eq 1
check "browser on: the web's client and scope are told (auth.browser), with offline_access" cfg_all \
  '^  browser:$' '^    clientId: "another-agentic-web"$' '^    scope: "openid email profile offline_access"$'
check "browser on: still a production, jwt, fail-closed configuration with the same audience" cfg_all '^  environment: production$' '^  mode: jwt$' '^  defaultRole: null$' '^      - another-agentic$'
check "browser on: every secret key of the configuration is still a reference" test -z "$(plain_secret_in_config)"
check "browser on: ten handle blocks: the seven, the public auth route and the two DPoP ones, no eleventh" test "$(n_handles)" -eq 10
check "browser on: the handles that skip forward_auth are the four of before, the public auth route, the two DPoP routes and the web's pages, and no other" test "$(handles_by_auth without)" = '/edge-healthz|/oauth2/*|/thread-tools/*|@runtimeConfig|@publicAuth|@dpopApi|@dpopAgui|(web)|'
check "browser on: /api/* and /agui/* are what still asks oauth2-proxy" test "$(handles_by_auth with)" = '/api/*|/agui/*|'
check "browser mode: /config.json is read with no sign-in too" runtime_config_public
check "browser on: the DPoP matcher of /api/*" matcher_has @dpopApi 'path /api/*'
check "browser on: ... and its header is Authorization: DPoP *" matcher_has @dpopApi 'header Authorization "DPoP *"'
check "browser on: the DPoP matcher of /agui/*" matcher_has @dpopAgui 'path /agui/*'
check "browser on: ... and its header is Authorization: DPoP *" matcher_has @dpopAgui 'header Authorization "DPoP *"'
check "browser on: the DPoP routes go to the orchestrator, remove X-Auth-Request-Email and never touch Authorization or DPoP, no oauth2-proxy" dpop_blocks_ok
check "browser on: ... the AG-UI one buffers 8MiB" in_block @dpopAgui 'request_buffers 8MiB'
check "browser on: ... and streams" in_block @dpopAgui 'flush_interval -1'
check "browser on: ... the API one streams" in_block @dpopApi 'flush_interval -1'
check "browser on: /api/public/auth is GET and HEAD only" matcher_has @publicAuth 'method GET HEAD'
check "browser on: ... exactly that path" matcher_has @publicAuth 'path /api/public/auth'
check "browser on: ... Authorization removed" in_block @publicAuth 'header_up -Authorization'
check "browser on: ... X-Auth-Request-Email removed" in_block @publicAuth 'header_up -X-Auth-Request-Email'
check "browser on: ... to the orchestrator" in_block @publicAuth 'another-agentic-orchestrator:8080'
check "browser on: ... with no sign-in" not_in_block @publicAuth 'forward_auth|copy_headers|oauth2-proxy'
check "browser on: those three routes are declared before /api/* and /agui/*" browser_order
check "browser on: the web's catch-all goes to the web" catch_all_is_web
check "browser on: ... with no forward_auth, no redirect to sign in, no oauth2-proxy" not_in_block '' 'forward_auth|/oauth2/start|copy_headers|oauth2-proxy'
check "browser on: ... Authorization removed" in_block '' 'header_up -Authorization'
check "browser on: ... X-Auth-Request-Email removed" in_block '' 'header_up -X-Auth-Request-Email'
check "browser on: /api/* and /agui/* are still behind forward_auth for what is not DPoP (two copy_headers, none for the web)" count '^\s+copy_headers Authorization$' 2
check "browser on: /oauth2/* is still routed to oauth2-proxy (a cookie of before ends where it began)" has 'handle /oauth2/\*'
check "browser on: the thread tools are still answered 404 and no MCP or webhook route exists" no_other_surface
csp_origin_ok() { web_args | grep -A1 -F 'name: WEB_CSP_CONNECT_SRC' | grep -Fq 'value: "https://auth.verif.fyi"'; }
web_one_env() { [ "$(web_args | grep -Ec '^ +- name: [A-Z_]+$')" -eq 1 ]; }
check "browser on: the web's CSP names the issuer's origin (scheme and host, no path)" csp_origin_ok
check "browser on: ... and it is the web's only variable (it holds no secret)" web_one_env
check "browser on: no Secret object" lacks '^kind: Secret$'
check "browser on: no secret-named environment variable has a literal value" fails literal_secret_env
check "browser on: every image is pinned" images_ok
check "browser on: the same NetworkPolicies (edge to web and to the orchestrator are already allowed; the browser's calls to the issuer are its own)" \
  sh -c "[ \"\$(grep -c '^kind: NetworkPolicy\$' '$out')\" -eq \"\$(grep -c '^kind: NetworkPolicy\$' '$out.browser-off')\" ]"
check "browser on: the render is not the default one" fails cmp -s "$out" "$out.browser-off"
render --set auth.browser.enabled=true --set auth.browser.clientId=web-two --set 'auth.browser.scope=openid email'
config_of config.yaml "$cfg"
check "browser on: the client id and the scope are values" cfg_all '^    clientId: "web-two"$' '^    scope: "openid email"$'
render --set auth.browser.enabled=true --set sharing.mode=public
check "browser on with public sharing: thirteen handle blocks (the seven, the three public links, the three of the browser)" test "$(n_handles)" -eq 13
check "browser on with public sharing: the public links keep their own routes, stripped of identity" public_blocks_ok
check "browser on with public sharing: the handles that skip forward_auth are those of the two features and no other" test "$(handles_by_auth without)" = '/edge-healthz|/oauth2/*|/thread-tools/*|@runtimeConfig|@publicApi|@publicAgui|@publicWeb|@publicAuth|@dpopApi|@dpopAgui|(web)|'
check "browser on with public sharing: dev/Caddyfile still has the same three public blocks" dev_has_the_chart_public_blocks
all_public_first() {
  caddyfile | grep -E "^${T}handle " | awk '{ o = o $2 "|" } END { exit (o ~ /@publicApi.*@publicAgui.*@publicWeb.*@publicAuth.*@dpopApi.*@dpopAgui.*\/api\/\*.*\/agui\/\*/) ? 0 : 1 }'
}
check "browser on with public sharing: the public link routes come first, then the browser's, then /api/* and /agui/*" all_public_first
render --set auth.browser.enabled=false
check "browser: turning it off again gives back the default render, byte for byte" cmp -s "$out" "$out.browser-off"
rm -f "$out.browser-off"
render
config_of config.yaml "$cfg"
refused "auth.browser.enabled as a string (the string false would be on)" --set-string auth.browser.enabled=false
refused "auth.browser.enabled with no client id" --set auth.browser.enabled=true --set auth.browser.clientId=
refused "auth.browser.enabled with a scope that has no openid" --set auth.browser.enabled=true --set 'auth.browser.scope=email profile'
refused "auth.browser.enabled with an empty scope" --set auth.browser.enabled=true --set auth.browser.scope=
refused "auth.browser.enabled with a scope that is a line of configuration" --set auth.browser.enabled=true --set 'auth.browser.scope=openid\nx: y'
refused "auth.browser.enabled with no issuer" --set auth.browser.enabled=true --set auth.issuer=
check "auth.browser's client id and scope are not checked while it is off" renders --set auth.browser.clientId= --set auth.browser.scope=

# ---- The browser agent (ADR 0057): `browser.enabled`, off by default ------------------------------------------------------------
br_values="$chart/tests/browser.values.yaml"
obscura_image='docker.io/h4ckf0r0day/obscura:0.2.4@sha256:772cf3bada266e81bc0fae0304074f4971a551e3766f412417f006273e202f0e'
render
cp "$out" "$out.browser-agent-off"
config_of agents.yaml "$cfg"
check "browser agent off: nothing of it is rendered (no pod, Service, policy, Secret, sidecar or token)" lacks 'another-agentic-browser|obscura|BROWSER_A2A_TOKEN|browser_a2a_token'
check "browser agent off: the agents file does not list it" cfg_lacks 'id: browser'
check "browser agent off: no pod may call a plain-http remote sub-agent (no A2A_ALLOW_INSECURE_REMOTES)" lacks 'A2A_ALLOW_INSECURE_REMOTES'
check "values.yaml: browser.chatSubagent is false by default" sh -c "awk '/^browser:/{m=1} m && /^  chatSubagent:/{print \$2; exit}' '$chart/values.yaml' | grep -qx false"
check "values.yaml pins obscura by tag and digest" sh -c "grep -Eq '^      digest: sha256:772cf3bada266e81bc0fae0304074f4971a551e3766f412417f006273e202f0e\$' '$chart/values.yaml'"
render -f "$br_values"
check "browser agent on: six Deployments, six Services, six NetworkPolicies, five ExternalSecrets, still no Secret" sh -c "
  [ \"\$(grep -Ec '^kind: Deployment\$' '$out')\" -eq 6 ] && [ \"\$(grep -Ec '^kind: Service\$' '$out')\" -eq 6 ] &&
  [ \"\$(grep -Ec '^kind: NetworkPolicy\$' '$out')\" -eq 6 ] && [ \"\$(grep -Ec '^kind: ExternalSecret\$' '$out')\" -eq 5 ] && ! grep -Eq '^kind: Secret\$' '$out'"
check "browser agent on: every image is ours by commit or a tag with a digest" images_ok
check "browser agent on: obscura by tag and digest, the agent the chart's adam image" sh -c "
  grep -Fq 'image: \"$obscura_image\"' '$out' && [ \"\$(grep -Ec '^ *image: \"ghcr.io/vymalo/another-adam-rs/coder:sha-[0-9a-f]{7}@sha256:' '$out')\" -eq 2 ]"
check "browser agent on: no secret-named variable has a literal value" fails literal_secret_env
doc Deployment another-agentic-browser > "$sec"
check "browser agent on: one replica, Recreate (never two browsers on one database)" sec_all '^  replicas: 1$' '^    type: Recreate$'
check "browser agent on: obscura is a native sidecar serving MCP on the loopback, no CDP, no stealth, no private network" sh -c "
  grep -Eq '^          restartPolicy: Always\$' '$sec' && grep -Fq 'args: [\"mcp\", \"--http\", \"--host\", \"127.0.0.1\", \"--port\", \"9223\"]' '$sec' &&
  ! grep -Eq -- '--stealth|--allow-private-network|ALLOW_PRIVATE_NETWORK|\"serve\"' '$out'"
check "browser agent on: obscura runs as 65532, read-only root, no escalation, every capability dropped" sh -c "
  awk '/^      initContainers:/,/^      containers:/' '$sec' | grep -Eq 'runAsUser: 65532\$' &&
  awk '/^      initContainers:/,/^      containers:/' '$sec' | grep -Eq 'readOnlyRootFilesystem: true\$' &&
  awk '/^      initContainers:/,/^      containers:/' '$sec' | grep -Eq 'allowPrivilegeEscalation: false\$' &&
  awk '/^      initContainers:/,/^      containers:/' '$sec' | grep -Fq 'drop: [\"ALL\"]'"
check "browser agent on: the agent runs as 10001, adam-agent, one worker, no service account token" sec_all 'runAsUser: 10001$' 'command: \["tini", "--", "adam-agent"\]' '- name: WORKERS$' 'value: "1"$' 'automountServiceAccountToken: false$'
check "browser agent on: the sidecar's bearer comes from the browser's Secret, to both containers" sh -c "
  [ \"\$(grep -Ec 'key: OBSCURA_MCP_TOKEN\$' '$sec')\" -eq 2 ] && grep -Eq 'name: another-agentic-browser\$' '$sec'"
check "browser agent on: its runs are in the chat agent's database (the agent role's Secret)" sec_all 'name: another-agentic-db-agent$' 'key: uri$'
check "browser agent on: the card is the Service" sec_all 'value: "http://another-agentic-browser.another-agentic-system.svc:8080/"$'
check "browser agent on: startup and liveness on the agent's port, readiness on the agent's and the browser's" sec_all 'startupProbe:$' 'livenessProbe:$' 'exec 3<>/dev/tcp/127.0.0.1/8080 && exec 4<>/dev/tcp/127.0.0.1/9223'
check "browser agent on: requests and memory limits for both containers" sh -c "[ \"\$(grep -Ec '^ +requests:\$' '$sec')\" -eq 2 ] && [ \"\$(grep -Ec '^ +limits:\$' '$sec')\" -eq 2 ]"
doc ConfigMap another-agentic-browser-agent > "$sec2"
check "browser agent on: its folder is files/browser: the instructions and the mcp.json" sec2_all '^  instructions.md: \|$' '^  mcp.json: \|$' '^    name: browser$'
check "browser agent on: the mcp.json names the sidecar on the loopback, the bearer a variable, no value" sec2_all '"url": "http://127.0.0.1:9223/mcp"' '"Authorization": "Bearer \$\{OBSCURA_MCP_TOKEN\}"'
check "browser agent on: the allow-list leaves out evaluate, the cookies, the storage state and the bulk form fill" sh -c "
  ! grep -Eq '\"browser_(evaluate|get_cookies|set_cookie|storage_state|set_storage_state|fill_form)\"' '$sec2' && grep -Eq '\"browser_screenshot\"' '$sec2'"
doc ExternalSecret another-agentic-browser > "$sec2"
check "browser agent on: its ExternalSecret reads its bearer, the model's key and the sidecar's token" sec2_all 'property: browser_a2a_token$' 'property: model_api_key$' 'property: obscura_mcp_token$'
check "browser agent on: the sidecar's token is read by the browser's ExternalSecret alone" count 'property: obscura_mcp_token$' 1
br_prop=$(doc ExternalSecret another-agentic-browser | awk '/secretKey: A2A_BEARER_TOKENS/ { getline; getline; getline; print $2 }')
orch_br_prop=$(doc ExternalSecret another-agentic-orchestrator | awk '/secretKey: BROWSER_A2A_TOKEN/ { getline; getline; getline; print $2 }')
check "browser agent on: the agent and the orchestrator read the browser's bearer from the same property" test -n "$br_prop" -a "$br_prop" = "$orch_br_prop"
check "browser agent on: the orchestrator has the bearer variable" dhas Deployment another-agentic-orchestrator '- name: BROWSER_A2A_TOKEN$'
config_of agents.yaml "$cfg"
check "browser agent on: the agents file lists it last, as Browser, by its Service's card" sh -c "
  [ \"\$(grep -Ec '^  id: ' '$cfg')\" -eq 3 ] && tail -n 4 '$cfg' | grep -Eq '^  id: browser\$' && grep -Eq '^  name: Browser\$' '$cfg' &&
  grep -Eq 'cardUrl: http://another-agentic-browser\.another-agentic-system\.svc:8080/\.well-known/agent-card\.json\$' '$cfg' && grep -Eq 'tokenEnv: BROWSER_A2A_TOKEN\$' '$cfg'"
doc NetworkPolicy another-agentic-browser > "$sec"
check "browser agent on: its policy covers ingress and egress" sec_all '^    - Ingress$' '^    - Egress$'
check "browser agent on: in from the orchestrator, on 8080 only" sec_all 'component: orchestrator$' 'port: 8080$'
check "browser agent on: not from the chat (its remote sub-agent, browser.chatSubagent, is off)" fails sec_all 'component: chat$'
check "browser agent on: not from the coder by default (open question 69)" fails sec_all 'app.kubernetes.io/name: coder$'
check "browser agent on: not from the edge, the web or oauth2-proxy" fails sec_all 'component: (edge|web|oauth2-proxy)$'
check "browser agent on: out to DNS and to the public internet on 443 and 80 except the special-purpose ranges" sec_all 'port: 53$' 'cidr: 0.0.0.0/0' '10.0.0.0/8' '100.64.0.0/10' '127.0.0.0/8' '169.254.0.0/16' '172.16.0.0/12' '192.0.0.0/24' '192.168.0.0/16' '198.18.0.0/15' 'cidr: ::/0' 'fc00::/7' 'fe80::/10' '64:ff9b::/96' '64:ff9b:1::/48' '2002::/16' 'port: 443$' 'port: 80$'
# browser_egress_rule PEER PORT: one rule of the browser policy's egress that goes to PEER (a label line) on PORT, in that rule alone.
browser_egress_rule() {
  awk '/^  egress:/ { e = 1; next } e && /^    - / { if (r != "") print r; r = "" } e { r = r $0 "|" } END { if (r != "") print r }' "$sec" |
    grep -E -- "$1" | grep -Eq -- "port: $2\\|"
}
check "browser agent on: out to its database (the release's CNPG instances, 5432)" browser_egress_rule 'cnpg.io/cluster: another-agentic-db\|' 5432
check "browser agent on: out to the orchestrator's pods on 8080 (the thread tools), in a rule of its own" browser_egress_rule 'component: orchestrator\|' 8080
check "browser agent on: no other egress rule names the orchestrator or the database" sh -c "
  [ \"\$(awk '/^  egress:/,0' '$sec' | grep -Ec 'component: orchestrator\$|cnpg.io/cluster:')\" -eq 2 ]"
check "browser agent on: no IPv4-mapped range" sh -c "! grep -q '::ffff:' '$out'"
check "browser agent on: its policy is the only one that restricts egress" count '^    - Egress$' 1
check "browser agent on: the orchestrator's policy lets it in (the thread tools)" dhas NetworkPolicy another-agentic-orchestrator 'component: browser$'
doc ConfigMap another-agentic-chat-agent > "$sec2"
check "browser agent on: the chat's folder has no browser sub-agent (browser.chatSubagent is off)" sh -c "! grep -Eq 'subagent-browser' '$sec2'"
check "browser agent on: the chat's Deployment names no browser bearer" dlacks Deployment another-agentic-chat 'BROWSER_A2A_TOKEN'
check "browser agent on: no pod may call a plain-http remote sub-agent (browser.chatSubagent is off)" lacks 'A2A_ALLOW_INSECURE_REMOTES'
check "browser agent on: the chat's ExternalSecret reads no browser bearer" dlacks ExternalSecret another-agentic-chat 'BROWSER_A2A_TOKEN'
# The chat's remote sub-agent (ADR 0057, amended 2026-10-09): a tool `browser` of the chat, the browser's Service over plain http with
# the browser's bearer, its files passed on (`files: true`). Off by default; on, it changes the chat and the browser's policy, nothing else.
render -f "$br_values" --set browser.chatSubagent=true
doc ConfigMap another-agentic-chat-agent > "$sec2"
check "chat sub-agent on: the chat's folder has subagents/browser.md, the browser's Service by its card, its bearer a variable, files passed on" sec2_all \
  '^  subagent-browser.md: \|$' '^    a2a: http://another-agentic-browser\.another-agentic-system\.svc:8080/\.well-known/agent-card\.json$' \
  '^    auth: bearer:BROWSER_A2A_TOKEN$' '^    files: true$'
check "chat sub-agent on: ... and not the dev stack's address" sh -c "! grep -Fq 'http://browser:8080' '$sec2'"
check "chat sub-agent on: the file is mounted at subagents/browser.md" dhas Deployment another-agentic-chat '\{ key: subagent-browser\.md, path: subagents/browser\.md \}'
chat_insecure_remotes() { doc Deployment another-agentic-chat | grep -A1 -E -- '- name: A2A_ALLOW_INSECURE_REMOTES$' | grep -Eq 'value: "true"$'; }
check "chat sub-agent on: the chat allows a plain-http remote sub-agent (A2A_ALLOW_INSECURE_REMOTES=true)" chat_insecure_remotes
check "chat sub-agent on: ... and no other pod does" count '- name: A2A_ALLOW_INSECURE_REMOTES$' 1
chat_browser_bearer() { doc Deployment another-agentic-chat | grep -A4 -E -- '- name: BROWSER_A2A_TOKEN$' | grep -Eq 'name: another-agentic-chat$'; }
check "chat sub-agent on: the chat's bearer for the browser is a key of the chat's Secret" chat_browser_bearer
chat_br_prop=$(doc ExternalSecret another-agentic-chat | awk '/secretKey: BROWSER_A2A_TOKEN/ { getline; getline; getline; print $2 }')
br_prop=$(doc ExternalSecret another-agentic-browser | awk '/secretKey: A2A_BEARER_TOKENS/ { getline; getline; getline; print $2 }')
check "chat sub-agent on: the chat sends the bearer the browser accepts (one AWS property)" test -n "$chat_br_prop" -a "$chat_br_prop" = "$br_prop"
doc NetworkPolicy another-agentic-browser > "$sec"
check "chat sub-agent on: the browser's policy lets the chat in" sec_all 'component: chat$'
check "chat sub-agent on: ... and only the orchestrator and the chat, on 8080 only" sh -c "
  [ \"\$(awk '/^  ingress:/,/^  egress:/' '$sec' | grep -Ec 'podSelector:\$')\" -eq 2 ] &&
  [ \"\$(awk '/^  ingress:/,/^  egress:/' '$sec' | grep -Ec 'port: ')\" -eq 1 ] && awk '/^  ingress:/,/^  egress:/' '$sec' | grep -Eq 'port: 8080\$'"
check "chat sub-agent on: every image is still pinned" images_ok
check "chat sub-agent on: no secret-named variable has a literal value" fails literal_secret_env
render -f "$br_values" --set 'browser.allowFrom[0].podSelector.matchLabels.app\.kubernetes\.io/name=coder'
check "browser agent on: who else may call it is a value (the coder's pods)" dhas NetworkPolicy another-agentic-browser 'app.kubernetes.io/name: coder$'
render -f "$br_values" --set 'browser.extraEgress[0].ports[0].port=4000' --set 'browser.extraEgress[0].to[0].namespaceSelector.matchLabels.kubernetes\.io/metadata\.name=ai'
check "browser agent on: more egress is a value (a model gateway in the cluster)" dhas NetworkPolicy another-agentic-browser 'kubernetes.io/metadata.name: ai$'
render -f "$br_values" --set chat.enabled=false --set-json 'agents=[{"id":"coder","name":"Adam","cardUrl":"http://c/","tokenEnv":"CODER_A2A_TOKEN"}]'
check "browser agent on, chat off: the database agent and its role are still there, for the browser" sh -c "
  grep -Eq '^  name: another-agentic-db-agent\$' '$out' && grep -Eq '^      - name: agent\$' '$out' && ! grep -Eq '^  name: another-agentic-chat\$' '$out'"
check "browser agent on, chat off: the policy lets in no chat" dlacks NetworkPolicy another-agentic-browser 'component: chat$'
render -f "$br_values" --set-json 'agents=[{"id":"coder","name":"Adam","cardUrl":"http://c/","tokenEnv":"CODER_A2A_TOKEN"},{"id":"chat","name":"Chat","cardUrl":"http://c2/","tokenEnv":"CHAT_A2A_TOKEN"},{"id":"browser","name":"Web","cardUrl":"http://b/","tokenEnv":"BROWSER_A2A_TOKEN"}]'
config_of agents.yaml "$cfg"
check "browser agent on, already in agents: listed once, as written" sh -c "[ \"\$(grep -Ec '^  id: browser\$' '$cfg')\" -eq 1 ] && grep -Eq '^  name: Web\$' '$cfg'"
# A port of its own for the sidecar reaches the folder's mcp.json, the sidecar's arguments and the readiness probe.
render -f "$br_values" --set browser.obscura.port=9300
port_9300_everywhere() {
  doc ConfigMap another-agentic-browser-agent | grep -Fq '"url": "http://127.0.0.1:9300/mcp"' &&
    ! doc ConfigMap another-agentic-browser-agent | grep -Fq '9223' &&
    doc Deployment another-agentic-browser | grep -Fq '"--port", "9300"]' &&
    doc Deployment another-agentic-browser | grep -Fq 'exec 4<>/dev/tcp/127.0.0.1/9300'
}
check "browser agent, another sidecar port: the mcp.json, the sidecar and the readiness probe all say 9300" port_9300_everywhere
# Each workload's checksum of secrets is its own ExternalSecret's: another Secret's shape changes and the browser does not restart.
render -f "$br_values"
br_sum=$(doc Deployment another-agentic-browser | sed -n 's/^ *checksum\/secrets: //p')
render -f "$br_values" -f "$ws_values"
check "browser agent: a change to another workload's Secret (the search pod's, the chat's) leaves its checksum of secrets as it was" \
  test -n "$br_sum" -a "$br_sum" = "$(doc Deployment another-agentic-browser | sed -n 's/^ *checksum\/secrets: //p')"
render -f "$br_values" --set networkPolicy.enabled=false
check "browser agent on, NetworkPolicies off: none is rendered" lacks '^kind: NetworkPolicy$'
render -f "$br_values" --set externalSecrets.enabled=false
check "browser agent on, ExternalSecrets off: none is rendered, the Deployment still names its Secret" sh -c "
  ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq 'name: another-agentic-browser\$' '$out'"
render -f "$br_values" --set browser.enabled=false
check "browser agent: turning it off again gives back the default render, byte for byte" cmp -s "$out" "$out.browser-agent-off"
rm -f "$out.browser-agent-off"
refused "browser.enabled as a string (the string false would be on)" --set-string browser.enabled=false
refused "the browser agent with two replicas (a run steps on any worker: another pod's browser)" -f "$br_values" --set browser.replicas=2
refused "the browser agent with no model endpoint" -f "$br_values" --set model.baseUrl=
refused "the browser agent with no model name" -f "$br_values" --set chat.model= --set chat.enabled=false --set-json 'agents=[{"id":"coder","name":"Adam","cardUrl":"http://c/","tokenEnv":"CODER_A2A_TOKEN"}]'
refused "the browser agent with no AWS property for the sidecar's bearer" -f "$br_values" --set externalSecrets.properties.obscuraMcpToken=
refused "the browser agent with no property for its own bearer" -f "$br_values" --set externalSecrets.agentTokens.BROWSER_A2A_TOKEN=
refused "obscura without a digest" -f "$br_values" --set browser.obscura.image.digest=
refused "obscura on the agent's port" -f "$br_values" --set browser.obscura.port=8080
# refused_saying <description> <words of the refusal> <helm args...>: the render fails, and with that reason (not another check's).
refused_saying() {
  desc=$1; words=$2; shift 2
  if why=$(helm template another-agentic-system "$chart" --namespace another-agentic-system -f "$base" "$@" 2>&1 >/dev/null); then
    echo "FAIL refused: $desc"; fail=1
  elif printf '%s' "$why" | grep -Fq -- "$words"; then
    echo "ok   refused: $desc"
  else
    echo "FAIL refused: $desc, but not for that reason: $(printf '%s' "$why" | tail -n 1)"; fail=1
  fi
}
refused "browser.chatSubagent as a string" -f "$br_values" --set-string browser.chatSubagent=false
refused_saying "the chat's browser sub-agent with the browser off (it would call no Service)" "browser.chatSubagent needs browser.enabled" --set browser.chatSubagent=true
refused_saying "the chat's browser sub-agent with the chat off (nobody would call it)" "browser.chatSubagent needs chat.enabled" \
  -f "$br_values" --set browser.chatSubagent=true --set chat.enabled=false --set-json 'agents=[{"id":"coder","name":"Adam","cardUrl":"http://c/","tokenEnv":"CODER_A2A_TOKEN"}]'
refused_saying "the chat's browser sub-agent with no property for the browser's bearer (the chat would stop, exit 78)" "browser.chatSubagent needs the browser's bearer" \
  -f "$br_values" --set browser.chatSubagent=true --set externalSecrets.agentTokens.BROWSER_A2A_TOKEN=
check "the chat's browser sub-agent renders with the browser, the chat and the bearer" renders -f "$br_values" --set browser.chatSubagent=true
check "files/browser/instructions.md is dev/agents/browser/agent/instructions.md" cmp -s "$chart/files/browser/instructions.md" "$repo/dev/agents/browser/agent/instructions.md"
check "files/browser/mcp.json is dev/agents/browser/agent/mcp.json" cmp -s "$chart/files/browser/mcp.json" "$repo/dev/agents/browser/agent/mcp.json"
check "files/browser/chat-subagent.md is dev/agents/browser/chat-subagent.md" cmp -s "$chart/files/browser/chat-subagent.md" "$repo/dev/agents/browser/chat-subagent.md"
check "the browser's screenshots and PDFs are shared files (obscura's entry says files: true, adam-rs ADR 0033)" sh -c "grep -Eq '^      \"files\": true,\$' '$chart/files/browser/mcp.json'"
render
config_of config.yaml "$cfg"

# ---- Artifacts in S3, and RustFS (ADR 0032, amended 2026-10-09): `orchestrator.artifacts.store`, `rustfs.enabled`, fs and off by default --------
rf_values="$chart/tests/rustfs.values.yaml"
rustfs_image='docker.io/rustfs/rustfs:1.0.1-preview.17@sha256:662587e2bdb0262fbb570cd2e7d34467e601ff32b8256e3203dbde40557e0cd2'
render
cp "$out" "$out.s3-off"
check "artifacts fs by default: no RustFS and no S3 key anywhere, the volume is rendered" sh -c "
  ! grep -Eq 'rustfs|artifacts-s3|artifacts_s3' '$out' && grep -Eq '^  name: another-agentic-artifacts\$' '$out'"
render --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=agentic-files --set orchestrator.artifacts.s3.endpoint=https://s3.example.org --set orchestrator.artifacts.s3.prefix=prod/agentic
config_of config.yaml "$cfg"
check "s3: the configuration names the bucket, the region, the endpoint and the prefix" cfg_all '^  store: s3$' '^    bucket: "agentic-files"$' '^    region: "us-east-1"$' '^    endpoint: "https://s3.example.org"$' '^    prefix: "prod/agentic"$' '^    timeoutSecs: 60$'
check "s3: the credentials are files of the orchestrator's Secret" cfg_all '^    accessKeyId: \{ file: /run/secrets/orchestrator/artifacts-s3-access-key-id \}$' '^    secretAccessKey: \{ file: /run/secrets/orchestrator/artifacts-s3-secret-access-key \}$'
check "s3: every secret key of the configuration is still a reference" test -z "$(plain_secret_in_config)"
check "s3: no directory store and no volume for it" sh -c "! grep -Eq 'fs: \\{ root:' '$cfg' && ! grep -Eq 'another-agentic-artifacts|mountPath: /var/lib/orchestrator/artifacts' '$out'"
check "s3: the orchestrator mounts the two keys" dhas Deployment another-agentic-orchestrator 'path: artifacts-s3-secret-access-key$'
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "s3: the orchestrator's ExternalSecret reads the two AWS properties" sec_all 'secretKey: artifacts-s3-access-key-id$' 'property: artifacts_s3_access_key_id$' 'secretKey: artifacts-s3-secret-access-key$' 'property: artifacts_s3_secret_access_key$'
check "s3 on another server: no RustFS" lacks 'rustfs'
render -f "$rf_values"
config_of config.yaml "$cfg"
check "RustFS on: a StatefulSet, a Service, a hook Job, an ExternalSecret and a NetworkPolicy of its own, still no Secret" sh -c "
  grep -Eq '^kind: StatefulSet\$' '$out' && grep -Eq '^kind: Job\$' '$out' && ! grep -Eq '^kind: Secret\$' '$out' &&
  [ \"\$(grep -Ec '^kind: ExternalSecret\$' '$out')\" -eq 5 ] && [ \"\$(grep -Ec '^kind: NetworkPolicy\$' '$out')\" -eq 6 ]"
check "RustFS on: the configuration is its Service and its bucket, in the path (no virtual host)" cfg_all '^    bucket: "artifacts"$' '^    endpoint: "http://another-agentic-rustfs\.another-agentic-system\.svc:9000"$'
check "RustFS on: every image is ours by commit or a tag with a digest, RustFS by tag and digest" sh -c "
  grep -Fq 'image: \"$rustfs_image\"' '$out'" && images_ok
check "RustFS on: no secret-named variable has a literal value" fails literal_secret_env
doc StatefulSet another-agentic-rustfs > "$sec"
check "RustFS on: one replica, uid 10001 with the volume's group, non-root, RuntimeDefault, no service account token" sec_all '^  replicas: 1$' 'runAsUser: 10001$' 'fsGroup: 10001$' 'runAsNonRoot: true$' 'type: RuntimeDefault$' 'automountServiceAccountToken: false$'
check "RustFS on: read-only root, no escalation, every capability dropped" sec_all 'readOnlyRootFilesystem: true$' 'allowPrivilegeEscalation: false$' 'drop: \["ALL"\]$'
check "RustFS on: S3 on 9000, the console off, logs on stdout, the version check sent nowhere" sec_all 'containerPort: 9000$' 'name: RUSTFS_CONSOLE_ENABLE$' 'value: "false"$' 'name: RUSTFS_OBS_LOG_DIRECTORY$' 'value: http://127.0.0.1:9$'
check "RustFS on: its credentials come from its own Secret" sec_all 'key: RUSTFS_ACCESS_KEY$' 'key: RUSTFS_SECRET_KEY$' 'name: another-agentic-rustfs$'
check "RustFS on: startup and liveness on /health/live, readiness on /health/ready" sec_all 'path: /health/live$' 'path: /health/ready$'
check "RustFS on: its objects on a longhorn claim of 10Gi" sec_all 'volumeClaimTemplates:$' 'storageClassName: "longhorn"$' 'storage: "10Gi"$'
doc Job another-agentic-rustfs-bucket > "$sec2"
check "RustFS on: the bucket is made by a hook Job after each sync, with RustFS's image and its credentials, signed" sh -c "
  grep -Eq 'helm.sh/hook: post-install,post-upgrade\$' '$sec2' && grep -Fq 'image: \"$rustfs_image\"' '$sec2' &&
  grep -Eq 'value: \"artifacts\"\$' '$sec2' && grep -Eq -- '--aws-sigv4' '$sec2' && grep -Eq 'key: RUSTFS_SECRET_KEY\$' '$sec2' && grep -Eq 'readOnlyRootFilesystem: true\$' '$sec2'"
check "RustFS on: the Job gives curl the credentials on its standard input, never on a command line" sh -c "
  grep -Fq 'curl -fsS -o /dev/null -K - --aws-sigv4' '$sec2' && ! grep -Eq -- '--user|-u \"' '$sec2'"
rf_prop=$(doc ExternalSecret another-agentic-rustfs | awk '/secretKey: RUSTFS_SECRET_KEY/ { getline; getline; getline; print $2 }')
orch_s3_prop=$(doc ExternalSecret another-agentic-orchestrator | awk '/secretKey: artifacts-s3-secret-access-key/ { getline; getline; getline; print $2 }')
check "RustFS on: RustFS and the orchestrator read the secret key from the same property" test -n "$rf_prop" -a "$rf_prop" = "$orch_s3_prop"
doc NetworkPolicy another-agentic-rustfs > "$sec"
check "RustFS on: in from the orchestrator and the bucket Job only, on 9000" sh -c "
  grep -Eq 'component: orchestrator\$' '$sec' && grep -Eq 'component: rustfs-bucket\$' '$sec' && grep -Eq 'port: 9000\$' '$sec' &&
  ! grep -Eq 'component: (edge|web|oauth2-proxy|chat|browser)\$' '$sec'"
check "RustFS on: out to DNS only (no ipBlock, no 443)" sh -c "grep -Eq '^    - Egress\$' '$sec' && grep -Eq 'port: 53\$' '$sec' && ! grep -Eq 'ipBlock|port: 443' '$sec'"
check "RustFS on: no volume for the directory store" lacks 'another-agentic-artifacts'
render -f "$rf_values" --set orchestrator.artifacts.s3.bucket=other-bucket
config_of config.yaml "$cfg"
check "RustFS on with a bucket of its own: the orchestrator writes there" cfg_has '^    bucket: "other-bucket"$'
doc Job another-agentic-rustfs-bucket > "$sec2"
check "RustFS on with a bucket of its own: the Job makes that bucket, not rustfs.bucket" sh -c "
  grep -A1 -E 'name: BUCKET\$' '$sec2' | grep -Eq 'value: \"other-bucket\"\$' && ! grep -Eq 'value: \"artifacts\"\$' '$sec2'"
render -f "$rf_values"
rf_sum=$(doc StatefulSet another-agentic-rustfs | sed -n 's/^ *checksum\/secrets: //p')
render -f "$rf_values" -f "$br_values" -f "$ws_values"
check "RustFS on: another workload's Secret (the browser's, the search pod's) leaves its checksum of secrets as it was (no restart)" \
  test -n "$rf_sum" -a "$rf_sum" = "$(doc StatefulSet another-agentic-rustfs | sed -n 's/^ *checksum\/secrets: //p')"
render --set orchestrator.artifacts.store=fs
check "artifacts: fs, said out loud, is the default render, byte for byte" cmp -s "$out" "$out.s3-off"
rm -f "$out.s3-off"
refused "an artifact store that is neither fs nor s3" --set orchestrator.artifacts.store=gcs
refused "rustfs.enabled with the directory store (nothing would use it)" --set rustfs.enabled=true
refused "rustfs.enabled as a string" --set-string rustfs.enabled=false
refused "s3 with no bucket and no RustFS" --set orchestrator.artifacts.store=s3
refused "s3 with a bucket name S3 refuses" --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=My_Bucket
refused "a bucket with a dot on AWS (no endpoint: virtual-hosted style, where a dot breaks TLS)" --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=agentic.files
check "a bucket with a dot behind an endpoint renders (path style)" renders --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=agentic.files --set orchestrator.artifacts.s3.endpoint=https://s3.example.org
refused "RustFS with an endpoint of its own" -f "$rf_values" --set orchestrator.artifacts.s3.endpoint=https://s3.example.org
refused "an endpoint with credentials in it" --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=b-1 --set orchestrator.artifacts.s3.endpoint=https://a:b@s3.example.org
refused "a prefix that climbs" --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=b-1 --set orchestrator.artifacts.s3.prefix=a/../b
refused "a timeout of zero" --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=b-1 --set orchestrator.artifacts.s3.timeoutSecs=0
refused "s3 with no AWS property for the access key" -f "$rf_values" --set externalSecrets.properties.artifactsS3AccessKeyId=
refused "RustFS without a digest" -f "$rf_values" --set rustfs.image.digest=
render
config_of config.yaml "$cfg"

# ---- The public files host of RustFS (ADR 0058, A5.2): `rustfs.ingress`, and the large-file keys of the orchestrator, all off by default ----------
# Off: nothing of it is rendered and the orchestrator's configuration has none of its keys (the pinned image need not know them). On: a Traefik
# Ingress with TLS in front of a small Caddy proxy (the edge's pinned image) in front of the RustFS Service, and the proxy is the wall: only the
# artifact keys, only GET, HEAD, OPTIONS (PUT only with uploads), only a signed query, the three security headers, CORS for the listed origins
# alone, no query string in its logs. tests/files-host-proof.sh runs this Caddyfile against a real RustFS; the goldens pin its text
# (regenerate them with `UPDATE_GOLDEN=1 sh deploy/chart/tests/render-check.sh`, and read the diff).
fh_values="$chart/tests/files-host.values.yaml"
fhu_values="$chart/tests/files-host-uploads.values.yaml"
fh_name=agentic-files.example.org
# The proxy's Caddyfile of the last render with its comments out, to $fcf; and whole.
fcaddy_full() { doc ConfigMap another-agentic-files | awk '$0 == "  Caddyfile: |" { on = 1; next } on { sub(/^    /, ""); print }'; }
fcaddy() { fcaddy_full | grep -Ev "^${T}*#" > "$fcf"; }
# fline <text>...: each text is a whole line of the Caddyfile, its indentation aside (no regular expression to escape).
fline() { for want in "$@"; do WANT=$want awk '{ sub(/^[ \t]+/, "") } $0 == ENVIRON["WANT"] { f = 1 } END { exit f ? 0 : 1 }' "$fcf" || return 1; done; }
# fcount <text> <n>: exactly n lines are that text.
fcount() { [ "$(WANT=$1 awk '{ sub(/^[ \t]+/, "") } $0 == ENVIRON["WANT"] { n++ } END { print n + 0 }' "$fcf")" -eq "$2" ]; }
# fin <opener> <line>...: each text is a whole line (indentation aside) inside the block that the line <opener> opens, up to the `}` at the opener's own
# indentation (nested blocks count): so a line in the wrong block, or a block emptied, is not found by a line that is somewhere else in the file.
fin() { op=$1; shift; for want in "$@"; do OP=$op WANT=$want awk '
  { t = $0; sub(/^[ \t]+/, "", t); ind = length($0) - length(t) }
  !on && t == ENVIRON["OP"] { on = 1; oi = ind; next }
  on && t == "}" && ind == oi { on = 0 }
  on && t == ENVIRON["WANT"] { f = 1 }
  END { exit f ? 0 : 1 }' "$fcf" || return 1; done; }
fcf_has() { grep -Eq -- "$1" "$fcf"; }
fcf_lacks() { ! grep -Eq -- "$1" "$fcf"; }
# golden_files <name>: the Caddyfile of the last render is tests/golden/<name> (or, with UPDATE_GOLDEN=1, becomes it).
golden_files() { if [ -n "${UPDATE_GOLDEN:-}" ]; then fcaddy_full > "$chart/tests/golden/$1"; else fcaddy_full | cmp -s - "$chart/tests/golden/$1"; fi; }
# values_has <block opener> <line>: the line follows the opener in values.yaml, within three lines.
values_has() { grep -A3 -- "$1" "$chart/values.yaml" | grep -qxF -- "$2"; }

render
cp "$out" "$out.files-off"
config_of config.yaml "$cfg"
check "files host off (default): no proxy, no Ingress for it, no Service, no policy and no certificate Secret name anywhere" fails grep -Eq 'another-agentic-files|files-host|files-tls' "$out"
check "files host off (default): the configuration has none of the keys of the large files (the pinned image need not know them)" cfg_lacks '(publicEndpoint|downloads|presignTtlSecs|maxPerJobBytes|upload):'
check "values.yaml: the files host is off by default, with no host (the deployment's own)" values_has '^  ingress:$' '    enabled: false'
check "values.yaml: ... no host" values_has '^  ingress:$' '    host: ""'
check "values.yaml: uploads are off by default" values_has '^    upload:$' '      enabled: false'
check "values.yaml: the downloads are streamed, and no public endpoint, lifetime or job limit is set" sh -c "
  grep -Eq '^      downloads: stream\$' '$chart/values.yaml' && grep -Eq '^      publicEndpoint: \"\"\$' '$chart/values.yaml' &&
  grep -Eq '^      presignTtlSecs: null\$' '$chart/values.yaml' && grep -Eq '^    maxPerJobBytes: null\$' '$chart/values.yaml'"
check "values.yaml: the owner's files host is not a default (home-os sets it)" fails grep -Eq '^ *[a-z].*agentic-files\.servers' "$chart/values.yaml"
render -f "$rf_values" --set networkPolicy.ingressControllerNamespace=traefik
cp "$out" "$out.files-rustfs-only"
config_of config.yaml "$cfg"
doc NetworkPolicy another-agentic-rustfs > "$sec"
check "RustFS on, files host off: no proxy, and RustFS's policy admits the orchestrator and the bucket Job only" sh -c "
  ! grep -Eq 'another-agentic-files|files-tls' '$out' && ! grep -Eq 'component: files\$' '$sec'"
check "RustFS on, files host off: no public endpoint in the configuration" cfg_lacks 'publicEndpoint'

render -f "$rf_values" -f "$fh_values"
config_of config.yaml "$cfg"
fcaddy
doc Ingress another-agentic-files > "$sec"
two_ingresses() { [ "$(grep -Ec '^kind: Ingress$' "$out")" -eq 2 ] && [ "$(grep -Ec '^  ingressClassName: traefik$' "$out")" -eq 2 ]; }
check "files host on: two Ingresses (the app's and the files host's), both on traefik" two_ingresses
check "files host on: its Ingress is for the files host, TLS from the app's cluster issuer, the certificate in a Secret of its own" \
  sec_all "^    - host: \"$fh_name\"\$" 'cert-manager.io/cluster-issuer: "cert-cloudflare"' "^        - \"$fh_name\"\$" 'secretName: another-agentic-files-tls$'
check "files host on: its Ingress sends the whole host to the proxy and to nothing else" sec_all '^          - path: /$' '^            pathType: Prefix$' 'name: another-agentic-files$' '^                  name: http$'
app_ingress_kept() { doc Ingress another-agentic | grep -Eq 'name: another-agentic-edge$' && doc Ingress another-agentic | grep -Eq '^    - host: "agentic.servers.segning.pro"$'; }
check "files host on: the app's Ingress still goes to the edge, for the app's host" app_ingress_kept
doc Service another-agentic-files > "$sec"
check "files host on: its Service is a ClusterIP on 8080, nothing else" sec_all '^  type: ClusterIP$' '^      port: 8080$' 'component: files$'
check "files host on: the configuration signs for the files host (publicEndpoint is https://<host>)" cfg_all "^    publicEndpoint: \"https://$fh_name\"\$"
check "files host on: ... and writes nothing else of the large files (the redirect, the lifetime, the job limit, the upload)" cfg_lacks '(downloads|presignTtlSecs|maxPerJobBytes|upload):'
doc Deployment another-agentic-files > "$sec"
edge_img=$(doc Deployment another-agentic-edge | sed -n 's/^ *image: //p')
files_img=$(sed -n 's/^ *image: //p' "$sec")
same_image() { [ -n "$edge_img" ] && [ "$edge_img" = "$files_img" ] && printf '%s' "$files_img" | grep -Eq ':[^:@]+@sha256:[0-9a-f]{64}"?$'; }
check "files host on: the proxy runs the edge's image, by tag and digest" same_image
check "files host on: every image is still ours by commit or a tag with a digest" images_ok
check "files host on: one replica, uid 65532, non-root, RuntimeDefault, no service account token" sec_all '^  replicas: 1$' 'runAsUser: 65532$' 'runAsNonRoot: true$' 'type: RuntimeDefault$' 'automountServiceAccountToken: false$'
check "files host on: read-only root, no escalation, every capability dropped (NET_BIND_SERVICE back, as the edge: the binary's file capability)" \
  sec_all 'readOnlyRootFilesystem: true$' 'allowPrivilegeEscalation: false$' 'drop: \["ALL"\]$' 'add: \["NET_BIND_SERVICE"\]$'
tcp_probes_only() { sec_all 'startupProbe:$' 'livenessProbe:$' 'readinessProbe:$' 'tcpSocket:$' && ! grep -Eq 'httpGet|exec:' "$sec"; }
check "files host on: the probes are TCP (no health path is routed on the public host)" tcp_probes_only
check "files host on: the Caddyfile is mounted read-only, the writable places are small emptyDirs" sec_all 'mountPath: /etc/caddy$' 'readOnly: true$' 'sizeLimit: 16Mi$'
check "files host on: it holds no secret (no Secret, no secret environment variable, no credential of RustFS)" fails grep -Eq 'secretKeyRef|secretName|RUSTFS_|ACCESS_KEY|SECRET' "$sec"
check "files host on: it restarts when its Caddyfile changes" sec_all 'checksum/config: [0-9a-f]{64}$'
check "files host on: limits and requests are set" sec_all 'limits:$' 'memory: 128Mi$' 'requests:$'
object_counts() {
  [ "$(grep -Ec '^kind: Deployment$' "$out")" -eq 6 ] && [ "$(grep -Ec '^kind: NetworkPolicy$' "$out")" -eq 7 ] &&
    [ "$(grep -Ec '^kind: ExternalSecret$' "$out")" -eq 5 ] && ! grep -Eq '^kind: Secret$' "$out"
}
check "files host on: a Deployment and a NetworkPolicy more than with RustFS alone, no new ExternalSecret, no Secret" object_counts
# The proxy's Caddyfile.
check "files host on: Caddy's admin API is off, there is no automatic HTTPS, Traefik is the trusted proxy" fline 'admin off' 'auto_https off' 'trusted_proxies static private_ranges'
route_matcher() {
  fin '@artifact {' 'vars_regexp {http.request.uri.path} ^/artifacts/threads/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/[0-9a-f]{64}$' && fline 'handle @artifact {'
}
check "files host on: the artifact keys are the only route: the bucket, a thread's uuid and the SHA-256, matched on the path decoded once (not cleaned)" route_matcher
check "files host on: ... and a path with any % in it is refused (it is not a key: %2F, %61 and %2e%2e would match decoded and reach RustFS encoded); a % in the query is not" \
  fin '@artifact {' 'not vars_regexp {http.request.uri} ^[^?]*%'
check "files host on: ... and not on a cleaned path (path_regexp folds // and /../ into a path that matches while RustFS is sent the one that does not)" fcf_lacks 'path_regexp|^[[:space:]]+path '
last_handle_404() { tail -n 4 "$fcf" | tr -d '\t' | tr '\n' ' ' | grep -Fq 'handle { respond "not found" 404 } } '; }
check "files host on: everything else is a 404 whatever the method (the last handle)" last_handle_404
one_upstream() { [ "$(grep -Ec '^[[:space:]]*reverse_proxy ' "$fcf")" -eq 1 ] && fline 'reverse_proxy another-agentic-rustfs.another-agentic-system.svc:9000 {'; }
check "files host on: the only upstream is the RustFS Service on 9000" one_upstream
check "files host on: no file server, redirect, rewrite, sign-in or template: a proxy and nothing else" fcf_lacks '^[[:space:]]*(file_server|redir|rewrite|uri|forward_auth|basic_auth|templates|php_fastcgi|root|try_files|encode)([[:space:]]|$)'
no_put() { fline '@method not method GET HEAD OPTIONS' && ! grep -Eq 'PUT|@put|@toolarge|Content-Length' "$fcf" && fcount 'max_size 1' 1 && [ "$(grep -Ec '^[[:space:]]*max_size ' "$fcf")" -eq 1 ]; }
check "files host on: without uploads the methods are GET, HEAD and OPTIONS, and PUT and the upload's limits are nowhere in the file (the one body limit is the GET and HEAD's)" no_put
read_limited() { fline '@read method GET HEAD' && fin 'request_body @read {' 'max_size 1'; }
check "files host on: a GET or HEAD may carry one byte of body at most (request_body max_size 1: 0 would be no limit), uploads or not" read_limited
check "files host on: an OPTIONS that is a preflight of nobody is answered by the proxy (204) and never reaches RustFS: the block has its respond, and no reverse_proxy" \
  fin 'handle @options {' 'respond 204' 'header Allow "GET, HEAD, OPTIONS"'
no_proxy_in_options() { ! fin 'handle @options {' 'reverse_proxy another-agentic-rustfs.another-agentic-system.svc:9000 {'; }
check "files host on: ... the OPTIONS block holds no proxying" no_proxy_in_options
hardening_everywhere() {
  fin '(hardening) {' '>X-Content-Type-Options nosniff' ">Content-Security-Policy \"default-src 'none'; sandbox\"" '>Referrer-Policy no-referrer' '-Server' '-Via' 'defer' &&
    fcount 'import hardening' 2 && fin 'handle_errors {' 'import hardening' 'respond "{err.status_code} {err.status_text}"'
}
check "files host on: the three security headers, and no Server and no Via, are one snippet (hardening), applied to the routes and again in handle_errors, where Caddy's own 413 and 502 are answered" hardening_everywhere
no_forwarding() {
  fin 'transport http {' 'compression off' && fin 'reverse_proxy another-agentic-rustfs.another-agentic-system.svc:9000 {' 'header_up -X-Forwarded-Host' 'header_up -X-Forwarded-Proto' \
    'header_up -X-Forwarded-Port' 'header_up -X-Forwarded-Server' 'header_up -Forwarded' 'header_up -X-Real-Ip' 'header_up -Cookie' 'header_up -Via'
}
check "files host on: no compression is added or undone, and what a hop in front says about the caller (Forwarded, X-Forwarded-Host|Proto|Port|Server, X-Real-Ip) and the Cookie are not passed on" no_forwarding
check "files host on: X-Forwarded-For is kept (it is for RustFS's own log)" fcf_lacks 'header_up -X-Forwarded-For'
check "files host on: any other method is a 405 that says which are served" fline 'header @method Allow "GET, HEAD, OPTIONS"' 'respond @method "method not allowed" 405'
check "files host on: a query without a 64-digit signature is a 403 (the proxy's, with its own words)" \
  fline '@unsigned not vars_regexp {query} (^|&)X-Amz-Signature=[0-9a-f]{64}(&|$)' 'respond @unsigned "a signed URL is required" 403'
strange_query() {
  fline 'respond @strange "a signed URL is required" 403' &&
    grep -E '^[[:space:]]*@strange not vars_regexp \{query\} \^' "$fcf" | grep -Fq 'X-Amz-(Algorithm|Credential|Date|Expires|SignedHeaders|Signature|Security-Token)|response-(content-disposition|content-type|cache-control)'
}
check "files host on: a query with a member the presigner does not write (an S3 subresource: uploads, acl, tagging, ...) is a 403" strange_query
check "files host on: the Authorization header is dropped on its way to RustFS (only a signature in the query authenticates)" fline 'header_up -Authorization'
check "files host on: the Host header is not rewritten (SigV4 signs the public host; Caddy keeps it)" fcf_lacks 'header_up (-)?Host|http\.request\.(host|hostport)|\{upstream'
check "files host on: the security headers RustFS lacks are set (deferred: replaced, not added) on every answer" \
  fline '>X-Content-Type-Options nosniff' ">Content-Security-Policy \"default-src 'none'; sandbox\"" '>Referrer-Policy no-referrer'
check "files host on: RustFS's own Access-Control-* headers are dropped (exactly one layer sets them)" \
  fline 'header_down -Access-Control-Allow-Origin' 'header_down -Access-Control-Allow-Methods' 'header_down -Access-Control-Allow-Headers' \
    'header_down -Access-Control-Allow-Credentials' 'header_down -Access-Control-Expose-Headers' 'header_down -Access-Control-Max-Age'
check "files host on: no origin listed means no CORS at all: no preflight, no origin matcher, no Vary, no header of ours named Access-Control" fcf_lacks '@preflight|@origin|Vary|^[[:space:]]+header [^ ]*[ ]?Access-Control'
log_without_query() {
  fcount 'import no_query' 2 && fline 'request>uri regexp \?.* ""' 'request>headers>Referer delete' 'format filter {' 'wrap json' 'output stdout' &&
    ! grep -Eq 'format (json|console|transform)|log_credentials|query \{' "$fcf"
}
check "files host on: the access log and the default log (runtime and errors) both run through the filter that empties the query string and drops the Referer" log_without_query
check "files host on: the Caddyfile of the default shape is the golden's, byte for byte (tests/golden/files-host.Caddyfile)" golden_files files-host.Caddyfile
doc NetworkPolicy another-agentic-rustfs > "$sec"
rustfs_admits_proxy() {
  grep -Eq 'component: orchestrator$' "$sec" && grep -Eq 'component: rustfs-bucket$' "$sec" && grep -Eq 'component: files$' "$sec" && grep -Eq 'port: 9000$' "$sec" &&
    ! grep -Eq 'component: (edge|web|oauth2-proxy|chat|browser)$' "$sec" && ! grep -Eq 'ipBlock|namespaceSelector' "$sec"
}
check "files host on: RustFS admits the orchestrator, the bucket Job and the proxy, on 9000 only, and still nobody else (agents come in through the public host)" rustfs_admits_proxy
doc NetworkPolicy another-agentic-files > "$sec"
proxy_from_controller() { sec_all '^    - ports:$' 'port: 8080$' '^      from:$' 'kubernetes.io/metadata.name: traefik$' && ! grep -Eq 'ipBlock|podSelector: \{\}' "$sec"; }
check "files host on: its policy admits its port from the ingress controller's namespace and from nowhere else (the namespace is required: an open proxy would trust forwarding headers from any pod)" proxy_from_controller
proxy_out_narrow() {
  grep -Eq '^    - Egress$' "$sec" && grep -Eq 'port: 53$' "$sec" && grep -Eq 'component: rustfs$' "$sec" && grep -Eq 'port: 9000$' "$sec" &&
    ! grep -Eq 'ipBlock|port: 443|component: (orchestrator|edge|web|chat|oauth2-proxy)$' "$sec"
}
check "files host on: it may go out to DNS and to RustFS on 9000, to no address block, no port 443 and no other pod" proxy_out_narrow
render -f "$rf_values" -f "$fh_values" --set networkPolicy.ingressControllerNamespace=ingress-nginx
doc NetworkPolicy another-agentic-files > "$sec"
check "files host on, another ingress controller's namespace named: only that namespace reaches the proxy, on 8080" sec_all 'kubernetes.io/metadata.name: ingress-nginx$' 'port: 8080$'
render -f "$rf_values" -f "$fh_values" --set networkPolicy.enabled=false --set networkPolicy.ingressControllerNamespace=
proxy_without_policies() { ! grep -Eq '^kind: NetworkPolicy$' "$out" && grep -Eq 'name: another-agentic-files$' "$out"; }
check "files host on, NetworkPolicies off (and so no namespace needed): none is rendered, the proxy still is" proxy_without_policies
render -f "$rf_values" -f "$fh_values" --set rustfs.ingress.enabled=false
check "files host: turning it off again gives back the RustFS-only render, byte for byte" cmp -s "$out" "$out.files-rustfs-only"
render -f "$rf_values" -f "$fh_values" --set rustfs.ingress.clusterIssuer=cert-other --set rustfs.ingress.className=other --set rustfs.ingress.tlsSecretName=files-cert --set 'rustfs.ingress.annotations.example\.org/x=y'
doc Ingress another-agentic-files > "$sec"
check "files host: an issuer, a class, a certificate Secret and an annotation of its own override the app's" sec_all 'cert-manager.io/cluster-issuer: "cert-other"' '^  ingressClassName: other$' 'secretName: files-cert$' 'example.org/x: "y"$'
render -f "$rf_values" -f "$fh_values" --set rustfs.ingress.proxy.replicas=2
check "files host: two replicas of the proxy are fine (it holds no state)" dhas Deployment another-agentic-files '^  replicas: 2$'
render -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.s3.prefix=/prod//agentic/ --set orchestrator.artifacts.s3.bucket=my.bucket
fcaddy
check "files host on, a bucket and a prefix: the route is the store's key, the prefix as the store joins it (/prod//agentic/ is prod/agentic), dots escaped" \
  fin '@artifact {' 'vars_regexp {http.request.uri.path} ^/my\.bucket/prod/agentic/threads/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/[0-9a-f]{64}$'

# With CORS and uploads.
render -f "$rf_values" -f "$fh_values" -f "$fhu_values"
config_of config.yaml "$cfg"
fcaddy
check "uploads on: the configuration has the job limit, the upload with its limits, and the public endpoint" \
  cfg_all '^  maxPerJobBytes: 1073741824$' '^  upload:$' '^    maxBytes: 1073741824$' '^    ttlSecs: 300$' "^    publicEndpoint: \"https://$fh_name\"\$"
check "uploads on: every secret key of the configuration is still a reference" test -z "$(plain_secret_in_config)"
check "uploads on: PUT is served, and no other method is" fline '@method not method GET HEAD OPTIONS PUT' 'header @method Allow "GET, HEAD, OPTIONS, PUT"'
check "uploads on: a PUT body is at most upload.maxBytes (a ceiling the bucket does not have: content-length cannot be signed)" fline '@put method PUT' 'request_body @put {' 'max_size 1073741824'
check "uploads on: the signature rules, the Authorization drop and the 404 are those of the default shape" \
  fline 'respond @unsigned "a signed URL is required" 403' 'respond @strange "a signed URL is required" 403' 'header_up -Authorization' 'handle {' 'respond "not found" 404'
check "CORS: each listed origin is matched exactly, by its own matcher, and answered as itself" \
  fline 'header Origin https://agentic.servers.segning.pro' 'header Origin tauri://localhost' \
    'header @origin0 Access-Control-Allow-Origin "https://agentic.servers.segning.pro"' 'header @origin1 Access-Control-Allow-Origin "tauri://localhost"'
cors_origin_count() { [ "$(grep -Ec '^[[:space:]]*header (@origin[0-9]+ )?Access-Control-Allow-Origin ' "$fcf")" -eq 4 ] && ! grep -Eq 'Allow-Origin ("?\*|.*\{http)|\{http\.request\.header\.Origin|header_regexp' "$fcf"; }
check "CORS: two origins, each answered on its preflight and on its reads (four lines): no wildcard and no echo of the request's Origin" cors_origin_count
cors_never_put() { fline 'header Access-Control-Allow-Methods "GET, HEAD, OPTIONS"' 'header Access-Control-Allow-Headers "DPoP, Range"' && ! grep -Eq 'Allow-Methods.*PUT|^[[:space:]]+header( @[a-z0-9]+)? Access-Control-Allow-Credentials' "$fcf"; }
check "CORS: the methods are GET, HEAD and OPTIONS, never PUT (uploads come from agents, not browsers), the headers DPoP and Range, no credentials" cors_never_put
check "CORS: the answer varies by Origin, a preflight is answered here (204) and never reaches RustFS, and Content-Disposition, Content-Length and Content-Range are exposed" \
  fline 'header Vary Origin' 'handle @preflight0 {' 'handle @preflight1 {' 'respond 204' 'header Access-Control-Max-Age 600' \
    'header @origin0 Access-Control-Expose-Headers "Content-Disposition, Content-Length, Content-Range"'
check "CORS: a preflight needs a requested method and an origin of the list; any other OPTIONS is a 204 with no CORS header" \
  fline 'header Access-Control-Request-Method *' '@options method OPTIONS' 'handle @options {' 'header Allow "GET, HEAD, OPTIONS, PUT"'
check "uploads on with CORS: the Caddyfile is the golden's, byte for byte (tests/golden/files-host-uploads.Caddyfile)" golden_files files-host-uploads.Caddyfile
render -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={https://a.example}'
fcaddy
cors_without_put() { fline 'header Origin https://a.example' && ! grep -Eq 'PUT|@put|@toolarge' "$fcf"; }
check "CORS without uploads: one origin, and still no PUT anywhere in the file" cors_without_put

# The redirect of downloads and the lifetime of its URL are written only when asked for.
render -f "$rf_values" -f "$fh_values" -f "$chart/tests/files-host-redirect.values.yaml"
config_of config.yaml "$cfg"
check "downloads: redirect and a lifetime are written under artifacts.s3, and only then" cfg_all '^    downloads: redirect$' '^    presignTtlSecs: 120$'
render -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.s3.downloads=stream
config_of config.yaml "$cfg"
check "downloads: stream, said out loud, writes nothing (the orchestrator's own default)" cfg_lacks 'downloads'
check "a public endpoint named by the deployment, equal to the files host's, renders" renders -f "$rf_values" -f "$fh_values" --set "orchestrator.artifacts.s3.publicEndpoint=https://$fh_name"
render --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=my-files --set orchestrator.artifacts.s3.endpoint=https://s3.example.org \
  --set orchestrator.artifacts.s3.publicEndpoint=https://files.example.org --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=1000
config_of config.yaml "$cfg"
other_server_endpoint() { cfg_all '^    publicEndpoint: "https://files\.example\.org"$' '^    maxBytes: 1000$' && ! grep -Eq 'another-agentic-files|files-host' "$out"; }
check "a public endpoint of another S3 server, with no files host of ours: written as given, uploads on, and no proxy anywhere" other_server_endpoint
rm -f "$out.files-off" "$out.files-rustfs-only"
refused_saying "the files host without RustFS (it fronts the RustFS of this release)" "it needs rustfs.enabled" \
  --set orchestrator.artifacts.store=s3 --set orchestrator.artifacts.s3.bucket=my-files --set rustfs.ingress.enabled=true --set rustfs.ingress.host=files.example.org
refused_saying "the files host with no host name" "rustfs.ingress.host is required" -f "$rf_values" --set rustfs.ingress.enabled=true
refused_saying "the files host named like a URL" "rustfs.ingress.host must be a lower-case DNS name" -f "$rf_values" -f "$fh_values" --set rustfs.ingress.host=https://files.example.org
refused_saying "the files host named like the app's" "must not be the app's host" -f "$rf_values" -f "$fh_values" --set rustfs.ingress.host=agentic.servers.segning.pro
l60=$(printf 'a%.0s' $(seq 60)); l63=$(printf 'b%.0s' $(seq 63)); l64=$(printf 'b%.0s' $(seq 64))
refused_saying "a files host with a label of 64 characters (a DNS label is at most 63)" "is 64 characters, at most 63" -f "$rf_values" -f "$fh_values" --set "rustfs.ingress.host=$l64.example.org"
refused_saying "a files host of 254 characters (a DNS name is at most 253)" "is 254 characters, at most 253" -f "$rf_values" -f "$fh_values" --set "rustfs.ingress.host=$l63.$l63.$l63.$(printf 'c%.0s' $(seq 62))"
check "a files host of exactly 253 characters, with labels of 63, renders" renders -f "$rf_values" -f "$fh_values" --set "rustfs.ingress.host=$l63.$l63.$l63.$(printf 'c%.0s' $(seq 61))"
check "a files host with a label of exactly 63 characters renders" renders -f "$rf_values" -f "$fh_values" --set "rustfs.ingress.host=$l63.example.org"
check "a files host with 60-character labels renders" renders -f "$rf_values" -f "$fh_values" --set "rustfs.ingress.host=$l60.example.org"
refused_saying "the files host with the NetworkPolicies on and no ingress controller namespace (the proxy trusts the private ranges' forwarding headers: it must not be open to any pod)" \
  "needs networkPolicy.ingressControllerNamespace" -f "$rf_values" -f "$fh_values" --set networkPolicy.ingressControllerNamespace=
check "the same with the NetworkPolicies off renders (the cluster's to guarantee then)" renders -f "$rf_values" -f "$fh_values" --set networkPolicy.ingressControllerNamespace= --set networkPolicy.enabled=false
check "no files host, the NetworkPolicies on and no ingress controller namespace: the edge's rule is unchanged, it still renders (this is not asked of the edge)" renders -f "$rf_values" --set networkPolicy.ingressControllerNamespace=
refused_saying "the files host with no cluster issuer anywhere" "needs a cert-manager ClusterIssuer" -f "$rf_values" -f "$fh_values" --set ingress.clusterIssuer=
refused_saying "rustfs.ingress.enabled as a string (the string false would be on)" "rustfs.ingress.enabled must be true or false" -f "$rf_values" --set-string rustfs.ingress.enabled=false
refused_saying "a wildcard CORS origin" "is not one exact origin" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={*}'
refused_saying "a CORS origin with a path" "is not one exact origin" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={https://a.example/app}'
refused_saying "a CORS origin with a trailing slash" "is not one exact origin" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={https://a.example/}'
refused_saying "a CORS origin in capitals (the browser sends it lower case)" "is not one exact origin" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={https://A.example}'
refused_saying "a CORS origin in http:// off this machine" "is http:// off this machine" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={http://a.example}'
refused_saying "a CORS origin with its scheme's default port" "names its scheme's default port" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={https://a.example:443}'
refused_saying "a CORS origin listed twice" "is listed twice" -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={https://a.example,https://a.example}'
refused_saying "CORS origins as a string" "must be a list of origins" -f "$rf_values" -f "$fh_values" --set-string rustfs.ingress.corsOrigins=https://a.example
check "CORS origins of this machine in http:// render (a loopback port, a name under .localhost)" renders -f "$rf_values" -f "$fh_values" --set 'rustfs.ingress.corsOrigins={http://127.0.0.1:1420,http://tauri.localhost,https://chat.example:8443}'
refused_saying "a public endpoint that is not the files host's, with the files host on" "leave it empty" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.s3.publicEndpoint=https://other.example.org
refused_saying "a public endpoint in http (exit 78 in production)" "must be https" -f "$rf_values" --set orchestrator.artifacts.s3.publicEndpoint=http://files.example.org
refused_saying "a public endpoint with credentials" "an https URL with a host and no credentials" -f "$rf_values" --set orchestrator.artifacts.s3.publicEndpoint=https://u:p@files.example.org
refused_saying "a public endpoint with a query" "an https URL with a host and no credentials" -f "$rf_values" --set 'orchestrator.artifacts.s3.publicEndpoint=https://files.example.org/?x=1'
refused_saying "a public endpoint with the directory store (nothing is presigned)" "needs orchestrator.artifacts.store: s3" --set orchestrator.artifacts.s3.publicEndpoint=https://files.example.org
refused_saying "downloads: redirect with no public endpoint (exit 78: the browser would have no host)" "needs a public endpoint" -f "$rf_values" --set orchestrator.artifacts.s3.downloads=redirect
refused_saying "downloads: redirect with the directory store" "needs orchestrator.artifacts.store: s3" --set orchestrator.artifacts.s3.downloads=redirect
refused_saying "downloads that is neither stream nor redirect" "must be stream or redirect" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.s3.downloads=maybe
refused_saying "a presign lifetime of 29 seconds" "presignTtlSecs must be a whole number from 30 to 3600" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.s3.presignTtlSecs=29
refused_saying "a presign lifetime of 3601 seconds" "presignTtlSecs must be a whole number from 30 to 3600" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.s3.presignTtlSecs=3601
refused_saying "a presign lifetime that is not whole" "presignTtlSecs must be a whole number" -f "$rf_values" -f "$fh_values" --set-json orchestrator.artifacts.s3.presignTtlSecs=60.5
refused_saying "a presign lifetime with the directory store" "presignTtlSecs needs orchestrator.artifacts.store: s3" --set orchestrator.artifacts.s3.presignTtlSecs=60
refused_saying "uploads with the directory store (it cannot presign)" "upload.enabled needs orchestrator.artifacts.store: s3" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=1000
refused_saying "uploads with no public endpoint (nobody could reach the bucket)" "upload.enabled needs a public endpoint" -f "$rf_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=1000
refused_saying "uploads with no thread-tools surface (request_upload is its tool)" "needs the thread-tools surface" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=1000 --set 'orchestrator.surfaces={agui}'
refused_saying "uploads with no maxBytes" "needs orchestrator.artifacts.upload.maxBytes" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true
refused_saying "uploads with maxBytes of zero" "upload.maxBytes must be a whole number from 1 to 5368709120" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=0
refused_saying "uploads with maxBytes over 5 GiB (what one PUT takes)" "upload.maxBytes must be a whole number from 1 to 5368709120" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=5368709121 --set orchestrator.artifacts.maxPerJobBytes=4294967296
refused_saying "uploads of more than a job keeps (exit 78: such a file could never be kept)" "is more than the most bytes a job keeps" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=104857601
refused_saying "uploads with a slot lifetime of 29 seconds" "upload.ttlSecs must be a whole number from 30 to 3600" -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=1000 --set orchestrator.artifacts.upload.ttlSecs=29
refused_saying "upload.enabled as a string (the string false would be on)" "upload.enabled must be true or false" -f "$rf_values" -f "$fh_values" --set-string orchestrator.artifacts.upload.enabled=false
refused_saying "a job limit over 4 GiB" "maxPerJobBytes must be a whole number from 1 to 4294967296" --set orchestrator.artifacts.maxPerJobBytes=4294967297
refused_saying "a job limit of zero" "maxPerJobBytes must be a whole number from 1 to 4294967296" --set orchestrator.artifacts.maxPerJobBytes=0
check "uploads at exactly the job limit (100 MiB by default) render" renders -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.enabled=true --set orchestrator.artifacts.upload.maxBytes=104857600
check "a disabled upload's settings are not read (nothing of it is written)" renders -f "$rf_values" -f "$fh_values" --set orchestrator.artifacts.upload.maxBytes=0
check "the proxy needs the edge's digest: without it the render is refused, as the edge's is" fails renders -f "$rf_values" -f "$fh_values" --set edge.image.digest=
render
config_of config.yaml "$cfg"

# ---- The Job that moves the directory store's files into the bucket (ADR 0058, A5.4): `orchestrator.artifacts.migrate`, off by default ----------
# Off: nothing of it is rendered. On: one hook Job (post-install, post-upgrade, after the bucket Job) that runs the pinned orchestrator image's
# `artifacts migrate --from fs --to s3`, mounts the KEPT claim of the directory store READ-ONLY and, of a Secret of its own, the two S3 credentials and
# nothing else, with the orchestrator's security context, and with a ConfigMap of its own that says `s3`. It has two orders (the one that matters is the
# first): BEFORE the orchestrator is switched (`store: fs`, the S3 target on beside it, the Job scheduled beside the orchestrator's pod because the claim
# is ReadWriteOnce), and AFTER (`store: s3`, the Job copies only the delta).
mg_values="$chart/tests/migrate.values.yaml"          # after the switch: store s3
mb_values="$chart/tests/migrate-before.values.yaml"   # before the switch: store fs, the S3 target is RustFS
ma_values="$chart/tests/migrate-allow-empty.values.yaml"
# The Job's own ConfigMap, to $jcfg.
mig_config() {
  doc ConfigMap another-agentic-artifacts-migrate | awk -v k="  config.yaml: |" '
    $0 == k { on = 1; next }
    on && /^  [^ ]/ { on = 0 }
    on { sub(/^    /, ""); print }' > "$jcfg"
}
mig_job() { doc Job another-agentic-artifacts-migrate > "$sec2"; }
mig_args() { sed -n '/^          args:$/,/^          env:$/p' "$sec2" | sed -n 's/^ *- //p' | tr '\n' ' '; }
# doc_in <file> <Kind> [name]: doc, on another render.
doc_in() { saved=$out; out=$1; shift; doc "$@"; out=$saved; }
# The bucket, region, endpoint and prefix of the `s3:` block of a configuration file (quoted or not).
s3_address() {
  awk '
    /^ *s3:$/ { match($0, /^ */); ind = RLENGTH; on = 1; next }
    on { match($0, /^ */); if (RLENGTH <= ind) on = 0; else print }' "$1" | grep -E '^ +(bucket|region|endpoint|prefix): ' | sed 's/^ *//; s/"//g' | sort
}
# same_doc <other render> <Kind> <name>: the document is there and is the same, byte for byte, in the other render. (`sh -c` does not see the functions
# of this script: a test that needs `doc` is a function, run by `check` itself, or reads a file written before.)
same_s3_address() { [ -n "$(s3_address "$1")" ] && [ "$(s3_address "$1")" = "$(s3_address "$2")" ]; }
same_doc() { [ -n "$(doc "$2" "$3")" ] && [ "$(doc_in "$1" "$2" "$3" | sha256sum)" = "$(doc "$2" "$3" | sha256sum)" ]; }
render -f "$rf_values"
cp "$out" "$out.mg-off"
config_of config.yaml "$cfg"
cp "$cfg" "$cfg.mg-off"
check "migrate off (default): no Job of it and no word of it anywhere" lacks 'artifacts-migrate|artifacts migrate'
check "migrate off (default): values.yaml has it off, and an empty claim is not accepted" sh -c "grep -A3 '^    migrate:' '$chart/values.yaml' | grep -qx '      enabled: false' && grep -A12 '^    migrate:' '$chart/values.yaml' | grep -qx '      allowEmpty: false'"
render -f "$rf_values" -f "$mg_values"
config_of config.yaml "$cfg"
check "migrate on (after the switch): the orchestrator's configuration is the one without it, byte for byte (no key for the pinned image to refuse)" cmp -s "$cfg" "$cfg.mg-off"
check "migrate on (after the switch): nothing of the render without it is changed or removed (the Job, its ConfigMap and ExternalSecret and one line of RustFS's policy are additions: the orchestrator's pod is not restarted by it)" sh -c "
  diff '$out.mg-off' '$out' | grep -E '^[<>]' | grep -Ev '^> ' ; [ \$? -eq 1 ]"
mig_job
mig_config
check "migrate on: a Job of its own, a hook after every sync and after the bucket Job (weight 10, an Argo CD wave of 10), recreated each time" sh -c "
  grep -Eq '^kind: Job\$' '$sec2' && grep -Eq 'helm.sh/hook: post-install,post-upgrade\$' '$sec2' && grep -Eq 'helm.sh/hook-weight: \"10\"\$' '$sec2' &&
  grep -Eq 'helm.sh/hook-delete-policy: before-hook-creation\$' '$sec2' && grep -Eq 'argocd.argoproj.io/sync-wave: \"10\"\$' '$sec2'"
check "migrate on: the report stays readable after success (no hook-succeeded deletion), and goes after a week" sh -c "
  ! grep -Eq 'hook-delete-policy:.*hook-(succeeded|failed)' '$sec2' && grep -Eq '^  ttlSecondsAfterFinished: 604800\$' '$sec2'"
check "migrate on: two retries, and a deadline (30 minutes) so a claim that is not there fails the sync instead of hanging it" sh -c "
  grep -Eq '^  backoffLimit: 2\$' '$sec2' && grep -Eq '^  activeDeadlineSeconds: 1800\$' '$sec2' && grep -Eq 'restartPolicy: Never\$' '$sec2'"
orch_image=$(doc Deployment another-agentic-orchestrator | sed -n 's/^ *image: //p')
mig_image=$(sed -n 's/^ *image: //p' "$sec2")
check "migrate on: the image is the orchestrator's, the pinned one" test -n "$orch_image" -a "$orch_image" = "$mig_image"
check "migrate on: every image of the render is still pinned" images_ok
check "migrate on: the command is exactly artifacts migrate --from fs --to s3 --fs-root <the claim's mount>, no --dry-run and no --allow-empty" \
  test "$(mig_args)" = "artifacts migrate --from fs --to s3 --fs-root /var/lib/orchestrator/artifacts "
check "migrate on: it reads the ConfigMap of its own, and has no database URL, token or secret-named variable" sh -c "
  grep -Eq 'name: ORCH_CONFIG_FILE\$' '$sec2' && grep -Eq 'value: /etc/orchestrator/config.yaml\$' '$sec2' && ! grep -Eq 'DATABASE_URL|secretKeyRef|valueFrom' '$sec2' &&
  grep -A2 '^        - name: config\$' '$sec2' | grep -Eq 'name: another-agentic-artifacts-migrate\$'"
check "migrate on: the claim is the kept one of the directory store, by its name, mounted read-only twice (the volume and the mount)" sh -c "
  grep -A3 'persistentVolumeClaim:' '$sec2' | grep -Eq 'claimName: another-agentic-artifacts\$' &&
  grep -A3 'persistentVolumeClaim:' '$sec2' | grep -Eq 'readOnly: true\$' &&
  grep -B1 -A2 'mountPath: /var/lib/orchestrator/artifacts\$' '$sec2' | grep -Eq 'readOnly: true\$'"
check "migrate on: of the Job's own Secret, the two S3 credentials and nothing else (no thread-tools key, no token, no database)" sh -c "
  grep -Eq 'secretName: another-agentic-artifacts-migrate\$' '$sec2' && [ \"\$(grep -Ec '^ +- key: ' '$sec2')\" -eq 2 ] &&
  grep -Eq '^ +- key: artifacts-s3-access-key-id\$' '$sec2' && grep -Eq '^ +- key: artifacts-s3-secret-access-key\$' '$sec2' &&
  ! grep -Eq 'thread-tools-secret|model-api-key|sharing-secret|name: db\$|/run/secrets/db|secretName: another-agentic-orchestrator' '$sec2'"
doc ExternalSecret another-agentic-artifacts-migrate > "$sec"
check "migrate on: the Job's Secret is filled by an ExternalSecret of its own with the two properties of the orchestrator's S3 credentials and no other" sh -c "
  [ \"\$(grep -Ec '^ +- secretKey: ' '$sec')\" -eq 2 ] && grep -Eq 'secretKey: artifacts-s3-access-key-id\$' '$sec' && grep -Eq 'secretKey: artifacts-s3-secret-access-key\$' '$sec' &&
  grep -Eq 'property: artifacts_s3_access_key_id\$' '$sec' && grep -Eq 'property: artifacts_s3_secret_access_key\$' '$sec' &&
  grep -Eq 'name: ssegning-aws\$' '$sec' && grep -Eq 'name: another-agentic-artifacts-migrate\$' '$sec'"
check "migrate on: the pod is the orchestrator's shape: non-root 65532, RuntimeDefault, no service account token, read-only root, nothing added" sh -c "
  grep -Eq 'runAsNonRoot: true\$' '$sec2' && grep -Eq 'runAsUser: 65532\$' '$sec2' && grep -Eq 'type: RuntimeDefault\$' '$sec2' && grep -Eq 'automountServiceAccountToken: false\$' '$sec2' &&
  grep -Eq 'readOnlyRootFilesystem: true\$' '$sec2' && grep -Eq 'allowPrivilegeEscalation: false\$' '$sec2' && grep -Eq 'drop: \[\"ALL\"\]\$' '$sec2' && ! grep -Eq 'privileged|hostPath|hostNetwork|add:' '$sec2'"
check "migrate on: it has resources, with a memory limit (it holds one file at a time)" sh -c "grep -Eq '^ +memory: 512Mi\$' '$sec2' && grep -Eq '^ +memory: 128Mi\$' '$sec2'"
doc Deployment another-agentic-orchestrator > "$sec"
check "migrate on (after the switch): the directory store's claim is still not rendered, and the orchestrator still does not mount it" sh -c "
  [ -s '$sec' ] && ! grep -Eq '^kind: PersistentVolumeClaim\$' '$out' && ! grep -Eq 'another-agentic-artifacts|mountPath: /var/lib/orchestrator/artifacts' '$sec'"
check "migrate on (after the switch): the Job has no pod affinity of its own (nothing holds the claim any more)" sh -c "! grep -Eq '^ +affinity:' '$sec2'"
check "migrate on (after the switch): the Job's configuration says s3 with the credential files" sh -c "
  grep -Eq '^  store: s3\$' '$jcfg' && ! grep -Eq '^  fs:|store: fs' '$jcfg' &&
  grep -Eq '^ +file: /run/secrets/orchestrator/artifacts-s3-access-key-id\$' '$jcfg' && grep -Eq '^ +file: /run/secrets/orchestrator/artifacts-s3-secret-access-key\$' '$jcfg'"
check "migrate on (after the switch): the Job's bucket, region and endpoint are the orchestrator's" same_s3_address "$jcfg" "$cfg"
check "migrate on: the Job's configuration has a literal value for no secret (every one is a file reference)" sh -c "
  ! grep -Eq '^ *(secret|apiKey|bearer|token|password|accessKeyId|secretAccessKey|previousSecret): .+' '$jcfg' &&
  grep -Eq '^ +file: /run/secrets/db/uri\$' '$jcfg'"
check "migrate on: no secret-named variable has a literal value" fails literal_secret_env
check "migrate on: no Secret object, no token-looking value" lacks '^kind: Secret$|(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16})'
doc NetworkPolicy another-agentic-rustfs > "$sec"
check "migrate on: RustFS admits the Job beside the orchestrator and the bucket Job, on 9000, and still no one else" sh -c "
  grep -Eq 'component: artifacts-migrate\$' '$sec' && grep -Eq 'component: orchestrator\$' '$sec' && grep -Eq 'component: rustfs-bucket\$' '$sec' && grep -Eq 'port: 9000\$' '$sec' &&
  ! grep -Eq 'component: (edge|web|oauth2-proxy|chat|browser|coder)\$' '$sec'"
check "migrate on: the Job has no policy of its own to widen (it opens no port); the policies are the RustFS one more" sh -c "
  [ \"\$(grep -Ec '^kind: NetworkPolicy\$' '$out')\" -eq 6 ]"
# Large numbers survive the round trip through the Job's own ConfigMap (YAML parsed and written by Helm).
render -f "$rf_values" -f "$mg_values" --set orchestrator.artifacts.maxPerJobBytes=4294967296
mig_config
check "migrate on: a large number of the configuration is written whole in the Job's (no exponent)" sh -c "grep -Eq '^  maxPerJobBytes: 4294967296\$' '$jcfg'"
render -f "$rf_values" -f "$mg_values" --set orchestrator.artifacts.migrate.dryRun=true
mig_job
check "migrate on with dryRun: the same command with --dry-run last" test "$(mig_args)" = "artifacts migrate --from fs --to s3 --fs-root /var/lib/orchestrator/artifacts --dry-run "
render -f "$rf_values" -f "$mg_values" -f "$ma_values"
mig_job
check "migrate on with allowEmpty: the same command with --allow-empty last" test "$(mig_args)" = "artifacts migrate --from fs --to s3 --fs-root /var/lib/orchestrator/artifacts --allow-empty "
render -f "$rf_values" -f "$mg_values" -f "$ma_values" --set orchestrator.artifacts.migrate.dryRun=true
mig_job
check "migrate on with both: --dry-run then --allow-empty" test "$(mig_args)" = "artifacts migrate --from fs --to s3 --fs-root /var/lib/orchestrator/artifacts --dry-run --allow-empty "
render -f "$rf_values" -f "$mg_values" --set orchestrator.artifacts.migrate.activeDeadlineSeconds=7200 --set orchestrator.artifacts.migrate.resources.limits.memory=2Gi
mig_job
check "migrate on with a longer deadline and more memory: they are written" sh -c "grep -Eq '^  activeDeadlineSeconds: 7200\$' '$sec2' && grep -Eq '^ +memory: 2Gi\$' '$sec2'"
render -f "$rf_values" -f "$mg_values" --set networkPolicy.enabled=false
check "migrate on without network policies: the Job is rendered and no policy is" sh -c "grep -Eq '^kind: Job\$' '$out' && ! grep -Eq '^kind: NetworkPolicy\$' '$out'"
render -f "$mg_values" --set rustfs.enabled=false --set orchestrator.artifacts.s3.bucket=agentic-files --set orchestrator.artifacts.s3.endpoint=https://s3.example.org
mig_job
check "migrate on another S3 server: the Job is rendered, and RustFS and its policy are not" sh -c "
  grep -Eq '^kind: Job\$' '$sec2' && ! grep -Eq 'rustfs' '$out'"
render -f "$rf_values" -f "$mg_values" --set 'podLabels.team=agents' --set 'podAnnotations.note=x'
mig_job
check "migrate on: the pod labels and annotations of the release reach the Job's pod" sh -c "grep -Eq '^        team: agents\$' '$sec2' && grep -Eq '^        note: x\$' '$sec2'"
render -f "$rf_values" -f "$mg_values" --set externalSecrets.enabled=false
check "migrate on with externalSecrets.enabled=false: the Job is rendered with its Secret named, and no ExternalSecret is" sh -c "
  grep -Eq 'secretName: another-agentic-artifacts-migrate\$' '$out' && ! grep -Eq '^kind: ExternalSecret\$' '$out'"

# BEFORE the switch (the order that matters): the orchestrator still serves from the directory while the Job copies into the bucket.
render -f "$mb_values"
config_of config.yaml "$cfg"
mig_job
mig_config
check "migrate before the switch: the orchestrator's configuration still says fs, with the directory and no s3" sh -c "
  grep -Eq '^  store: fs\$' '$cfg' && grep -Eq '^  fs: \{ root: /var/lib/orchestrator/artifacts \}\$' '$cfg' && ! grep -Eq 's3' '$cfg'"
doc Deployment another-agentic-orchestrator > "$sec"
check "migrate before the switch: the orchestrator still mounts the claim, read-write, and gets no S3 credential" sh -c "
  grep -Eq 'claimName: another-agentic-artifacts\$' '$sec' && ! grep -A3 'claimName: another-agentic-artifacts\$' '$sec' | grep -Eq 'readOnly' &&
  grep -B1 -A2 'mountPath: /var/lib/orchestrator/artifacts\$' '$sec' | grep -Eq 'name: artifacts\$' &&
  ! grep -B1 -A2 'mountPath: /var/lib/orchestrator/artifacts\$' '$sec' | grep -Eq 'readOnly' && ! grep -Eq 'artifacts-s3' '$sec'"
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "migrate before the switch: the claim is rendered, and the orchestrator's ExternalSecret carries no S3 credential" sh -c "
  grep -Eq '^kind: PersistentVolumeClaim\$' '$out' && [ -s '$sec' ] && ! grep -Eq 'artifacts' '$sec'"
check "migrate before the switch: the Job's configuration says s3 and has no directory, whatever the orchestrator's says" sh -c "
  grep -Eq '^  store: s3\$' '$jcfg' && ! grep -Eq '^  fs:|store: fs' '$jcfg'"
check "migrate before the switch: the Job's bucket is RustFS's, the endpoint its Service, with the two credential files" sh -c "
  grep -Eq '^    bucket: artifacts\$' '$jcfg' && grep -Eq '^    endpoint: http://another-agentic-rustfs\.another-agentic-system\.svc:9000\$' '$jcfg' &&
  grep -Eq '^    region: us-east-1\$' '$jcfg' && grep -Eq '^ +file: /run/secrets/orchestrator/artifacts-s3-access-key-id\$' '$jcfg'"
render -f "$rf_values"
config_of config.yaml "$cfg"
render -f "$mb_values"
mig_config
check "migrate before the switch: the bucket is exactly the one the switch writes (the same bucket, region, endpoint and prefix as the render of store: s3)" same_s3_address "$jcfg" "$cfg"
check "migrate before the switch: the Job is scheduled beside the orchestrator's pod (a required pod affinity on the node), because the claim is ReadWriteOnce" sh -c "
  sed -n '/^      affinity:\$/,\$p' '$sec2' | grep -Eq '^ +requiredDuringSchedulingIgnoredDuringExecution:\$' &&
  sed -n '/^      affinity:\$/,\$p' '$sec2' | grep -Eq 'app.kubernetes.io/component: orchestrator\$' &&
  sed -n '/^      affinity:\$/,\$p' '$sec2' | grep -Eq 'app.kubernetes.io/instance: another-agentic-system\$' &&
  sed -n '/^      affinity:\$/,\$p' '$sec2' | grep -Eq 'topologyKey: kubernetes.io/hostname\$' &&
  grep -Eq '^ +podAffinity:\$' '$sec2' && ! grep -Eq 'preferredDuring|podAntiAffinity' '$sec2'"
check "migrate before the switch: the claim is the same one the orchestrator holds, mounted read-only by the Job twice" sh -c "
  grep -A3 'persistentVolumeClaim:' '$sec2' | grep -Eq 'claimName: another-agentic-artifacts\$' &&
  grep -A3 'persistentVolumeClaim:' '$sec2' | grep -Eq 'readOnly: true\$' &&
  grep -B1 -A2 'mountPath: /var/lib/orchestrator/artifacts\$' '$sec2' | grep -Eq 'readOnly: true\$'"
check "migrate before the switch: of the Job's own Secret, the two S3 credentials and nothing else" sh -c "
  grep -Eq 'secretName: another-agentic-artifacts-migrate\$' '$sec2' && [ \"\$(grep -Ec '^ +- key: ' '$sec2')\" -eq 2 ] &&
  grep -Eq '^ +- key: artifacts-s3-access-key-id\$' '$sec2' && grep -Eq '^ +- key: artifacts-s3-secret-access-key\$' '$sec2' &&
  ! grep -Eq 'thread-tools-secret|model-api-key|name: db\$|/run/secrets/db|secretName: another-agentic-orchestrator' '$sec2'"
doc ExternalSecret another-agentic-artifacts-migrate > "$sec"
check "migrate before the switch: the Job has an ExternalSecret of its own for them (the orchestrator's has none), on the same two AWS properties" sh -c "
  grep -Eq 'property: artifacts_s3_access_key_id\$' '$sec' && grep -Eq 'property: artifacts_s3_secret_access_key\$' '$sec' &&
  [ \"\$(grep -Ec '^ +- secretKey: ' '$sec')\" -eq 2 ]"
check "migrate before the switch: the same pod shape, command and report as after (non-root, read-only root, nothing added)" sh -c "
  test \"\$(sed -n '/^          args:\$/,/^          env:\$/p' '$sec2' | sed -n 's/^ *- //p' | tr '\n' ' ')\" = 'artifacts migrate --from fs --to s3 --fs-root /var/lib/orchestrator/artifacts ' &&
  grep -Eq 'runAsUser: 65532\$' '$sec2' && grep -Eq 'readOnlyRootFilesystem: true\$' '$sec2' && ! grep -Eq 'privileged|hostPath|hostNetwork|add:' '$sec2'"
check "migrate before the switch: every image is still pinned" images_ok
check "migrate before the switch: no secret-named variable has a literal value" fails literal_secret_env
check "migrate before the switch: no Secret object, no token-looking value" lacks '^kind: Secret$|(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16})'
doc NetworkPolicy another-agentic-rustfs > "$sec"
check "migrate before the switch: RustFS (the target) admits the orchestrator's pods, the bucket Job and the migration Job on 9000, and no one else" sh -c "
  grep -Eq 'component: artifacts-migrate\$' '$sec' && grep -Eq 'component: orchestrator\$' '$sec' && grep -Eq 'component: rustfs-bucket\$' '$sec' && grep -Eq 'port: 9000\$' '$sec' &&
  ! grep -Eq 'component: (edge|web|oauth2-proxy|chat|browser|coder)\$' '$sec'"
render -f "$mb_values" --set orchestrator.artifacts.migrate.dryRun=true
mig_job
check "migrate before the switch with dryRun (possible now: the plan before anything moves): the command ends with --dry-run" test "$(mig_args)" = "artifacts migrate --from fs --to s3 --fs-root /var/lib/orchestrator/artifacts --dry-run "
render -f "$mb_values" --set-json 'affinity={"nodeAffinity":{"requiredDuringSchedulingIgnoredDuringExecution":{"nodeSelectorTerms":[{"matchExpressions":[{"key":"disk","operator":"Exists"}]}]}}}' --set nodeSelector.pool=agents --set-json 'tolerations=[{"key":"x","operator":"Exists"}]'
mig_job
check "migrate before the switch: the release's own affinity, node selector and tolerations are kept, and the pod affinity is added to them" sh -c "
  grep -Eq '^ +nodeAffinity:\$' '$sec2' && grep -Eq '^ +podAffinity:\$' '$sec2' && grep -Eq '^      nodeSelector:\$' '$sec2' && grep -Eq '^ +pool: agents\$' '$sec2' && grep -Eq '^      tolerations:\$' '$sec2'"
# The orchestrator is not touched by turning the Job on when the S3 target is not RustFS: its Deployment, ConfigMap and ExternalSecret are byte for byte the same.
render --set orchestrator.artifacts.s3.bucket=agentic-files --set orchestrator.artifacts.s3.endpoint=https://s3.example.org
cp "$out" "$out.mx-off"
render --set orchestrator.artifacts.s3.bucket=agentic-files --set orchestrator.artifacts.s3.endpoint=https://s3.example.org --set orchestrator.artifacts.migrate.enabled=true
mig_job
mig_config
check "migrate before the switch on another S3 server: the Job is rendered and is the only addition besides its ConfigMap and ExternalSecret (the orchestrator is byte for byte the same, so not restarted)" sh -c "
  diff '$out.mx-off' '$out' | grep -E '^[<>]' | grep -Ev '^> ' ; [ \$? -eq 1 ]"
check "migrate before the switch on another S3 server: the Job's bucket and endpoint are that server's, and RustFS is not rendered" sh -c "
  grep -Eq '^    bucket: agentic-files\$' '$jcfg' && grep -Eq '^    endpoint: https://s3.example.org\$' '$jcfg' && ! grep -Eq 'rustfs' '$out'"
check "migrate before the switch: the orchestrator's Deployment does not change when the Job is turned on" same_doc "$out.mx-off" Deployment another-agentic-orchestrator
check "migrate before the switch: the orchestrator's ConfigMap does not change when the Job is turned on" same_doc "$out.mx-off" ConfigMap another-agentic-orchestrator
check "migrate before the switch: the orchestrator's ExternalSecret does not change when the Job is turned on" same_doc "$out.mx-off" ExternalSecret another-agentic-orchestrator
rm -f "$out.mg-off" "$cfg.mg-off" "$out.mx-off"

refused_saying "migrate with the directory store and no S3 target (nothing to copy into)" "migrate.enabled needs a bucket to copy into" --set orchestrator.artifacts.migrate.enabled=true
refused_saying "rustfs.enabled with the directory store and no migration (nothing would use it)" "rustfs.enabled is the store of orchestrator.artifacts.store s3" --set rustfs.enabled=true
refused_saying "migrate before the switch with a bucket name that is not one" "orchestrator.artifacts.s3.bucket must be a bucket name" --set orchestrator.artifacts.migrate.enabled=true --set orchestrator.artifacts.s3.bucket=Not_A_Bucket
refused_saying "migrate before the switch with a bucket that has a dot and no endpoint (AWS virtual-hosted style)" "has a dot" --set orchestrator.artifacts.migrate.enabled=true --set orchestrator.artifacts.s3.bucket=my.bucket
refused_saying "migrate before the switch with RustFS and an endpoint of its own" "endpoint must be empty with rustfs.enabled" -f "$mb_values" --set orchestrator.artifacts.s3.endpoint=https://s3.example.org
refused_saying "migrate.enabled as a string (the string false would be on)" "migrate.enabled must be true or false" -f "$rf_values" --set-string orchestrator.artifacts.migrate.enabled=false
refused_saying "migrate.dryRun as a string (the string false would be on)" "migrate.dryRun must be true or false" -f "$rf_values" -f "$mg_values" --set-string orchestrator.artifacts.migrate.dryRun=false
refused_saying "migrate.allowEmpty as a string (the string false would be on)" "migrate.allowEmpty must be true or false" -f "$rf_values" -f "$mg_values" --set-string orchestrator.artifacts.migrate.allowEmpty=false
refused_saying "a migration deadline of 59 seconds" "migrate.activeDeadlineSeconds must be a whole number from 60 to 86400" -f "$rf_values" -f "$mg_values" --set orchestrator.artifacts.migrate.activeDeadlineSeconds=59
refused_saying "a migration deadline over a day" "migrate.activeDeadlineSeconds must be a whole number from 60 to 86400" -f "$rf_values" -f "$mg_values" --set orchestrator.artifacts.migrate.activeDeadlineSeconds=86401
refused_saying "a migration deadline that is not whole" "migrate.activeDeadlineSeconds must be a whole number" -f "$rf_values" -f "$mg_values" --set-json orchestrator.artifacts.migrate.activeDeadlineSeconds=60.5
refused_saying "migrate before the switch with no property for the S3 credentials (the Job's Secret would be empty)" "externalSecrets.properties.artifactsS3AccessKeyId is required" -f "$mb_values" --set externalSecrets.properties.artifactsS3AccessKeyId=
check "a migration that is off does not read its deadline" renders -f "$rf_values" --set orchestrator.artifacts.migrate.activeDeadlineSeconds=1
check "a migration that is off does not need the S3 credential properties on the directory store" renders --set externalSecrets.properties.artifactsS3AccessKeyId=
render
config_of config.yaml "$cfg"

# ---- The keys of the stored secrets (ADR 0046, slice 3): `orchestrator.userSecrets`, off by default -------------------------------------
# Off: no key is asked of the AWS secret and the configuration has no `secrets` section, so an image built before the section reads the
# render (an ExternalSecret that asks for a property the AWS secret lacks stops the whole Secret syncing, which is why turning it on is the
# owner's step, after adding the property). On: the property as a file in the orchestrator's Secret, mounted, and named in `secrets.pg.keys`.
us_values="$chart/tests/user-secrets.values.yaml"
us_rotation="$chart/tests/user-secrets-rotation.values.yaml"
checksums_of() { doc Deployment another-agentic-orchestrator | grep -E 'checksum/(config|secrets):'; }
render
config_of config.yaml "$cfg"
cp "$out" "$out.us-off"
cp "$cfg" "$cfg.us-off"
checksums_of > "$out.us-off-sums"
doc ExternalSecret another-agentic-chat > "$out.us-off-chat"
doc ExternalSecret another-agentic-oauth2-proxy > "$out.us-off-oauth"
check "values.yaml: orchestrator.userSecrets.enabled is false by default" sh -c "awk '/^  userSecrets:/{m=1} m && /^    enabled:/{print \$2; exit}' '$chart/values.yaml' | grep -qx false"
check "user secrets off (default): the configuration has no secrets section" cfg_lacks '^secrets:'
check "user secrets off (default): no property, file, mount or key id of it anywhere in the render" lacks 'user_secrets_key|user-secrets-key|secrets\.pg'

render -f "$us_values"
config_of config.yaml "$cfg"
check "user secrets on: the configuration names the sealed Postgres store with its key as a { file } reference and current" cfg_all '^secrets:$' '^  store: pg$' '^  pg:$' '^      - id: "k1"$' '^        key: \{ file: /run/secrets/orchestrator/user-secrets-key-k1 \}$' '^    current: "k1"$'
check "user secrets on: no key is written in the configuration, only a file reference" cfg_lacks '^        key: [^{]'
check "user secrets on: every secret key of the configuration is still a reference" test -z "$(plain_secret_in_config)"
check "user secrets on: the configuration gains the seven lines of the secrets section and loses none" sh -c "
  [ \"\$(diff '$cfg.us-off' '$cfg' | grep -c '^>')\" -eq 7 ] && [ \"\$(diff '$cfg.us-off' '$cfg' | grep -c '^<')\" -eq 0 ]"
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "user secrets on: the orchestrator's ExternalSecret reads user_secrets_key as user-secrets-key-k1, beside the thread tools' key" sec_all '^    - secretKey: user-secrets-key-k1$' '^        property: user_secrets_key$' '^        property: thread_tools_secret$'
doc Deployment another-agentic-orchestrator > "$sec"
check "user secrets on: the orchestrator mounts it as a file (path user-secrets-key-k1)" sec_all '^              - key: user-secrets-key-k1$' '^                path: user-secrets-key-k1$'
checksums_of > "$out.us-on-sums"
check "user secrets on: the pod restarts on both checksums (the keys are read once, at startup)" sh -c "[ \"\$(diff '$out.us-off-sums' '$out.us-on-sums' | grep -Ec '^>')\" -eq 2 ]"
es_same_as() { doc ExternalSecret "$1" | cmp -s - "$2"; }
check "user secrets on: the chat agent's ExternalSecret is the default one, byte for byte" es_same_as another-agentic-chat "$out.us-off-chat"
check "user secrets on: oauth2-proxy's ExternalSecret is the default one, byte for byte" es_same_as another-agentic-oauth2-proxy "$out.us-off-oauth"
check "user secrets on: no Secret object, no secret-named literal, still production and jwt" sh -c "
  ! grep -Eq '^kind: Secret\$' '$out' && grep -Eq '^  environment: production\$' '$cfg' && grep -Eq '^  mode: jwt\$' '$cfg'"
check "user secrets on: no secret-named environment variable has a literal value" fails literal_secret_env
check "user secrets on: no token-looking value in the render" lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
check "user secrets on: the values file the checks layer holds no key (the property name only)" sh -c "! grep -Eq '[0-9a-fA-F]{64}' '$us_values' '$us_rotation'"

# The middle of a rotation: two keys, the second current, each from its own property.
render -f "$us_values" -f "$us_rotation"
config_of config.yaml "$cfg"
check "user secrets, two keys: both are listed in order and the second is current" cfg_all '^      - id: "k1"$' '^        key: \{ file: /run/secrets/orchestrator/user-secrets-key-k1 \}$' '^      - id: "k2"$' '^        key: \{ file: /run/secrets/orchestrator/user-secrets-key-k2 \}$' '^    current: "k2"$'
doc ExternalSecret another-agentic-orchestrator > "$sec"
check "user secrets, two keys: each is read from its own property" sec_all '^    - secretKey: user-secrets-key-k1$' '^        property: user_secrets_key$' '^    - secretKey: user-secrets-key-k2$' '^        property: user_secrets_key_2$'
doc Deployment another-agentic-orchestrator > "$sec"
check "user secrets, two keys: both are mounted as files" sec_all '^                path: user-secrets-key-k1$' '^                path: user-secrets-key-k2$'
render -f "$us_values" --set-json 'orchestrator.userSecrets.keys=[{"id":"k1","property":"p1"},{"id":"2026-10.b_c","property":"p2"}]' --set orchestrator.userSecrets.current=2026-10.b_c
config_of config.yaml "$cfg"
check "user secrets: an id of the whole alphabet (digits, dot, underscore, dash) is written quoted, as text" cfg_has '^      - id: "2026-10.b_c"$'
render --set orchestrator.userSecrets.enabled=true --set-json 'orchestrator.userSecrets.keys=[{"id":"k1","property":"other_key_property"}]'
check "user secrets: the AWS property is a value" has 'property: other_key_property$'
render
check "user secrets: turning it off again gives back the default render, byte for byte" cmp -s "$out" "$out.us-off"
rm -f "$out.us-off" "$cfg.us-off" "$out.us-off-sums" "$out.us-on-sums" "$out.us-off-chat" "$out.us-off-oauth"

one_key() { printf '[{"id":"%s","property":"%s"}]' "$1" "$2"; }
refused "user secrets on with no key" --set orchestrator.userSecrets.enabled=true --set-json 'orchestrator.userSecrets.keys=[]'
refused "user secrets with a current key that is not one of the keys" -f "$us_values" --set orchestrator.userSecrets.current=k9
refused "user secrets with no current key" -f "$us_values" --set orchestrator.userSecrets.current=
refused "user secrets with a key id used twice" -f "$us_values" --set-json 'orchestrator.userSecrets.keys=[{"id":"k1","property":"p1"},{"id":"k1","property":"p2"}]'
refused "user secrets with a key id of another alphabet (a space)" -f "$us_values" --set-json "orchestrator.userSecrets.keys=$(one_key 'k 1' p1)" --set 'orchestrator.userSecrets.current=k 1'
refused "user secrets with a key id of another alphabet (a slash)" -f "$us_values" --set-json "orchestrator.userSecrets.keys=$(one_key 'k/1' p1)" --set 'orchestrator.userSecrets.current=k/1'
refused "user secrets with an empty key id" -f "$us_values" --set-json "orchestrator.userSecrets.keys=$(one_key '' p1)" --set orchestrator.userSecrets.current=
long=$(printf 'x%.0s' $(seq 65))
refused "user secrets with a key id over 64 characters" -f "$us_values" --set-json "orchestrator.userSecrets.keys=$(one_key "$long" p1)" --set "orchestrator.userSecrets.current=$long"
refused "user secrets with a key that names no AWS property" -f "$us_values" --set-json "orchestrator.userSecrets.keys=$(one_key k1 '')"
refused "user secrets with two keys read from one AWS property (two ids, one key: a rotation that rotates nothing)" -f "$us_values" --set-json 'orchestrator.userSecrets.keys=[{"id":"k1","property":"p"},{"id":"k2","property":"p"}]'
many=$(for n in $(seq 17); do printf '{"id":"x%s","property":"p%s"},' "$n" "$n"; done)
refused "user secrets with more than sixteen keys" -f "$us_values" --set-json "orchestrator.userSecrets.keys=[${many%,}]" --set orchestrator.userSecrets.current=x1
check "user secrets with sixteen keys" renders -f "$us_values" --set-json "orchestrator.userSecrets.keys=[$(for n in $(seq 16); do printf '{"id":"x%s","property":"p%s"},' "$n" "$n"; done | sed 's/,$//')]" --set orchestrator.userSecrets.current=x16
refused "orchestrator.userSecrets.enabled as a string" --set-string orchestrator.userSecrets.enabled=false
refused "orchestrator.userSecrets.keys as a string" -f "$us_values" --set-string orchestrator.userSecrets.keys=k1
check "user secrets on with ExternalSecrets off needs no property name (the Secrets are the deployment's)" renders -f "$us_values" --set externalSecrets.enabled=false --set-json "orchestrator.userSecrets.keys=$(one_key k1 '')"

# The migration Job (ADR 0058) is built from the orchestrator's configuration, and mounts the two S3 credentials and nothing else: with both on, its
# configuration must not name the key files of `secrets`, which it does not mount. The orchestrator's own configuration keeps them.
for label in "after the switch" "before the switch"; do
  if [ "$label" = "after the switch" ]; then values="-f $rf_values -f $mg_values"; else values="-f $mb_values"; fi
  # shellcheck disable=SC2086 # the values files are paths without spaces, word-split on purpose
  render $values -f "$us_values" -f "$us_rotation"
  config_of config.yaml "$cfg"
  mig_config
  mig_job
  check "user secrets and the migration Job ($label): the orchestrator's configuration names the keys" cfg_all '^secrets:$' '^      - id: "k1"$' '^      - id: "k2"$'
  check "user secrets and the migration Job ($label): the Job's configuration has no secrets section and names no key file of it" sh -c "
    [ -s '$jcfg' ] && ! grep -Eq '^secrets:' '$jcfg' && ! grep -Eq 'user-secrets-key|secrets\.pg' '$jcfg'"
  check "user secrets and the migration Job ($label): the Job still has its own configuration (the bucket) and mounts no key of the secrets" sh -c "
    grep -Eq '^  store: s3\$' '$jcfg' && ! grep -Eq 'user-secrets-key' '$sec2'"
done

# ---- Calls from the apps (ADR 0047): `orchestrator.cors.allowedOrigins`, empty by default ---------------------------------------
# Empty: no `cors` key and no preflight route. With origins (only in browser mode): the orchestrator answers CORS for exactly those, and
# the edge passes a preflight of /api or /agui (OPTIONS with Origin and Access-Control-Request-Method, no credentials) to it without
# oauth2-proxy; everything else is routed as before.
render
config_of config.yaml "$cfg"
check "cors off (default): the configuration has no cors section" cfg_lacks '^  cors:'
check "cors off (default): no preflight route" lacks '@preflight'
check "values.yaml: orchestrator.cors.allowedOrigins is empty by default" sh -c "grep -A1 '^  cors:' '$chart/values.yaml' | grep -qx '    allowedOrigins: \[\]'"
render --set auth.browser.enabled=true
cp "$out" "$out.cors-off"
render --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={tauri://localhost,http://tauri.localhost}'
config_of config.yaml "$cfg"
check "cors on: the orchestrator allows exactly the listed origins (server.cors)" cfg_all '^  cors:$' '^    allowedOrigins:$' '^      - "tauri://localhost"$' '^      - "http://tauri.localhost"$'
check "cors on: no wildcard origin anywhere" fails grep -Eq 'allowedOrigins:.*\*|- "\*"' "$cfg"
check "cors on: ... OPTIONS only" matcher_has @preflight 'method OPTIONS'
check "cors on: ... /api/* and /agui/* only" matcher_has @preflight 'path /api/* /agui/*'
check "cors on: ... with an Origin" matcher_has @preflight 'header Origin *'
check "cors on: ... and an Access-Control-Request-Method" matcher_has @preflight 'header Access-Control-Request-Method *'
check "cors on: the preflight goes to the orchestrator" in_block @preflight 'another-agentic-orchestrator:8080'
check "cors on: ... with no sign-in and no credential riding along" not_in_block @preflight 'forward_auth|copy_headers|oauth2-proxy'
check "cors on: ... Authorization removed" in_block @preflight 'header_up -Authorization'
preflight_before_api() { caddyfile | grep -E "^${T}handle " | awk '{ o = o $2 "|" } END { exit (o ~ /@preflight.*\/api\/\*.*\/agui\/\*/) ? 0 : 1 }'; }
check "cors on: the preflight route is declared before /api/* and /agui/*" preflight_before_api
check "cors on: /api/* and /agui/* are still behind forward_auth for what is neither DPoP nor a preflight" count '^\s+copy_headers Authorization$' 2
render --set auth.browser.enabled=true --set-json 'orchestrator.cors.allowedOrigins=[]'
check "cors: an empty list again gives back the browser render, byte for byte" cmp -s "$out" "$out.cors-off"
rm -f "$out.cors-off"
render
config_of config.yaml "$cfg"
refused "orchestrator.cors.allowedOrigins without auth.browser.enabled (the apps send DPoP)" --set 'orchestrator.cors.allowedOrigins={tauri://localhost}'
refused "orchestrator.cors.allowedOrigins with a wildcard" --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={*}'
refused "orchestrator.cors.allowedOrigins with a path" --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={https://a.example/app}'
refused "orchestrator.cors.allowedOrigins with a trailing slash" --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={https://a.example/}'
refused "orchestrator.cors.allowedOrigins as a string" --set auth.browser.enabled=true --set-string orchestrator.cors.allowedOrigins=tauri://localhost
# What the production orchestrator would refuse at startup (exit 78, a crashloop) is refused here instead.
refused "orchestrator.cors.allowedOrigins with http:// off this machine" --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={http://chat.example}'
refused "orchestrator.cors.allowedOrigins with https's default port" --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={https://chat.example:443}'
refused "orchestrator.cors.allowedOrigins with http's default port" --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={http://localhost:80}'
check "orchestrator.cors.allowedOrigins takes http:// of this machine (a loopback port, a name under .localhost)" renders --set auth.browser.enabled=true --set 'orchestrator.cors.allowedOrigins={http://127.0.0.1:1420,http://tauri.localhost,https://chat.example:8443}'
# The issuer's origin is written into the web's policy header as it is (WEB_CSP_CONNECT_SRC): nothing but an origin gets there.
refused "auth.browser.enabled with an issuer whose origin carries a ;" --set auth.browser.enabled=true --set 'auth.issuer=https://id.example;script-src/realms/v'
refused "auth.browser.enabled with an issuer whose origin carries a quote" --set auth.browser.enabled=true --set "auth.issuer=https://id.example'x/realms/v"
refused "auth.browser.enabled with an issuer whose origin carries a space" --set auth.browser.enabled=true --set 'auth.issuer=https://id.example x/realms/v'

# ---- Claimed Android/iOS HTTPS link (ADR 0047) ----------------------------------------------------------------------
# The hostname's association documents are public only after the real release certificate and Apple app identity are named.
# They live in the web static server; the edge exposes exactly those two paths without oauth2-proxy.
render --set auth.browser.enabled=true --set mobileAppLinks.enabled=true \
  --set 'mobileAppLinks.android.sha256CertFingerprints={AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99}' \
  --set mobileAppLinks.ios.appId=ABCDE12345.com.vymalo.agentic \
  --set 'orchestrator.cors.allowedOrigins={tauri://localhost,http://tauri.localhost}'
config_of config.yaml "$cfg"
check "mobile links: one public ConfigMap holds Android's association" dhas ConfigMap another-agentic-web-mobile-app-links 'package_name":"com\.vymalo\.agentic"'
check "mobile links: the Android release certificate fingerprint is rendered" dhas ConfigMap another-agentic-web-mobile-app-links 'AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99'
check "mobile links: iOS's application identifier and only the mobile callback are rendered" dhas ConfigMap another-agentic-web-mobile-app-links 'ABCDE12345\.com\.vymalo\.agentic.*"/mobile/auth/callback"'
check "mobile links: the web pod mounts the association documents and rolls on their change" dhas Deployment another-agentic-web 'checksum/mobile-app-links:|mountPath: /srv/mobile-app-links'
check "mobile links: only the two association paths bypass oauth2-proxy" dhas ConfigMap another-agentic-edge 'path /\.well-known/assetlinks\.json /\.well-known/apple-app-site-association'
check "mobile links: the edge sends the documents to the web without an identity header" dhas ConfigMap another-agentic-edge '@mobileAppLinks|header_up -Authorization|header_up -X-Auth-Request-Email'
check "mobile links: CORS includes Android and iOS webview origins" cfg_all '^      - "tauri://localhost"$' '^      - "http://tauri.localhost"$'
check "mobile links: the static server serves association JSON rather than the HTML fallback" all_in "$repo/web/Caddyfile" '@androidAppLinks path /\.well-known/assetlinks\.json' '@iosUniversalLinks path /\.well-known/apple-app-site-association' 'root \* /srv/mobile-app-links' 'header Content-Type application/json'
render
config_of config.yaml "$cfg"
refused "mobile links without Android's release certificate" --set auth.browser.enabled=true --set mobileAppLinks.enabled=true --set mobileAppLinks.ios.appId=ABCDE12345.com.vymalo.agentic --set 'orchestrator.cors.allowedOrigins={tauri://localhost,http://tauri.localhost}'
refused "mobile links with a malformed Android certificate fingerprint" --set auth.browser.enabled=true --set mobileAppLinks.enabled=true --set 'mobileAppLinks.android.sha256CertFingerprints={aa:bb}' --set mobileAppLinks.ios.appId=ABCDE12345.com.vymalo.agentic --set 'orchestrator.cors.allowedOrigins={tauri://localhost,http://tauri.localhost}'
refused "mobile links without iOS's Apple Team ID" --set auth.browser.enabled=true --set mobileAppLinks.enabled=true --set 'mobileAppLinks.android.sha256CertFingerprints={AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99}' --set 'orchestrator.cors.allowedOrigins={tauri://localhost,http://tauri.localhost}'
refused "mobile links without Android's CORS origin" --set auth.browser.enabled=true --set mobileAppLinks.enabled=true --set 'mobileAppLinks.android.sha256CertFingerprints={AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99:AA:BB:CC:DD:EE:FF:00:11:22:33:44:55:66:77:88:99}' --set mobileAppLinks.ios.appId=ABCDE12345.com.vymalo.agentic --set 'orchestrator.cors.allowedOrigins={tauri://localhost}'

# ---- Backups of the database cluster (issue #151; README.md, "Backups"): `backup.enabled`, off by default ---------------------------------
# Off, nothing of it is rendered (the Cluster's documents are tests/golden/db-default.yaml, byte for byte, in the checks above). On, the barman-cloud
# plugin archives the WAL and takes a base backup to S3: an ObjectStore and a ScheduledBackup of the cluster's name, the plugin in the Cluster's
# `spec.plugins`, and the credentials in an ExternalSecret that reads two properties of the AWS secret, which are not the artifacts'.
bk_values="$chart/tests/backup.values.yaml"
bk_aws="$chart/tests/backup-aws.values.yaml"
render
check "backup: off by default in values.yaml (enabled false, no bucket, no region)" sh -c "
  awk '/^backup:/ { b = 1; next } b && /^[^ #]/ { b = 0 } b' '$chart/values.yaml' | grep -Eq '^  enabled: false\$' &&
  awk '/^backup:/ { b = 1; next } b && /^[^ #]/ { b = 0 } b' '$chart/values.yaml' | grep -Eq '^    bucket: \"\"\$' &&
  awk '/^backup:/ { b = 1; next } b && /^[^ #]/ { b = 0 } b' '$chart/values.yaml' | grep -Eq '^    region: \"\"\$'"
check "backup: the default render has no ObjectStore, no ScheduledBackup, no plugin on the Cluster and no backup Secret" lacks '^kind: (ObjectStore|ScheduledBackup)$|barman|^  plugins:$|db-backup'
bk_no_plugin() { ! doc Cluster another-agentic-db | grep -Eq '^  plugins:|isWALArchiver'; }
check "backup: the default render's Cluster has no plugins and no archiver (the golden holds the rest)" bk_no_plugin
cp "$out" "$out.def"

# On, with every knob away from its default (an S3-compatible server).
render -f "$bk_values"
doc Cluster another-agentic-db > "$sec"
bk_plugin() {
  sec_all '^  plugins:$' &&
    line_after "$sec" '^  plugins:$' 1 '^    - name: barman-cloud\.cloudnative-pg\.io$' &&
    line_after "$sec" '^    - name: barman-cloud\.cloudnative-pg\.io$' 1 '^      isWALArchiver: true$' &&
    line_after "$sec" '^      isWALArchiver: true$' 1 '^      parameters:$' &&
    line_after "$sec" '^      parameters:$' 1 '^        barmanObjectName: another-agentic-db$'
}
check "backup: the Cluster loads the barman-cloud plugin as the WAL archiver, naming the ObjectStore of its own name" bk_plugin
bk_server_name_set() { line_after "$sec" '^        barmanObjectName: another-agentic-db$' 1 '^        serverName: "another-agentic-db-2"$' && [ "$(grep -Ec '^ +serverName:' "$sec")" -eq 1 ]; }
check "backup: the fixture's backup.serverName is the plugin entry's serverName parameter, below barmanObjectName, and the Cluster has no other" bk_server_name_set
# The two restore examples are the README's: what they name is what the chart renders (a rename in the chart breaks a restore nobody has rehearsed).
bk_block() { # bk_block <top-level key of spec> <file>: its lines under `spec:`, quotes dropped (the render quotes, the examples do not)
  awk -v k="  $1:" '$0 == k { on = 1; print; next } on && /^    / { print; next } { on = 0 }' "$2" | tr -d '"'
}
bk_cutover_example() {
  ex="$chart/examples/restore-cutover.yaml"
  bk_block plugins "$sec" > "$sec2" && bk_block plugins "$ex" | cmp -s - "$sec2" &&
    bk_block storage "$sec" > "$sec2" && bk_block storage "$ex" | cmp -s - "$sec2" &&
    line_after "$ex" '^metadata:$' 1 '^  name: another-agentic-db$' &&
    line_after "$ex" '^          barmanObjectName: another-agentic-db$' 1 '^          serverName: another-agentic-db$' &&
    line_after "$ex" '^    recovery:$' 1 '^      source: source$'
}
check "backup: examples/restore-cutover.yaml has the chart's cluster name, storage and plugin entry (the fixture's backup.serverName), and reads the old server name from the ObjectStore" bk_cutover_example
bk_test_example() {
  ex="$chart/examples/restore-cluster.yaml"
  ! grep -Eq '^  plugins:' "$ex" && bk_block storage "$sec" > "$sec2" && bk_block storage "$ex" | cmp -s - "$sec2" &&
    line_after "$ex" '^metadata:$' 1 '^  name: another-agentic-db-restore$' &&
    line_after "$ex" '^          barmanObjectName: another-agentic-db$' 1 '^          serverName: another-agentic-db$'
}
check "backup: examples/restore-cluster.yaml has no plugins entry (it must not archive), the chart's storage, and reads the cluster's own server name from the ObjectStore" bk_test_example
check "backup: still one Cluster (three databases, one cluster, one object store)" nkind Cluster 1
bk_one_each() {
  nkind ObjectStore 1 && nkind ScheduledBackup 1 && grep -Eq '^apiVersion: barmancloud\.cnpg\.io/v1$' "$out" &&
    dhas ObjectStore another-agentic-db '^  name: another-agentic-db$' && dhas ScheduledBackup another-agentic-db '^  name: another-agentic-db$'
}
check "backup: one ObjectStore (barmancloud.cnpg.io/v1) and one ScheduledBackup, both named like the cluster" bk_one_each
doc ScheduledBackup another-agentic-db > "$sec"
check "backup: the ScheduledBackup uses the plugin method, the plugin's name, a backup owned by it, on the cluster, and the fixture's immediate false" sec_all \
  '^  method: plugin$' '^    name: barman-cloud\.cloudnative-pg\.io$' '^  backupOwnerReference: self$' '^    name: another-agentic-db$' '^  immediate: false$'
bk_sb_pairs() {
  line_after "$sec" '^  pluginConfiguration:$' 1 '^    name: barman-cloud\.cloudnative-pg\.io$' &&
    line_after "$sec" '^  cluster:$' 1 '^    name: another-agentic-db$'
}
check "backup: the plugin's name is under pluginConfiguration and the cluster's under cluster (not swapped)" bk_sb_pairs
bk_schedule_fields() { awk -F'"' '/^  schedule: / { n = split($2, a, " "); print n }' "$sec" | grep -qx 6; }
check "backup: the schedule has six fields, seconds first (the fixture's is 0 30 2 * * *)" bk_schedule_fields
check "backup: the schedule is the fixture's" sec_all '^  schedule: "0 30 2 \* \* \*"$'
doc ObjectStore another-agentic-db > "$sec"
check "backup: the ObjectStore keeps a recovery window of the fixture's 4w" sec_all '^  retentionPolicy: "4w"$'
check "backup: the destination is s3://<bucket>/<prefix>, and the endpoint is the server's" sec_all \
  '^    destinationPath: "s3://agentic-db-backups/netcup/prod"$' '^    endpointURL: "http://object-store\.storage\.svc:9000"$'
bk_cred_pairs() { # the key id is read from ACCESS_KEY_ID and the secret key from ACCESS_SECRET_KEY, each from the Secret of its own name (not swapped)
  line_after "$sec" '^      accessKeyId:$' 1 '^        name: another-agentic-db-backup$' && line_after "$sec" '^      accessKeyId:$' 2 '^        key: ACCESS_KEY_ID$' &&
    line_after "$sec" '^      secretAccessKey:$' 1 '^        name: another-agentic-db-backup$' && line_after "$sec" '^      secretAccessKey:$' 2 '^        key: ACCESS_SECRET_KEY$'
}
check "backup: accessKeyId is the key ACCESS_KEY_ID and secretAccessKey the key ACCESS_SECRET_KEY, each of the Secret another-agentic-db-backup (not swapped)" bk_cred_pairs
check "backup: the credentials are two references, no value (both name the Secret, one the key id and one the secret key)" sh -c "
  [ \"\$(grep -Ec '^        name: another-agentic-db-backup\$' '$sec')\" -eq 2 ] && [ \"\$(grep -Ec '^        key: ' '$sec')\" -eq 2 ] &&
  ! grep -Eiq '(accessKeyId|secretAccessKey|sessionToken): [^ ]' '$sec'"
bk_compression_pairs() { line_after "$sec" '^    wal:$' 1 '^      compression: lz4$' && line_after "$sec" '^    data:$' 1 '^      compression: snappy$'; }
check "backup: the WAL's compression is the fixture's lz4 and the base backup's its snappy (not swapped)" bk_compression_pairs
bk_env_pairs() { # each variable has its own value: the region, then the fixture's two checksum settings, which differ so that a swap shows
  line_after "$sec" '^      - name: AWS_DEFAULT_REGION$' 1 '^        value: "us-east-1"$' &&
    line_after "$sec" '^      - name: AWS_REQUEST_CHECKSUM_CALCULATION$' 1 '^        value: "when_required"$' &&
    line_after "$sec" '^      - name: AWS_RESPONSE_CHECKSUM_VALIDATION$' 1 '^        value: "when_supported"$'
}
check "backup: the sidecar gets the region as AWS_DEFAULT_REGION and the fixture's two checksum settings, each with its own value" bk_env_pairs
bk_resources_pairs() {
  grep -Eq '^    resources:$' "$sec" && line_after "$sec" '^      requests:$' 1 '^        cpu: 50m$' && line_after "$sec" '^      requests:$' 2 '^        memory: 128Mi$' &&
    line_after "$sec" '^      limits:$' 1 '^        memory: 512Mi$'
}
check "backup: the sidecar's resources are the fixture's: requests 50m and 128Mi, a limit of 512Mi (not swapped)" bk_resources_pairs
check "backup: the ObjectStore has no serverName of its own (the plugin forbids one there; it is the Cluster's parameter)" sh -c "! grep -Eq '^ +serverName:' '$sec'"
doc ExternalSecret another-agentic-db-backup > "$sec"
check "backup: the credentials' ExternalSecret reads the two backup properties of the AWS secret, on the ClusterSecretStore ssegning-aws, and only those" sec_all \
  '^    name: ssegning-aws$' '^    kind: ClusterSecretStore$' '^        key: prod/another-agentic/env$' '^        property: backup_s3_access_key_id$' '^        property: backup_s3_secret_access_key$' \
  '^    - secretKey: ACCESS_KEY_ID$' '^    - secretKey: ACCESS_SECRET_KEY$'
bk_es_pairs() { # ACCESS_KEY_ID is filled from the key id's property and ACCESS_SECRET_KEY from the secret key's (not swapped)
  line_after "$sec" '^    - secretKey: ACCESS_KEY_ID$' 3 '^        property: backup_s3_access_key_id$' &&
    line_after "$sec" '^    - secretKey: ACCESS_SECRET_KEY$' 3 '^        property: backup_s3_secret_access_key$'
}
check "backup: the secretKey ACCESS_KEY_ID reads backup_s3_access_key_id and ACCESS_SECRET_KEY reads backup_s3_secret_access_key (not swapped)" bk_es_pairs
check "backup: exactly two properties are read" sh -c "[ \"\$(grep -Ec '^        property: ' '$sec')\" -eq 2 ]"
check "backup: the Secret it makes is the one the ObjectStore names, and carries the cnpg.io/reload label (harmless: the sidecar re-reads the Secret at each call)" sec_all '^    name: another-agentic-db-backup$' '^          cnpg\.io/reload: "true"$'
check "backup: the artifacts' properties are not read by it" sh -c "! grep -Eq 'artifacts_s3' '$sec'"
check "backup: still no Secret object (the credentials are an ExternalSecret)" lacks '^kind: Secret$'
check "backup: no token-looking value in the render" lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
check "backup: no secret-named environment variable has a literal value" fails literal_secret_env
check "backup: every image is still pinned" images_ok
check "backup: five ExternalSecrets, one more than the default's four (the backup credentials')" count '^kind: ExternalSecret$' 5

# Whatever else is rendered is what the default render holds, so turning backups on restarts no pod (the pods' checksums are of externalsecrets.yaml
# and of the ConfigMaps, and the backup Secret is in a file of its own) and changes the Cluster by its `plugins` lines only.
bk_strip() { # bk_strip <render>: the render without what backups add
  awk '
    function flush() {
      if (buf != "" && buf !~ /(^|\n)kind: (ObjectStore|ScheduledBackup)\n/ && !(buf ~ /(^|\n)kind: ExternalSecret\n/ && buf ~ /\n  name: another-agentic-db-backup\n/)) printf "---\n%s", buf
      buf = ""
    }
    /^---$/ { flush(); skip = 0; next }
    /^  plugins:$/ { skip = 1; next }
    skip && /^    / { next }
    { skip = 0; buf = buf $0 "\n" }
    END { flush() }' "$1"
}
bk_additive() { bk_strip "$out.def" > "$sec"; bk_strip "$out" > "$sec2"; cmp -s "$sec" "$sec2"; }
check "backup: on, every other document is the default render's, byte for byte (no pod restarts), and the Cluster differs by its plugins block only" bk_additive

# On, with every default left alone, to AWS S3 (an empty endpoint).
render -f "$bk_aws"
doc ObjectStore another-agentic-db > "$sec"
check "backup, AWS: the destination is the bucket's root, there is no endpoint, retention is 28d and both compressions are gzip" sh -c "
  grep -Eq '^    destinationPath: \"s3://agentic-db-backups/\"\$' '$sec' && ! grep -q 'endpointURL' '$sec' && grep -Eq '^  retentionPolicy: \"28d\"\$' '$sec' &&
  [ \"\$(grep -Ec '^      compression: gzip\$' '$sec')\" -eq 2 ]"
check "backup, AWS: the region is the sidecar's AWS_DEFAULT_REGION" line_after "$sec" '^      - name: AWS_DEFAULT_REGION$' 1 '^        value: "eu-central-1"$'
check "backup, AWS: no resources key and no other variable" sh -c "! grep -Eq '^    resources:' '$sec' && [ \"\$(grep -Ec '^      - name: ' '$sec')\" -eq 1 ]"
doc ScheduledBackup another-agentic-db > "$sec"
check "backup, AWS: a daily 03:17:00 schedule and a first backup at once" sec_all '^  schedule: "0 17 3 \* \* \*"$' '^  immediate: true$'
doc Cluster another-agentic-db > "$sec"
check "backup, AWS: backup.serverName unset renders no serverName on the plugin entry (the cluster's own name is the server)" sh -c "grep -Eq '^        barmanObjectName: another-agentic-db\$' '$sec' && ! grep -q 'serverName' '$sec'"
render -f "$bk_aws" --set backup.s3.prefix=/a//b/
check "backup: a prefix is joined by its non-empty parts (/a//b/ is a/b)" dhas ObjectStore another-agentic-db '^    destinationPath: "s3://agentic-db-backups/a/b"$'
render -f "$bk_aws" --set externalSecrets.enabled=false
check "backup, ExternalSecrets off: the ObjectStore still names the Secret, which is the deployment's own, and no ExternalSecret is rendered" sh -c "
  ! grep -Eq '^kind: ExternalSecret\$' '$out' && grep -Eq '^        name: another-agentic-db-backup\$' '$out' && grep -Eq '^kind: ObjectStore\$' '$out'"
check "backup, ExternalSecrets off: the properties' names are not asked for" renders -f "$bk_aws" --set externalSecrets.enabled=false \
  --set externalSecrets.properties.backupS3AccessKeyId= --set externalSecrets.properties.backupS3SecretAccessKey=
check "backup: the properties' names are values" sh -c "
  helm template x '$chart' -n a -f '$base' -f '$bk_aws' --set externalSecrets.properties.backupS3AccessKeyId=other_id --set externalSecrets.properties.backupS3SecretAccessKey=other_key |
  grep -Ec 'property: other_(id|key)\$' | grep -qx 2"
check "backup: a retention of 28d, 27d, 1d, 1w and 4w is accepted (28 days is the ceiling)" sh -c "
  for r in 28d 27d 1d 1w 4w; do helm template x '$chart' -n a -f '$base' -f '$bk_aws' --set backup.retention=\$r >/dev/null || exit 1; done"
check "backup: a server name of 63 characters, one with digits and dashes, and a digits-only one given as a string are accepted" sh -c "
  for n in \$(printf 'a%.0s' \$(seq 63)) another-agentic-db-2 0 9-9; do helm template x '$chart' -n a -f '$base' -f '$bk_aws' --set-string backup.serverName=\$n >/dev/null || exit 1; done"
check "backup: a prefix with a dotted segment that is not only dots (a/.b, a.b/c) is accepted" sh -c "
  for p in a/.b a.b/c; do helm template x '$chart' -n a -f '$base' -f '$bk_aws' --set backup.s3.prefix=\$p >/dev/null || exit 1; done"
check "backup: a cron with a step, a list and a name (0 */15 1-5 * * MON,FRI) is accepted" renders -f "$bk_aws" --set 'backup.schedule.cron=0 */15 1-5 * * MON\,FRI'
check "backup: a hostname-style bucket with dots is accepted with an endpoint" renders -f "$bk_aws" --set backup.s3.bucket=my.bucket.example --set backup.s3.endpoint=https://s3.example.org
check "backup, off: its settings are not checked (nothing of it is written)" renders --set backup.retention=99m --set backup.schedule.cron=x --set backup.s3.bucket=A
check "backup, off: a coder Secret may have any name" renders --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p' --set 'sharedDatabase.coders[0].secretName=another-agentic-db-backup'
rm -f "$out.def"

refused "backups on without a bucket" -f "$bk_aws" --set backup.s3.bucket=
refused "a bucket that is not a bucket name" -f "$bk_aws" --set backup.s3.bucket=Agentic_DB
refused "a dotted bucket with no endpoint (AWS)" -f "$bk_aws" --set backup.s3.bucket=my.bucket.example
refused "backups on without a region" -f "$bk_aws" --set backup.s3.region=
refused "a region that is not a region name" -f "$bk_aws" --set backup.s3.region=EU_Central
refused "a backup endpoint that is not http(s)" -f "$bk_aws" --set backup.s3.endpoint=ftp://object-store:9000
refused "a backup endpoint with credentials in it" -f "$bk_aws" --set backup.s3.endpoint=http://user:pass@object-store:9000
refused "a prefix with .." -f "$bk_aws" --set backup.s3.prefix=a/../b
refused "a prefix with a space" -f "$bk_aws" --set 'backup.s3.prefix=a b'
refused "a prefix with a . segment in the middle" -f "$bk_aws" --set backup.s3.prefix=a/./b
refused "a prefix that starts with a . segment" -f "$bk_aws" --set backup.s3.prefix=./a
refused "a prefix that is only a dot" -f "$bk_aws" --set backup.s3.prefix=.
refused "a backup endpoint with a space" -f "$bk_aws" --set 'backup.s3.endpoint=http://object-store:9000/a b'
refused "a backup endpoint with a double quote" -f "$bk_aws" --set 'backup.s3.endpoint=http://object-store:9000/a"b'
refused "a backup endpoint with a single quote" -f "$bk_aws" --set "backup.s3.endpoint=http://object-store:9000/a'b"
refused "a server name in upper case" -f "$bk_aws" --set backup.serverName=Another-DB
refused "a server name that starts with a dash" -f "$bk_aws" --set backup.serverName=-db
refused "a server name that ends with a dash" -f "$bk_aws" --set backup.serverName=db-
refused "a server name with an underscore" -f "$bk_aws" --set backup.serverName=db_2
refused "a server name with a dot" -f "$bk_aws" --set backup.serverName=db.2
refused "a server name with a slash (a path into the bucket)" -f "$bk_aws" --set backup.serverName=a/b
refused "a server name of 64 characters" -f "$bk_aws" --set backup.serverName=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
refused "a server name that is a number (not a string)" -f "$bk_aws" --set backup.serverName=123
refused "a server name that is a list" -f "$bk_aws" --set-json 'backup.serverName=["a"]'
refused "backups on without the access key's AWS property" -f "$bk_aws" --set externalSecrets.properties.backupS3AccessKeyId=
refused "backups on without the secret key's AWS property" -f "$bk_aws" --set externalSecrets.properties.backupS3SecretAccessKey=
refused "the two backup credentials on one AWS property" -f "$bk_aws" --set externalSecrets.properties.backupS3SecretAccessKey=backup_s3_access_key_id
refused "the backup credentials on the artifacts' access key property (the orchestrator's pod reads that one)" -f "$bk_aws" --set externalSecrets.properties.backupS3AccessKeyId=artifacts_s3_access_key_id
refused "the backup credentials on the artifacts' secret key property" -f "$bk_aws" --set externalSecrets.properties.backupS3SecretAccessKey=artifacts_s3_secret_access_key
refused "a retention of 29 days (a window of W days keeps data W+1 days: 29 would break the 30-day sentence)" -f "$bk_aws" --set backup.retention=29d
refused "a retention of 30 days" -f "$bk_aws" --set backup.retention=30d
refused "a retention of 31 days" -f "$bk_aws" --set backup.retention=31d
refused "a retention of 5 weeks (35 days)" -f "$bk_aws" --set backup.retention=5w
refused "a retention in months" -f "$bk_aws" --set backup.retention=2m
refused "a retention of one month" -f "$bk_aws" --set backup.retention=1m
refused "a retention of 0 days" -f "$bk_aws" --set backup.retention=0d
refused "a retention of 365 days" -f "$bk_aws" --set backup.retention=365d
refused "a retention too long to be a number of days" -f "$bk_aws" --set backup.retention=99999999999999999999d
refused "a retention with no unit" -f "$bk_aws" --set-string backup.retention=28
refused "a retention with a unit that is not d or w" -f "$bk_aws" --set backup.retention=28h
refused "an empty retention" -f "$bk_aws" --set backup.retention=
refused "a Unix cron of five fields" -f "$bk_aws" --set 'backup.schedule.cron=17 3 * * *'
refused "a cron of seven fields" -f "$bk_aws" --set 'backup.schedule.cron=0 0 17 3 * * *'
refused "an empty cron" -f "$bk_aws" --set backup.schedule.cron=
refused "a cron descriptor (one field)" -f "$bk_aws" --set 'backup.schedule.cron=@daily'
refused "a cron field with a character that is not cron's" -f "$bk_aws" --set 'backup.schedule.cron=0 17 3 * * *;x'
refused "backup.enabled as a string" -f "$bk_aws" --set-string backup.enabled=true
refused "backup.enabled as a number" -f "$bk_aws" --set backup.enabled=1
refused "backup.enabled as the string false (a flag that is not a boolean is refused whatever it says)" -f "$bk_aws" --set-string backup.enabled=false
refused "backup.schedule.immediate as a string" -f "$bk_aws" --set-string backup.schedule.immediate=true
refused "a compression the WAL does not take" -f "$bk_aws" --set backup.wal.compression=lz5
refused "a compression the base backup does not take (zstd is the WAL's)" -f "$bk_aws" --set backup.data.compression=zstd
refused "an empty compression" -f "$bk_aws" --set backup.wal.compression=
refused "a sidecar variable whose name says secret" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"BACKUP_SECRET","value":"x"}]'
refused "a sidecar variable whose name says key" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"AWS_ACCESS_KEY_ID","value":"x"}]'
refused "a sidecar variable whose name says token" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"AWS_SESSION_TOKEN","value":"x"}]'
refused "a sidecar variable whose name says password" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"DB_PASSWORD","value":"x"}]'
refused "a sidecar variable set twice" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"FOO","value":"1"},{"name":"FOO","value":"2"}]'
refused "a sidecar variable that is the region (backup.s3.region sets it)" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"AWS_DEFAULT_REGION","value":"eu-west-1"}]'
refused "a sidecar variable from a Secret (valueFrom)" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"FOO","valueFrom":{"secretKeyRef":{"name":"s","key":"k"}}}]'
refused "a sidecar variable whose value is not a string" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"FOO","value":1}]'
refused "a sidecar variable with a lower-case name" -f "$bk_aws" --set-json 'backup.sidecar.env=[{"name":"foo","value":"1"}]'
refused "sidecar.env as a map" -f "$bk_aws" --set backup.sidecar.env.FOO=1
refused "sidecar resources with a key that is not requests or limits" -f "$bk_aws" --set backup.sidecar.resources.claims=x
refused "sidecar resources that are not a map" -f "$bk_aws" --set-string backup.sidecar.resources=big
refused "a coder Secret named like the backups' credentials" -f "$bk_aws" --set 'sharedDatabase.coders[0].name=coderone' --set 'sharedDatabase.coders[0].passwordProperty=p' --set 'sharedDatabase.coders[0].secretName=another-agentic-db-backup'
check "the refusal of a long retention says the key and the limit" sh -c "helm template x '$chart' -n x -f '$base' -f '$bk_aws' --set backup.retention=29d 2>&1 | grep -q 'backup.retention \"29d\" is more than 28 days'"
check "the refusal of a bad server name says the key and the rule" sh -c "helm template x '$chart' -n x -f '$base' -f '$bk_aws' --set backup.serverName=Bad_Name 2>&1 | grep -q 'backup.serverName must be a name of at most 63 lower-case letters'"
check "the refusal of months says to write the days" sh -c "helm template x '$chart' -n x -f '$base' -f '$bk_aws' --set backup.retention=2m 2>&1 | grep -q 'the unit m (months) is refused'"
check "the refusal of a five-field cron says seconds come first" sh -c "helm template x '$chart' -n x -f '$base' -f '$bk_aws' --set 'backup.schedule.cron=17 3 * * *' 2>&1 | grep -q 'backup.schedule.cron must have six fields, seconds first'"
check "the refusal of a missing property says which and what to do" sh -c "helm template x '$chart' -n x -f '$base' -f '$bk_aws' --set externalSecrets.properties.backupS3AccessKeyId= 2>&1 | grep -q 'backup.enabled needs externalSecrets.properties.backupS3AccessKeyId and backupS3SecretAccessKey'"
render

# ---- The chat agent's folder is the dev stack's ------------------------------------------------------------------------
# With every option off, the chat's folder and the checksum its pods carry are those of tests/golden/chat-default.yaml (made from the
# chart at origin/main 4dd7c27, before the browser agent and S3): a change there restarts the chat on the next sync, so it is on purpose
# or not at all. Regenerate with `helm template ... | sh tests/golden/chat-objects.sh > tests/golden/chat-default.yaml`.
render
golden_chat() { sh "$chart/tests/golden/chat-objects.sh" < "$out" | cmp -s - "$chart/tests/golden/chat-default.yaml"; }
check "everything off: the chat's folder and its checksum/agent are the golden's, byte for byte (the chat does not restart)" golden_chat
check "files/chat/instructions.md is dev/agents/chat/agent/instructions.md" cmp -s "$chart/files/chat/instructions.md" "$repo/dev/agents/chat/agent/instructions.md"
for f in subagents/planner.md subagents/writer.md subagents/researcher/instructions.md; do
  check "files/chat/$f is dev/agents/chat/agent/$f" cmp -s "$chart/files/chat/$f" "$repo/dev/agents/chat/agent/$f"
done
# Without the search pod the researcher has no search to name: its `tools:` line is dropped (a pattern that matches nothing is refused by
# adam-agent at startup), it has no mcp.json, and the chat is not let into a pod that is not there.
render
doc ConfigMap another-agentic-chat-agent > "$sec2"
check "no search pod: the chat's folder has the researcher, without a tools pattern and without an mcp.json" sh -c "
  grep -Eq '^  subagent-researcher.md: [|]\$' '$sec2' && ! grep -Eq 'tools: .[^ ]*search__' '$sec2' && ! grep -Eq 'subagent-researcher-mcp.json' '$sec2'"
check "no search pod: the chat has no search bearer and the folder has no mcp.json of the researcher" sh -c "
  ! grep -Eq 'SEARCH_MCP_TOKEN' '$out' && ! grep -Eq 'path: subagents/researcher/mcp.json' '$out'"

# ---- Agents with other names (ADR 0049) ----------------------------------------------------------------------------------
render
config_of agents.yaml "$cfg"
check "agents: the coder is shown as Adam, under its id coder and with no aliases until the pinned image reads them" sh -c "
  grep -Eq '^  id: coder\$' '$cfg' && grep -Eq '^  name: Adam\$' '$cfg' && ! grep -Eq 'aliases' '$cfg'"
check "agents: its card, its token variable and its Service keep the name coder" sh -c "
  grep -Eq 'cardUrl: http://coder\.another-agentic-system\.svc:8080/' '$cfg' && grep -Eq 'tokenEnv: CODER_A2A_TOKEN' '$cfg'"
refused "an alias that is the agent's own id" --set-json 'agents=[{"id":"chat","name":"Chat","aliases":["chat"],"cardUrl":"http://c/","tokenEnv":"CHAT_A2A_TOKEN"}]'
refused "an alias that is another agent's id" --set-json 'agents=[{"id":"chat","name":"Chat","aliases":["adam"],"cardUrl":"http://c/","tokenEnv":"CHAT_A2A_TOKEN"},{"id":"adam","name":"Adam","cardUrl":"http://a/","tokenEnv":"CODER_A2A_TOKEN"}]'
refused "an alias that two agents share" --set-json 'agents=[{"id":"chat","name":"Chat","aliases":["x"],"cardUrl":"http://c/","tokenEnv":"CHAT_A2A_TOKEN"},{"id":"adam","name":"Adam","aliases":["x"],"cardUrl":"http://a/","tokenEnv":"CODER_A2A_TOKEN"}]'
check "agents: a tool server may name an agent by its alias" renders -f "$ws_values" \
  --set-json 'agents=[{"id":"adam","name":"Adam","aliases":["coder"],"cardUrl":"http://a/","tokenEnv":"CODER_A2A_TOKEN"},{"id":"chat","name":"Chat","cardUrl":"http://c/","tokenEnv":"CHAT_A2A_TOKEN"}]'

# ---- A limit per client address at the Ingress (issue #284): `ingress.rateLimit`, off by default ---------------------------------------------------
# Off: not a trace of it, and the Ingress is what it was (the deployer's annotations as written). On: a Traefik `RateLimit` middleware with the numbers, and
# the Ingress names it in `traefik.ingress.kubernetes.io/router.middlewares` as `<namespace>-<name>@kubernetescrd`, ours first, the deployer's entries after
# it. The numbers are refused when they mean nothing (0 is "no limit" to Traefik), whether the limit is on or not. tests/ratelimit-e2e/ asks a real Traefik.
rl_values="$chart/tests/ratelimit.values.yaml"
rl_key='traefik.ingress.kubernetes.io/router.middlewares'
# rl_annotation: the value of the Ingress's router.middlewares annotation in the last render (empty if none).
rl_annotation() { doc Ingress another-agentic | sed -n "s|^    $rl_key: \(.*\)\$|\1|p"; }
rl_annotation_is() { [ "$(rl_annotation)" = "$1" ]; }
# the Middleware the last render made, by name, and the entry the Ingress ought to name for it (the release's namespace is another-agentic-system)
rl_name() { doc Middleware | sed -n 's/^  name: //p'; }
rl_ours() { printf 'another-agentic-system-%s@kubernetescrd' "$(rl_name)"; }
rl_names_its_middleware() { [ -n "$(rl_name)" ] && rl_annotation_is "$(rl_ours)"; }
# rl_one_annotation: exactly one router.middlewares annotation on the Ingress (the render's comments name the key too, so the Ingress's document is counted)
rl_one_annotation() { [ "$(doc Ingress another-agentic | grep -c 'router\.middlewares:')" -eq 1 ]; }
# rl_ingress_annotations: how many annotations the Ingress has
rl_ingress_annotations() { doc Ingress another-agentic | awk '/^  annotations:$/ { a = 1; next } a && /^    / { n++; next } a { exit } END { print n + 0 }'; }
rl_issuer_alone() { [ "$(rl_ingress_annotations)" -eq 1 ]; }
rl_no_namespace() { ! doc Middleware | grep -Eq '^  namespace:'; }
rl_helm() { helm template another-agentic-system "$chart" --namespace "${RL_NS:-another-agentic-system}" -f "$base" "$@"; }
rl_deployer_kept_off() { rl_helm --set-json "ingress.annotations={\"$rl_key\":\"a-b@kubernetescrd\"}" | grep -Fqx "    $rl_key: a-b@kubernetescrd"; }
rl_no_ingress_no_middleware() { rl_helm --set ingress.enabled=false | grep -Eqv '^kind: (Middleware|Ingress)$'; }
rl_in_namespace() { RL_NS=elsewhere rl_helm --set ingress.rateLimit.enabled=true | grep -Fq "$rl_key: elsewhere-another-agentic-ratelimit@kubernetescrd"; }
rl_bounds() { for d in 0 5; do rl_helm --set ingress.rateLimit.enabled=true --set ingress.rateLimit.sourceDepth=$d >/dev/null || return 1; done; }
rl_units() { for p in 250ms 1s 30s 2m 1h; do rl_helm --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=$p >/dev/null || return 1; done; }

render
check "rate limit off (default): no Middleware and no Traefik custom resource of any kind" lacks '^apiVersion: traefik\.io/'
check "rate limit off (default): the Ingress names no middleware" dlacks Ingress another-agentic 'router\.middlewares'
check "rate limit off (default): the Ingress's annotations are the issuer's alone" rl_issuer_alone
check "values.yaml: the limit is off by default" sh -c "grep -A2 '^  rateLimit:\$' '$chart/values.yaml' | grep -qx '    enabled: false'"
check "rate limit off: the deployer's own router.middlewares is kept as written" rl_deployer_kept_off
check "rate limit off, ingress off: renders, with neither a Middleware nor an Ingress" rl_no_ingress_no_middleware

render -f "$rl_values"
check "rate limit on: one Middleware, traefik.io/v1alpha1" sh -c "
  [ \"\$(grep -Ec '^kind: Middleware\$' '$out')\" -eq 1 ] && grep -Eq '^apiVersion: traefik\.io/v1alpha1\$' '$out'"
check "rate limit on: average, period and burst are the values'" out_all '^    average: 7$' '^    period: "500ms"$' '^    burst: 11$'
check "rate limit on: ... average first, under rateLimit" line_after "$out" '^  rateLimit:$' 1 '^    average: 7$'
check "rate limit on: ... the period follows the average" line_after "$out" '^    average: 7$' 1 '^    period: "500ms"$'
check "rate limit on: ... the burst follows the period" line_after "$out" '^    period: "500ms"$' 1 '^    burst: 11$'
check "rate limit on: the source is the address at depth 1 of X-Forwarded-For (sourceCriterion.ipStrategy.depth)" line_after "$out" '^    sourceCriterion:$' 2 '^        depth: 1$'
check "rate limit on: ... the criterion holds the strategy" line_after "$out" '^    sourceCriterion:$' 1 '^      ipStrategy:$'
check "rate limit on: the Middleware has no namespace of its own (Helm gives it the release's, as the Ingress's)" rl_no_namespace
check "rate limit on: the Ingress names the middleware as <namespace>-<name>@kubernetescrd, and it is the Middleware that was made" rl_names_its_middleware
check "rate limit on: one router.middlewares annotation in the render, on the chart's Ingress" rl_one_annotation
check "rate limit on: no IngressRoute and no second middleware" lacks '^kind: (IngressRoute|MiddlewareTCP)$'
check "rate limit on: the chart adds no access log of its own" lacks '(accesslog|accessLog)'

# the deployer's own middlewares: ours first in one comma-separated value, theirs after it, as written; their other annotations stay
render -f "$rl_values" --set-json 'ingress.annotations={"'"$rl_key"'":"sec-headers@kubernetescrd,auth-fwd@kubernetescrd","example.org/keep":"yes"}'
check "rate limit on, the deployer chains middlewares: ours first, then theirs as written, in one value" rl_annotation_is "$(rl_ours),sec-headers@kubernetescrd,auth-fwd@kubernetescrd"
check "rate limit on, the deployer chains middlewares: one router.middlewares annotation, never two" rl_one_annotation
check "rate limit on, the deployer chains middlewares: their other annotations are kept" dhas Ingress another-agentic '^    example.org/keep: "?yes"?$'
check "rate limit on, the deployer chains middlewares: the issuer annotation is kept" dhas Ingress another-agentic 'cert-manager.io/cluster-issuer: "cert-cloudflare"'
render -f "$rl_values" --set-json 'ingress.annotations={"'"$rl_key"'":""}'
check "rate limit on, the deployer's value is empty: ours alone, no trailing comma" rl_annotation_is "$(rl_ours)"
render -f "$rl_values" --set-json 'ingress.annotations={"'"$rl_key"'":"  one@kubernetescrd  "}'
check "rate limit on, the deployer's value has spaces around it: trimmed, ours first" rl_annotation_is "$(rl_ours),one@kubernetescrd"
render -f "$rl_values" --set fullnameOverride=shop
check "rate limit on, another release name: the Middleware is named for it" count '^  name: shop-ratelimit$' 1
check "rate limit on, another release name: ... and the annotation names that one" sh -c "
  grep -Fq '$rl_key: another-agentic-system-shop-ratelimit@kubernetescrd' '$out'"

# the documented starting point is what `enabled: true` alone gives (README: average 20, period 1s, burst 50, the address of the peer)
render --set ingress.rateLimit.enabled=true
check "rate limit on with no other value: 20 per 1s, burst 50, the peer's address (the documented starting point)" out_all '^    average: 20$' '^    period: "1s"$' '^    burst: 50$' '^        depth: 0$'
check "rate limit on: the annotation carries the release's namespace" rl_in_namespace

# refusals: what Traefik would take but means nothing here, and what cannot work
refused "rate limit: average 0 (Traefik reads it as no limit)" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.average=0
refused "rate limit: average negative" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.average=-5
refused "rate limit: average not whole" --set ingress.rateLimit.enabled=true --set-json 'ingress.rateLimit.average=1.5'
refused "rate limit: average a string" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.average=20
refused "rate limit: average beyond a million" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.average=1000001
refused "rate limit: burst 0" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.burst=0
refused "rate limit: burst not whole" --set ingress.rateLimit.enabled=true --set-json 'ingress.rateLimit.burst=2.5'
refused "rate limit: burst null" --set ingress.rateLimit.enabled=true --set-json 'ingress.rateLimit.burst=null'
refused "rate limit: period without a unit" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=1
refused "rate limit: period an integer (Traefik would read seconds; the chart wants the unit written)" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.period=1
refused "rate limit: period in days" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=1d
refused "rate limit: period fractional" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=1.5s
refused "rate limit: period zero" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=0s
refused "rate limit: period zero milliseconds" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=00ms
refused "rate limit: period compound" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=1m30s
refused "rate limit: period empty" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=
refused "rate limit: period negative" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.period=-1s
refused "rate limit: depth 6 (more than five proxies is a mistake)" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.sourceDepth=6
refused "rate limit: depth negative" --set ingress.rateLimit.enabled=true --set ingress.rateLimit.sourceDepth=-1
refused "rate limit: depth not whole" --set ingress.rateLimit.enabled=true --set-json 'ingress.rateLimit.sourceDepth=1.5'
refused "rate limit: depth a string" --set ingress.rateLimit.enabled=true --set-string ingress.rateLimit.sourceDepth=1
refused "rate limit: enabled with ingress.enabled false (nothing would name the middleware)" --set ingress.rateLimit.enabled=true --set ingress.enabled=false
refused "rate limit: enabled a string" --set-string ingress.rateLimit.enabled=true
refused "rate limit: the deployer's router.middlewares is not a string" --set ingress.rateLimit.enabled=true --set-json 'ingress.annotations={"'"$rl_key"'":5}'
refused "rate limit: a wrong number is refused while the limit is off too (found when written, not the day it is turned on)" --set ingress.rateLimit.average=0
refused "rate limit: ... a wrong period, off" --set-string ingress.rateLimit.period=soon
check "rate limit: depth 0 and depth 5 are accepted (the bounds)" rl_bounds
check "rate limit: the units ms, s, m and h are accepted" rl_units

# ---- Scaling the database: `database.instances`, `database.resources` and the connection pooler (issue #290; README.md, "Scaling the database") -------------
# Off by default, and then the render is the one of a chart that has never heard of it: the Cluster has `instances: 1` and no `resources`, there is no Pooler, and
# the orchestrator reads the app Secret's `uri` as a file. Scaled out: three instances with resources. With the pooler: a Pooler (transaction mode, with
# max_prepared_statements), the orchestrator's `database.url` the pooler's Service and its `database.directUrl` the cluster's own `-rw` Service, both composed by the
# Deployment from the app Secret's keys, so that no ConfigMap holds a URL or a password. The agents' databases are never pooled.
pool_values="$chart/tests/pooler.values.yaml"
db_values_block() { awk '/^database:/ { b = 1; next } b && /^[^ #]/ { b = 0 } b' "$chart/values.yaml"; }
vals_default_off() {
  db_values_block > "$sec" &&
    grep -Eq '^  instances: 1$' "$sec" && grep -Eq '^  resources: \{\}$' "$sec" && grep -Eq '^    enabled: false$' "$sec" && grep -Eq '^    resources: \{\}$' "$sec"
}
vals_pooler_parameters() {
  db_values_block > "$sec" &&
    grep -Eq '^      max_prepared_statements: "200"$' "$sec" && grep -Eq '^      max_client_conn: "1000"$' "$sec" && grep -Eq '^      default_pool_size: "20"$' "$sec" &&
    ! grep -Eq '^ +pool_?[mM]ode:' "$sec"
}
pool_strip() { # pool_strip <render>: the render without the documents the pooler changes (the Pooler, the Cluster, the orchestrator's ConfigMap and Deployment)
  awk '
    function flush() {
      if (buf != "" && buf !~ /(^|\n)kind: (Pooler|Cluster)\n/ && !(buf ~ /(^|\n)kind: (ConfigMap|Deployment)\n/ && buf ~ /\n  name: another-agentic-orchestrator\n/)) printf "---\n%s", buf
      buf = ""
    }
    /^---$/ { flush(); next }
    { buf = buf $0 "\n" }
    END { flush() }' "$1"
}
same_elsewhere() { pool_strip "$out.scale-default" > "$sec"; pool_strip "$out" > "$sec2"; cmp -s "$sec" "$sec2"; }
orchestrator_docs_same() {
  [ "$(doc_in "$out.scale-default" ConfigMap another-agentic-orchestrator | sha256sum)" = "$(doc ConfigMap another-agentic-orchestrator | sha256sum)" ] &&
    [ "$(doc_in "$out.scale-default" Deployment another-agentic-orchestrator | sha256sum)" = "$(doc Deployment another-agentic-orchestrator | sha256sum)" ]
}
checksum_moved() { # the orchestrator's pod restarts when its configuration does
  [ "$(doc_in "$out.scale-default" Deployment another-agentic-orchestrator | grep 'checksum/config:')" != "$(doc Deployment another-agentic-orchestrator | grep 'checksum/config:')" ]
}
cluster_lacks_resources() { ! doc Cluster another-agentic-db | grep -Eq '^  resources:'; }
check "scale: values.yaml defaults: database.instances 1, database.resources empty, the pooler off with no resources" vals_default_off
check "scale: values.yaml's pooler parameters are strings, max_prepared_statements among them, and the pool mode is not a value" vals_pooler_parameters
render
config_of config.yaml "$cfg"
check "scale: the default render has no Pooler, no ORCH_DB_ variable and nothing of the pooler" lacks '^kind: Pooler$|ORCH_DB_|db-pooler|poolMode'
check "scale: the default Cluster has instances: 1 ..." dhas Cluster another-agentic-db '^  instances: 1$'
check "scale: ... and no resources key" cluster_lacks_resources
check "scale: the default configuration reads the app Secret's uri file ..." cfg_has '^  url: \{ file: /run/secrets/db/uri \}$'
check "scale: ... and has no directUrl" cfg_lacks 'directUrl'
check "scale: the default Deployment mounts the app Secret's uri file" dhas Deployment another-agentic-orchestrator '^              mountPath: /run/secrets/db$'
cp "$out" "$out.scale-default"

# Three instances with resources, no pooler: only the Cluster changes.
render --set database.instances=3 --set database.resources.requests.cpu=500m --set database.resources.requests.memory=1Gi --set database.resources.limits.memory=2Gi
doc Cluster another-agentic-db > "$sec"
check "scale: database.instances and database.resources are the Cluster's spec.instances and spec.resources" sec_all '^  instances: 3$' '^  resources:$' '^    limits:$' '^      memory: 2Gi$' '^    requests:$' '^      cpu: 500m$' '^      memory: 1Gi$'
check "scale: ... spec.resources comes right before spec.storage" line_after "$sec" '^  storage:$' -1 '^      memory: 1Gi$'
check "scale: three instances alone render no Pooler and leave the orchestrator on the uri file" sh -c "! grep -Eq '^kind: Pooler\$|ORCH_DB_' '$out' && grep -Eq '/run/secrets/db/uri' '$out'"
check "scale: three instances and resources change the Cluster and nothing else (no other pod restarts)" same_elsewhere
check "scale: ... and the orchestrator's ConfigMap and Deployment are the default's" orchestrator_docs_same

# The pooler on, with a fixture away from the defaults (three instances, resources everywhere, default_pool_size 30).
render -f "$pool_values"
config_of config.yaml "$cfg"
doc Pooler another-agentic-db-pooler > "$sec"
check "pooler: one Pooler, the CloudNativePG kind, named for the release and not like the cluster (CloudNativePG refuses that)" sh -c "
  [ \"\$(grep -Ec '^kind: Pooler\$' '$out')\" -eq 1 ] && grep -Eq '^apiVersion: postgresql\\.cnpg\\.io/v1\$' '$sec' && grep -Eq '^  name: another-agentic-db-pooler\$' '$sec' && ! grep -Eq '^  name: another-agentic-db\$' '$sec'"
check "pooler: it is read/write and two pods" sec_all '^  type: rw$' '^  instances: 2$'
check "pooler: it is of the orchestrator's Cluster (the name is under spec.cluster)" line_after "$sec" '^  cluster:$' 1 '^    name: another-agentic-db$'
check "pooler: the pool mode is transaction, under spec.pgbouncer" line_after "$sec" '^  pgbouncer:$' 1 '^    poolMode: transaction$'
check "pooler: no other pool mode anywhere (session and statement mode break the orchestrator)" sh -c "! grep -Eqi 'poolMode: (session|statement)|pool_mode' '$sec'"
check "pooler: the parameters are strings in key order, max_prepared_statements among them, default_pool_size from the fixture" sec_all '^      default_pool_size: "30"$' '^      max_client_conn: "1000"$' '^      max_prepared_statements: "200"$'
check "pooler: PgBouncer's resources are on a container named pgbouncer (the operator merges by that name)" line_after "$sec" '^        - name: pgbouncer$' 1 '^          resources:$'
check "pooler: ... with the fixture's requests and limit" sec_all '^              cpu: 100m$' '^              memory: 128Mi$' '^              memory: 256Mi$'
check "pooler: the Cluster is three instances, as the fixture says" dhas Cluster another-agentic-db '^  instances: 3$'
check "pooler: database.url and database.directUrl are { env } references" cfg_all '^  url: \{ env: ORCH_DB_POOLED_URL \}$' '^  directUrl: \{ env: ORCH_DB_DIRECT_URL \}$'
check "pooler: the configuration names no file of the app Secret and holds no URL" cfg_lacks '/run/secrets/db|postgres(ql)?://'
check "pooler: no ConfigMap holds a URL, a password or a host of the database" dlacks ConfigMap "" 'postgres(ql)?://|password|db-rw|db-pooler'
doc Deployment another-agentic-orchestrator > "$sec"
# shellcheck disable=SC2016 # the text of a Kubernetes variable reference, not the shell's
pooled_url_ok() { grep -Fq 'value: "postgresql://$(ORCH_DB_USER):$(ORCH_DB_PASSWORD)@another-agentic-db-pooler.another-agentic-system.svc:5432/$(ORCH_DB_NAME)"' "$sec"; }
# shellcheck disable=SC2016
direct_url_ok() { grep -Fq 'value: "postgresql://$(ORCH_DB_USER):$(ORCH_DB_PASSWORD)@another-agentic-db-rw.another-agentic-system.svc:5432/$(ORCH_DB_NAME)"' "$sec"; }
env_before_urls() { # the kubelet expands $(NAME) from the variables declared above it: the three Secret keys come first
  awk '/- name: ORCH_DB_USER$/ { u = NR } /- name: ORCH_DB_PASSWORD$/ { p = NR } /- name: ORCH_DB_NAME$/ { n = NR } /- name: ORCH_DB_POOLED_URL$/ { a = NR } /- name: ORCH_DB_DIRECT_URL$/ { d = NR }
       END { exit !(u && p && n && a && d && u < a && p < a && n < a && u < d && p < d && n < d) }' "$sec"
}
secret_keys_ok() {
  awk '/- name: ORCH_DB_USER$/ { f = 1 } f == 1 && /key: username$/ { a = 1 } /- name: ORCH_DB_PASSWORD$/ { f = 2 } f == 2 && /key: password$/ { b = 1 } /- name: ORCH_DB_NAME$/ { f = 3 } f == 3 && /key: dbname$/ { c = 1 }
       END { exit !(a && b && c) }' "$sec" &&
    [ "$(grep -Ec '^                  name: another-agentic-db-app$' "$sec")" -eq 3 ]
}
check "pooler: ORCH_DB_POOLED_URL is the app Secret's role on the pooler's Service, port 5432" pooled_url_ok
check "pooler: ORCH_DB_DIRECT_URL is the same role on the cluster's -rw Service, port 5432" direct_url_ok
check "pooler: the three Secret keys are declared above the two URLs that name them" env_before_urls
check "pooler: username, password and dbname come from the app Secret's keys (secretKeyRef, never a value)" secret_keys_ok
check "pooler: the Deployment mounts no uri file and no db volume any more" sh -c "! grep -Eq 'mountPath: /run/secrets/db|path: uri|^        - name: db\$' '$sec'"
check "pooler: no secret-named environment variable has a literal value" fails literal_secret_env
check "pooler: every image is still pinned" images_ok
check "pooler: the orchestrator restarts when the pooler comes (the checksum of its configuration moves)" checksum_moved
externalsecrets_direct() {
  ! doc ExternalSecret "" | grep -q 'db-pooler' && doc ExternalSecret "" | grep -Eq 'uri: "postgresql://agent:.*@another-agentic-db-rw\.another-agentic-system\.svc:5432/agent"'
}
check "pooler: the agents' databases stay direct: no ExternalSecret names the pooler, the chat agent's URI is on the -rw Service" externalsecrets_direct
cp "$out" "$out.pool"
cp "$out.scale-default" "$out.keep-default"
check "pooler: turning it on changes the Cluster, the orchestrator's ConfigMap and Deployment and adds the Pooler, and no other document" same_elsewhere

# The pooler with the coders: their URIs are the direct ones.
render -f "$pool_values" -f "$chart/tests/coders.values.yaml"
coders_direct() {
  [ "$(doc ExternalSecret "" | grep -Ec 'uri: "postgresql://coder(vymalo|stephane):.*@another-agentic-db-rw\.another-agentic-system\.svc:5432/coder(vymalo|stephane)"')" -eq 2 ] && ! doc ExternalSecret "" | grep -q 'db-pooler'
}
check "pooler, two coders: each coder's database URI is on the -rw Service, and no ExternalSecret names the pooler" coders_direct

# Defaults of the pooler when only `enabled` is set: two pods, transaction, the three parameters, no template (no resources).
render --set database.pooler.enabled=true
doc Pooler another-agentic-db-pooler > "$sec"
check "pooler, enabled alone: two pods, transaction, 200 prepared statements, 1000 clients and a pool of 20" sec_all '^  instances: 2$' '^    poolMode: transaction$' '^      max_prepared_statements: "200"$' '^      max_client_conn: "1000"$' '^      default_pool_size: "20"$'
check "pooler, enabled alone: no template key (no resources, nothing to merge)" sh -c "! grep -Eq '^  template:' '$sec'"
render --set database.pooler.enabled=true --set-json 'database.pooler.parameters.max_client_conn=1000000' --set-string database.pooler.parameters.max_prepared_statements=500
check "pooler: a number from a values file or JSON (a float64) is written whole (1000000, never 1e+06), a string as it is" out_all '^      max_client_conn: "1000000"$' '^      max_prepared_statements: "500"$'
render --set database.pooler.enabled=true --set database.pooler.parameters.max_client_conn=null
doc Pooler another-agentic-db-pooler > "$sec"
check "pooler: a parameter set to null is dropped (Helm deletes the key) and the others stay" sh -c "! grep -q 'max_client_conn' '$sec' && grep -Eq '^      max_prepared_statements: \"200\"\$' '$sec'"

# Refusals, and what is accepted.
refused "database.instances 0" --set database.instances=0
refused "database.instances negative" --set database.instances=-1
refused "database.instances not whole" --set-json 'database.instances=1.5'
refused "database.instances a string" --set-string database.instances=3
refused "database.instances null" --set-json 'database.instances=null'
refused "database.instances above the typo guard (20)" --set database.instances=21
refused "database.pooler.instances 0, with the pooler on" -f "$pool_values" --set database.pooler.instances=0
refused "database.pooler.instances not whole, with the pooler on" -f "$pool_values" --set-json 'database.pooler.instances=2.5'
refused "database.pooler.instances 0, with the pooler off (found when written)" --set database.pooler.instances=0
refused "database.pooler.enabled as the string true (the string false would be on)" --set-string database.pooler.enabled=true
refused "the pooler with max_prepared_statements 0" -f "$pool_values" --set database.pooler.parameters.max_prepared_statements=0
refused "the pooler with max_prepared_statements the string 0" -f "$pool_values" --set-string database.pooler.parameters.max_prepared_statements=0
refused "the pooler with max_prepared_statements negative" -f "$pool_values" --set database.pooler.parameters.max_prepared_statements=-1
refused "the pooler with max_prepared_statements not whole" -f "$pool_values" --set-json 'database.pooler.parameters.max_prepared_statements=1.5'
refused "the pooler with max_prepared_statements not a number" -f "$pool_values" --set-string database.pooler.parameters.max_prepared_statements=lots
refused "the pooler with max_prepared_statements removed (null)" -f "$pool_values" --set database.pooler.parameters.max_prepared_statements=null
refused "the pooler with all its parameters removed" -f "$pool_values" --set-json 'database.pooler.parameters=null'
refused "the pooler with the parameters a list" -f "$pool_values" --set-json 'database.pooler.parameters=["max_prepared_statements"]'
refused_saying "the pooler with max_prepared_statements 0 says which setting" "max_prepared_statements" -f "$pool_values" --set database.pooler.parameters.max_prepared_statements=0
refused "pool_mode among the parameters (the mode is not a value)" -f "$pool_values" --set-string database.pooler.parameters.pool_mode=session
refused "poolMode among the parameters" -f "$pool_values" --set-string database.pooler.parameters.poolMode=statement
refused "a parameter that is a boolean" -f "$pool_values" --set-json 'database.pooler.parameters.server_round_robin=true'
refused "a parameter that is a map" -f "$pool_values" --set-json 'database.pooler.parameters.x_y={"a":1}'
refused "a parameter named in capitals" -f "$pool_values" --set-string database.pooler.parameters.MAX_CLIENT_CONN=10
refused "database.resources with a misspelt key (limit)" --set-json 'database.resources={"limit":{"memory":"2Gi"}}'
refused "database.resources a list" --set-json 'database.resources=[]'
refused "database.pooler.resources with a misspelt key (request)" --set-json 'database.pooler.resources={"request":{"cpu":"100m"}}'
refused "database.pooler.resources a string" --set-string database.pooler.resources=big
instances_accepted() { for n in 1 3 20; do renders --set database.instances=$n || return 1; done; }
pooler_instances_accepted() { for n in 1 2 20; do renders -f "$pool_values" --set database.pooler.instances=$n || return 1; done; }
mps_accepted() {
  for v in 1 200 5000; do renders -f "$pool_values" --set database.pooler.parameters.max_prepared_statements=$v || return 1; done
  renders -f "$pool_values" --set-string database.pooler.parameters.max_prepared_statements=200
}
check "scale: database.instances of 1, 3 and 20 are accepted" instances_accepted
check "scale: ... and the pooler's of 1, 2 and 20" pooler_instances_accepted
check "scale: max_prepared_statements 0 is not looked at while the pooler is off (nothing of it is written)" renders --set database.pooler.parameters.max_prepared_statements=0
check "scale: max_prepared_statements of 1, 200 and 5000 (numbers and strings) are accepted with the pooler on" mps_accepted
rm -f "$out.scale-default" "$out.pool" "$out.keep-default"
render

# ---- The shipped values.yaml --------------------------------------------------------------------------------------------
check "values.yaml leaves the deployment's own values empty (host, issuer, model)" sh -c "
  grep -Eq '^host: \"\"$' '$chart/values.yaml' && grep -Eq '^  issuer: \"\"$' '$chart/values.yaml' && grep -Eq '^  baseUrl: \"\"$' '$chart/values.yaml'"
check "values.yaml has no value that looks like a secret" sh -c "! grep -Eq '(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16})' '$chart/values.yaml'"
check "values.yaml has no latest tag" sh -c "! grep -Eq 'tag: \"?latest' '$chart/values.yaml'"

if [ "$fail" -eq 0 ]; then echo "render checks passed"; else echo "render checks FAILED"; exit 1; fi

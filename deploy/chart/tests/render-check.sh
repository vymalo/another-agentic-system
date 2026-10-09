#!/bin/sh
# Assertions on the rendered chart: the properties the design depends on, so a careless edit to the templates or values
# fails CI instead of exposing the deployment. Needs only `helm`, `grep`, `awk`, `sed` and `cmp`.
#
#   sh deploy/chart/tests/render-check.sh            (from the repository root)
#
# The base render is examples/netcup.values.yaml: the values of the Application on netcup, with placeholders for the model.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
chart="$here/.."
repo=$(cd "$chart/../.." && pwd)
base="$chart/examples/netcup.values.yaml"
out=$(mktemp)
cfg=$(mktemp)
sec=$(mktemp)
sec2=$(mktemp)
trap 'rm -f "$out" "$cfg" "$sec" "$sec2"' EXIT
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
check "search pod on: egress to DNS, and to the public internet except private ranges and the metadata address" sec_all 'port: 53$' 'cidr: 0.0.0.0/0' '10.0.0.0/8' '169.254.0.0/16' '172.16.0.0/12' '192.168.0.0/16' 'port: 443$'
check "search pod on: the IPv6 exceptions include the NAT64 form" sec_all 'cidr: ::/0' 'fc00::/7' 'fe80::/10' '64:ff9b::/96'
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
edge_has_no_public_route() { ! caddyfile | grep -Eq 'header_up -Authorization|public/shared|handle @public|/s/\*'; }
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

# Off (the default): the render has no trace of sharing.
render
config_of config.yaml "$cfg"
check "sharing off (default): the configuration has no sharing key and no role holds thread.share" cfg_lacks '^sharing:|thread.share'
check "sharing off (default): no sharing secret, permission or public route anywhere in the render" sharing_off_everywhere
check "sharing off (default): the edge has no public route (no header stripped, no /s/, no public/shared)" edge_has_no_public_route
check "sharing off (default): six handle blocks in the Caddyfile (health, oauth2, thread-tools, api, agui, the web)" test "$(n_handles)" -eq 6
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
check "sharing internal: six handle blocks, as with sharing off" test "$(n_handles)" -eq 6
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
check "sharing public: nine handle blocks: the six, and the three public ones, no fourth" test "$(n_handles)" -eq 9
check "sharing public: the public routes are exactly the page and its files, /api/public/shared/* and /agui/public/shared/*" test "$(public_paths)" = '/agui/public/shared/*|/api/public/shared/*|/s/* /_next/static/* /favicon.ico /icon.svg /apple-icon.png /manifest.webmanifest /brand/*|'
check "sharing public: three public blocks, each GET and HEAD only" test "$(caddyfile | grep -Ec "^${T}${T}method GET HEAD\$")" -eq 3
check "sharing public: each public block drops the client's Authorization and X-Auth-Request-Email, and asks no sign-in" public_blocks_ok
check "sharing public: the public blocks come before /api/*, /agui/* and the catch-all (Caddy takes the first match)" public_first
check "sharing public: the three protected routes still ask oauth2-proxy (forward_auth, copy_headers: three, as without sharing)" count '^\s+copy_headers Authorization$' 3
check "sharing public: the public API goes to the orchestrator and the page to the web, nothing to oauth2-proxy" public_targets
check "sharing public: the thread tools are still not routed, nor /mcp, nor webhooks" no_other_surface
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
check "browser off (default): six handle blocks in the Caddyfile" test "$(n_handles)" -eq 6
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
check "browser on: nine handle blocks: the six, the public auth route and the two DPoP ones, no tenth" test "$(n_handles)" -eq 9
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
check "browser on with public sharing: twelve handle blocks (the six, the three public links, the three of the browser)" test "$(n_handles)" -eq 12
check "browser on with public sharing: the public links keep their own routes, stripped of identity" public_blocks_ok
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

# ---- The chat agent's folder is the dev stack's ------------------------------------------------------------------------
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

# ---- The shipped values.yaml --------------------------------------------------------------------------------------------
check "values.yaml leaves the deployment's own values empty (host, issuer, model)" sh -c "
  grep -Eq '^host: \"\"$' '$chart/values.yaml' && grep -Eq '^  issuer: \"\"$' '$chart/values.yaml' && grep -Eq '^  baseUrl: \"\"$' '$chart/values.yaml'"
check "values.yaml has no value that looks like a secret" sh -c "! grep -Eq '(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16})' '$chart/values.yaml'"
check "values.yaml has no latest tag" sh -c "! grep -Eq 'tag: \"?latest' '$chart/values.yaml'"

if [ "$fail" -eq 0 ]; then echo "render checks passed"; else echo "render checks FAILED"; exit 1; fi

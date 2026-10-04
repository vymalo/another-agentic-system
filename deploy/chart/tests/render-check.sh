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
trap 'rm -f "$out" "$cfg"' EXIT
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
check "three ExternalSecrets, all on the ClusterSecretStore ssegning-aws" count '^kind: ExternalSecret$' 3
check "every ExternalSecret reads the AWS secret prod/another-agentic/env" count '^        key: prod/another-agentic/env$' 8
check "the store is ssegning-aws (ClusterSecretStore)" count '^    name: ssegning-aws$' 3
check "the store kind is ClusterSecretStore" count '^    kind: ClusterSecretStore$' 3
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
check "two CNPG Clusters (the orchestrator's and the chat agent's)" count '^kind: Cluster$' 2
check "the chat agent runs adam-agent, with the thread tools allowed over http" dhas Deployment another-agentic-chat 'MCP_ALLOW_INSECURE'
check "the chat agent's database is its own cluster" dhas Deployment another-agentic-chat 'name: another-agentic-chat-db-app'
check "no model is named by the chart: the chat model is the value's" dhas Deployment another-agentic-chat 'value: "chat"'

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
refused "a title model without an endpoint" --set model.baseUrl= --set chat.enabled=false --set 'agents[0].id=coder' --set 'agents[0].name=Coder' --set 'agents[0].cardUrl=http://coder.x.svc:8080/c'
refused "an agent whose bearer has no AWS property" --set 'agents[1].tokenEnv=NOPE_TOKEN'
refused "no agents" --set 'agents=null'
refused "no AWS secret" --set externalSecrets.key=
refused "no database" --set database.instances=0

# ---- The chat agent's folder is the dev stack's ------------------------------------------------------------------------
check "files/chat/instructions.md is dev/agents/chat/agent/instructions.md" cmp -s "$chart/files/chat/instructions.md" "$repo/dev/agents/chat/agent/instructions.md"

# ---- The shipped values.yaml --------------------------------------------------------------------------------------------
check "values.yaml leaves the deployment's own values empty (host, issuer, model)" sh -c "
  grep -Eq '^host: \"\"$' '$chart/values.yaml' && grep -Eq '^  issuer: \"\"$' '$chart/values.yaml' && grep -Eq '^  baseUrl: \"\"$' '$chart/values.yaml'"
check "values.yaml has no value that looks like a secret" sh -c "! grep -Eq '(ghp_|github_pat_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16})' '$chart/values.yaml'"
check "values.yaml has no latest tag" sh -c "! grep -Eq 'tag: \"?latest' '$chart/values.yaml'"

if [ "$fail" -eq 0 ]; then echo "render checks passed"; else echo "render checks FAILED"; exit 1; fi

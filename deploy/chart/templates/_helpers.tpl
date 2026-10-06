{{/* Names and labels. Every object is named <fullname>-<component>. */}}
{{- define "agentic.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "agentic.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/* <fullname>-<component>; call with (dict "root" $ "component" "web"). */}}
{{- define "agentic.component" -}}
{{- printf "%s-%s" (include "agentic.fullname" .root) .component | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* Selector labels of one component; the same dict. */}}
{{- define "agentic.selectorLabels" -}}
app.kubernetes.io/name: {{ include "agentic.name" .root }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "agentic.labels" -}}
{{ include "agentic.selectorLabels" . }}
app.kubernetes.io/version: {{ .root.Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .root.Release.Service }}
app.kubernetes.io/part-of: another-agentic-system
helm.sh/chart: {{ printf "%s-%s" .root.Chart.Name .root.Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{/* repository:tag, plus @digest when there is one. Call with the image map. */}}
{{- define "agentic.image" -}}
{{- $tag := required "an image needs a tag" .tag -}}
{{- if .digest -}}
{{- printf "%s:%s@%s" (required "an image needs a repository" .repository) $tag .digest -}}
{{- else -}}
{{- printf "%s:%s" (required "an image needs a repository" .repository) $tag -}}
{{- end -}}
{{- end -}}

{{/* The in-cluster host of a component's Service: <name>.<namespace>.svc */}}
{{- define "agentic.svcHost" -}}
{{- printf "%s.%s.svc" (include "agentic.component" .) .root.Release.Namespace -}}
{{- end -}}

{{/* The audiences of the tokens: auth.audiences, else the client id. */}}
{{- define "agentic.audiences" -}}
{{- if .Values.auth.audiences -}}{{- toYaml .Values.auth.audiences -}}{{- else -}}{{- toYaml (list .Values.auth.clientId) -}}{{- end -}}
{{- end -}}

{{/* Names of the Secrets the ExternalSecrets create. */}}
{{- define "agentic.secret.orchestrator" -}}{{- include "agentic.component" (dict "root" . "component" "orchestrator") -}}{{- end -}}
{{- define "agentic.secret.oauth2" -}}{{- include "agentic.component" (dict "root" . "component" "oauth2-proxy") -}}{{- end -}}
{{- define "agentic.secret.chat" -}}{{- include "agentic.component" (dict "root" . "component" "chat") -}}{{- end -}}
{{- define "agentic.secret.websearch" -}}{{- include "agentic.component" (dict "root" . "component" "websearch") -}}{{- end -}}
{{- define "agentic.secret.oauth2redis" -}}{{- include "agentic.component" (dict "root" . "component" "oauth2-redis") -}}{{- end -}}

{{/* "true" or nothing: whether oauth2-proxy keeps its sessions in the Redis of this release (`oauth2Proxy.sessionStore: redis`). */}}
{{- define "agentic.oauth2Redis" -}}{{- if eq (toString .Values.oauth2Proxy.sessionStore) "redis" -}}true{{- end -}}{{- end -}}

{{/* CNPG Clusters; CNPG makes the Secret <cluster>-app, whose key `uri` is the connection string. */}}
{{- define "agentic.db.orchestrator" -}}{{- include "agentic.component" (dict "root" . "component" "db") -}}{{- end -}}

{{/*
One cluster, the databases of the orchestrator (`orchestrator`, the cluster's bootstrap database), of the chat agent (`agent`) and of
each coder (`sharedDatabase.coders`, or with `sharedDatabase.coder.enabled` the one database `coder`: its run store, read by
adam-rs's chart from an existing Secret). Each of the last two has a role of its own (CNPG managed roles) whose Secret an
ExternalSecret fills: `username`, `password` and `uri`.
*/}}
{{- define "agentic.db.host" -}}{{- printf "%s-rw.%s.svc" (include "agentic.db.orchestrator" .) .Release.Namespace -}}{{- end -}}
{{- define "agentic.secret.db.agent" -}}{{- include "agentic.component" (dict "root" . "component" "db-agent") -}}{{- end -}}

{{/*
The coders' databases, as a YAML list of {name, secretName, passwordProperty}, in order; `[]` when there are none. It is the one
place that reads `sharedDatabase`, and it refuses what cannot work, so every template that includes it is checked.
  * `sharedDatabase.coders`: one entry per coder (the database and role `name`, the Secret `secretName`, default `<name>-db-uri`,
    the AWS property `passwordProperty`, required except for the name `coder`, whose default is `externalSecrets.properties.coderDbPassword`).
  * `sharedDatabase.coder.enabled` (the older key, kept so values written for it render as before) is the one-element list
    {coder, coder.secretName, externalSecrets.properties.coderDbPassword}. Both keys at once are refused.
A name is a Postgres identifier (the role and the database) and a DNS label (it is part of the name of the `Database` object), so it
is lower-case letters and digits, starting with a letter: no hyphen (not an identifier without quotes), no underscore (not a label).
*/}}
{{- define "agentic.db.coders" -}}
{{- $out := list -}}
{{- $list := .Values.sharedDatabase.coders | default list -}}
{{- if not (kindIs "slice" $list) -}}
{{- fail "sharedDatabase.coders must be a list of {name, secretName, passwordProperty}" -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.sharedDatabase.coder.enabled) -}}
{{- fail (printf "sharedDatabase.coder.enabled must be true or false, got %v" .Values.sharedDatabase.coder.enabled) -}}
{{- end -}}
{{- if .Values.sharedDatabase.coder.enabled -}}
{{- if and .Values.externalSecrets.enabled (not .Values.externalSecrets.properties.coderDbPassword) -}}
{{- fail "sharedDatabase.coder.enabled needs externalSecrets.properties.coderDbPassword: the property of the AWS secret that holds the password of the database role `coder` (coder_db_password)" -}}
{{- end -}}
{{- if $list -}}
{{- fail "sharedDatabase.coder.enabled and sharedDatabase.coders are both set: list the coders in sharedDatabase.coders (the entry named coder is what coder.enabled makes) and leave sharedDatabase.coder off" -}}
{{- end -}}
{{- $out = append $out (dict "name" "coder" "secretName" (required "sharedDatabase.coder.secretName is required" .Values.sharedDatabase.coder.secretName) "passwordProperty" (toString .Values.externalSecrets.properties.coderDbPassword)) -}}
{{- end -}}
{{- $names := dict -}}
{{- $secrets := dict -}}
{{- $props := dict -}}
{{- $fullname := include "agentic.fullname" . -}}
{{- $agentSecret := include "agentic.secret.db.agent" . -}}
{{- range $i, $c := $list -}}
{{- $at := printf "sharedDatabase.coders[%d]" $i -}}
{{- if not (kindIs "map" $c) -}}{{- fail (printf "%s must be a map with name, secretName and passwordProperty" $at) -}}{{- end -}}
{{- range $k, $_v := $c -}}
{{- if not (has $k (list "name" "secretName" "passwordProperty")) -}}
{{- fail (printf "%s: %q is not a key (name, secretName and passwordProperty are)" $at $k) -}}
{{- end -}}
{{- end -}}
{{- $name := toString (default "" $c.name) -}}
{{- if not $name -}}{{- fail (printf "%s.name is required: the database and its role" $at) -}}{{- end -}}
{{- $at = printf "sharedDatabase.coders[%d] (%s)" $i $name -}}
{{- if not (regexMatch "^[a-z][a-z0-9]*$" $name) -}}
{{- fail (printf "%s: the name must be a Postgres identifier and a DNS label: lower-case letters and digits, starting with a letter (no hyphen, no underscore)" $at) -}}
{{- end -}}
{{- if gt (len (printf "%s-db-%s" $fullname $name)) 63 -}}
{{- fail (printf "%s: the name is too long: the Database object %s-db-%s must be at most 63 characters" $at $fullname $name) -}}
{{- end -}}
{{- if has $name (list "agent" "postgres" "template0" "template1" (toString $.Values.database.name) (toString $.Values.database.owner)) -}}
{{- fail (printf "%s: the name is taken: `agent` is the chat agent's database and role, `%s` is the orchestrator's, `postgres` and the templates are PostgreSQL's" $at $.Values.database.name) -}}
{{- end -}}
{{- if hasKey $names $name -}}{{- fail (printf "%s: the name is listed twice" $at) -}}{{- end -}}
{{- $_ := set $names $name true -}}
{{- $secretName := toString (default (printf "%s-db-uri" $name) $c.secretName) -}}
{{- if not (regexMatch "^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$" $secretName) -}}
{{- fail (printf "%s.secretName must be a Kubernetes Secret name (lower-case letters, digits, - and .), got %q" $at $secretName) -}}
{{- end -}}
{{- if gt (len $secretName) 253 -}}{{- fail (printf "%s.secretName must be at most 253 characters" $at) -}}{{- end -}}
{{- if or (eq $secretName $agentSecret) (hasKey $secrets $secretName) -}}
{{- fail (printf "%s.secretName: %q is used by another database role (each role has a Secret of its own)" $at $secretName) -}}
{{- end -}}
{{- $_ := set $secrets $secretName true -}}
{{- $prop := toString (default "" $c.passwordProperty) -}}
{{- if and (not $prop) (eq $name "coder") -}}{{- $prop = toString $.Values.externalSecrets.properties.coderDbPassword -}}{{- end -}}
{{- /* The property is only read by the ExternalSecrets: with `externalSecrets.enabled: false` the Secrets are the deployment's own. */ -}}
{{- if and $.Values.externalSecrets.enabled (not $prop) -}}
{{- fail (printf "%s.passwordProperty is required: the property of the AWS secret that holds the password of the database role %s (for example %s_db_password)" $at $name $name) -}}
{{- end -}}
{{- if $prop -}}
{{- if or (hasKey $props $prop) (eq $prop (toString $.Values.externalSecrets.properties.agentDbPassword)) -}}
{{- fail (printf "%s.passwordProperty: %q is used by another database role (each role has a password of its own)" $at $prop) -}}
{{- end -}}
{{- $_ := set $props $prop true -}}
{{- end -}}
{{- $out = append $out (dict "name" $name "secretName" $secretName "passwordProperty" $prop) -}}
{{- end -}}
{{- toYaml $out -}}
{{- end -}}

{{/* "true" or nothing: whether the orchestrator has a model endpoint (its address a value, or a property of the AWS secret). */}}
{{- define "agentic.hasModel" -}}{{- if or .Values.model.baseUrl .Values.model.baseUrlFromSecret -}}true{{- end -}}{{- end -}}

{{/* "true" or nothing: whether the model's address is read from the AWS secret (`model.baseUrlFromSecret`). */}}
{{- define "agentic.modelBaseUrlFromSecret" -}}{{- if .Values.model.baseUrlFromSecret -}}true{{- end -}}{{- end -}}

{{/* The distinct environment variables the agents file names as `tokenEnv`, one per line, in order. */}}
{{- define "agentic.tokenEnvs" -}}
{{- $seen := dict -}}
{{- range .Values.agents -}}
{{- if and .tokenEnv (not (hasKey $seen .tokenEnv)) -}}
{{- $_ := set $seen .tokenEnv true -}}
{{ .tokenEnv }}
{{ end -}}
{{- end -}}
{{- end -}}

{{/* Pod placement, shared by every component. */}}
{{- define "agentic.placement" -}}
{{- with .Values.nodeSelector }}
nodeSelector:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .Values.affinity }}
affinity:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- with .Values.tolerations }}
tolerations:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- end -}}

{{/* "true" or nothing: whether the orchestrator is given the search pod / Context7 as a tool server (and so a key). */}}
{{/* The `mcp.json` of the chat's researcher sub-agent (ADR 0050): the search pod, over its Service, the bearer named by variable.
     Only with webSearch.enabled. The folder's own copy of this file (dev/agents/chat/agent/subagents/researcher/mcp.json) names the
     compose mock instead. */}}
{{- define "agentic.chat.researcherMcp" -}}
{{- $server := dict "type" "http" "url" (printf "http://%s:8080/mcp" (include "agentic.svcHost" (dict "root" . "component" "websearch"))) "headers" (dict "Authorization" "Bearer ${SEARCH_MCP_TOKEN}") "tools" (list "web_search" "fetch") -}}
{{- dict "mcpServers" (dict "search" $server) | toPrettyJson -}}
{{- end -}}

{{- define "agentic.toolServer.websearch" -}}{{- if .Values.orchestrator.toolServers.websearch.enabled -}}true{{- end -}}{{- end -}}
{{- define "agentic.toolServer.context7" -}}{{- if .Values.orchestrator.toolServers.context7.enabled -}}true{{- end -}}{{- end -}}

{{/*
The `toolServers` list of the orchestrator's configuration, as YAML; nothing at all when no server is enabled, so the
default render has no `toolServers` key (an orchestrator image from before the key refuses it). A key is a `{ file }`
reference to what the orchestrator's ExternalSecret mounts, never a value.
*/}}
{{- define "agentic.toolServers" -}}
{{- $list := list -}}
{{- $ws := .Values.orchestrator.toolServers.websearch -}}
{{- if $ws.enabled -}}
{{- $s := dict "id" "websearch" "name" $ws.name "url" (printf "http://%s:8080/mcp" (include "agentic.svcHost" (dict "root" . "component" "websearch"))) "bearer" (dict "file" "/run/secrets/orchestrator/search-mcp-token") -}}
{{- with $ws.description }}{{- $_ := set $s "description" . -}}{{- end -}}
{{- with $ws.icon }}{{- $_ := set $s "icon" . -}}{{- end -}}
{{- with $ws.tools }}{{- $_ := set $s "tools" . -}}{{- end -}}
{{- with $ws.agents }}{{- $_ := set $s "agents" . -}}{{- end -}}
{{- if hasKey $ws "timeoutSecs" }}{{- $_ := set $s "timeoutSecs" (int $ws.timeoutSecs) -}}{{- end -}}
{{- $list = append $list $s -}}
{{- end -}}
{{- $c7 := .Values.orchestrator.toolServers.context7 -}}
{{- if $c7.enabled -}}
{{- $s := dict "id" "context7" "name" $c7.name "url" (required "orchestrator.toolServers.context7.url is required" $c7.url) "bearer" (dict "file" "/run/secrets/orchestrator/context7-api-key") -}}
{{- with $c7.description }}{{- $_ := set $s "description" . -}}{{- end -}}
{{- with $c7.icon }}{{- $_ := set $s "icon" . -}}{{- end -}}
{{- with $c7.tools }}{{- $_ := set $s "tools" . -}}{{- end -}}
{{- with $c7.agents }}{{- $_ := set $s "agents" . -}}{{- end -}}
{{- if hasKey $c7 "timeoutSecs" }}{{- $_ := set $s "timeoutSecs" (int $c7.timeoutSecs) -}}{{- end -}}
{{- $list = append $list $s -}}
{{- end -}}
{{- if $list -}}{{- toYaml $list -}}{{- end -}}
{{- end -}}

{{/* "true" or nothing: whether sharing is on (`sharing.mode` other than disabled), and whether the public link is. */}}
{{- define "agentic.sharing" -}}{{- if ne (toString .Values.sharing.mode) "disabled" -}}true{{- end -}}{{- end -}}
{{- define "agentic.sharing.public" -}}{{- if eq (toString .Values.sharing.mode) "public" -}}true{{- end -}}{{- end -}}

{{/*
`auth.roles` of the orchestrator's configuration, as YAML. With sharing on, the roles of `sharing.roles` also hold `thread.share`;
with it off this is `auth.roles` as written.
*/}}
{{- define "agentic.roles" -}}
{{- $roles := deepCopy .Values.auth.roles -}}
{{- if include "agentic.sharing" . -}}
{{- range $name := .Values.sharing.roles -}}
{{- $role := get $roles (toString $name) -}}
{{- if not (has "thread.share" ($role.permissions | default list)) -}}
{{- $_ := set $role "permissions" (append ($role.permissions | default list) "thread.share") -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- toYaml $roles -}}
{{- end -}}

{{/*
redis.conf of the session Redis (`oauth2Proxy.sessionStore: redis`). The password is not here: it is written to a file in memory at
startup and read with `include`, so it is in no ConfigMap, no render and no command line. A session key always has a lifetime
(`cookieExpire`), so `volatile-lru` drops only sessions. No persistence unless `oauth2Proxy.redis.persistence.enabled`.
*/}}
{{- define "agentic.oauth2Redis.conf" -}}
bind * -::*
port 6379
protected-mode yes
include /run/redis-auth/auth.conf
dir /data
maxmemory {{ .Values.oauth2Proxy.redis.maxMemory }}
maxmemory-policy volatile-lru
save ""
{{- if .Values.oauth2Proxy.redis.persistence.enabled }}
appendonly yes
appendfsync everysec
{{- else }}
appendonly no
{{- end }}
{{- end -}}

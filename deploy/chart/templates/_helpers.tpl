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

{{/* CNPG Clusters; CNPG makes the Secret <cluster>-app, whose key `uri` is the connection string. */}}
{{- define "agentic.db.orchestrator" -}}{{- include "agentic.component" (dict "root" . "component" "db") -}}{{- end -}}

{{/*
One cluster, three databases: `orchestrator` (the cluster's bootstrap database), `agent` (the chat agent's run store) and, with
`sharedDatabase.coder.enabled`, `coder` (the coder's run store, read by adam-rs's chart from an existing Secret). Each of the last
two has a role of its own (CNPG managed roles) whose Secret an ExternalSecret fills: `username`, `password` and `uri`.
*/}}
{{- define "agentic.db.host" -}}{{- printf "%s-rw.%s.svc" (include "agentic.db.orchestrator" .) .Release.Namespace -}}{{- end -}}
{{- define "agentic.secret.db.agent" -}}{{- include "agentic.component" (dict "root" . "component" "db-agent") -}}{{- end -}}
{{- define "agentic.secret.db.coder" -}}{{- required "sharedDatabase.coder.secretName is required" .Values.sharedDatabase.coder.secretName -}}{{- end -}}

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

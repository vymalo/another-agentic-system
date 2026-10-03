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

{{/* CNPG Clusters; CNPG makes the Secret <cluster>-app, whose key `uri` is the connection string. */}}
{{- define "agentic.db.orchestrator" -}}{{- include "agentic.component" (dict "root" . "component" "db") -}}{{- end -}}
{{- define "agentic.db.chat" -}}{{- include "agentic.component" (dict "root" . "component" "chat-db") -}}{{- end -}}

{{/* "true" or nothing: whether the orchestrator has a model endpoint. */}}
{{- define "agentic.hasModel" -}}{{- if .Values.model.baseUrl -}}true{{- end -}}{{- end -}}

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

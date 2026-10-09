{{/*
The ExternalSecrets of the browser agent and of RustFS, one define each: externalsecrets.yaml renders them, and each workload's pods
carry a checksum of their own one only, so a change to another Secret's shape does not restart them.
*/}}
{{- define "agentic.es.rustfs" -}}
{{- $es := .Values.externalSecrets -}}
{{- $key := required "externalSecrets.key is required" $es.key -}}
{{- $rustfs := dict "root" . "component" "rustfs" -}}
apiVersion: {{ $es.apiVersion }}
kind: ExternalSecret
metadata:
  name: {{ include "agentic.secret.rustfs" . }}
  labels:
    {{- include "agentic.labels" $rustfs | nindent 4 }}
spec:
  refreshInterval: {{ $es.refreshInterval }}
  secretStoreRef:
    name: {{ $es.secretStoreRef.name }}
    kind: {{ $es.secretStoreRef.kind }}
  target:
    name: {{ include "agentic.secret.rustfs" . }}
    creationPolicy: Owner
  data:
    # RustFS's root credentials: the properties the orchestrator signs with, so the two sides cannot differ.
    - secretKey: RUSTFS_ACCESS_KEY
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.artifactsS3AccessKeyId is required with rustfs.enabled" $es.properties.artifactsS3AccessKeyId }}
    - secretKey: RUSTFS_SECRET_KEY
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.artifactsS3SecretAccessKey is required with rustfs.enabled" $es.properties.artifactsS3SecretAccessKey }}
{{- end -}}

{{- define "agentic.es.browser" -}}
{{- $es := .Values.externalSecrets -}}
{{- $key := required "externalSecrets.key is required" $es.key -}}
{{- $browser := dict "root" . "component" "browser" -}}
apiVersion: {{ $es.apiVersion }}
kind: ExternalSecret
metadata:
  name: {{ include "agentic.secret.browser" . }}
  labels:
    {{- include "agentic.labels" $browser | nindent 4 }}
spec:
  refreshInterval: {{ $es.refreshInterval }}
  secretStoreRef:
    name: {{ $es.secretStoreRef.name }}
    kind: {{ $es.secretStoreRef.kind }}
  target:
    name: {{ include "agentic.secret.browser" . }}
    creationPolicy: Owner
  data:
    # The same property the orchestrator reads as `{{ .Values.browser.tokenEnv }}`, so the two sides cannot differ.
    - secretKey: A2A_BEARER_TOKENS
      remoteRef:
        key: {{ $key }}
        property: {{ required (printf "externalSecrets.agentTokens.%s is required with browser.enabled" .Values.browser.tokenEnv) (get $es.agentTokens .Values.browser.tokenEnv) }}
    - secretKey: MODEL_API_KEY
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.modelApiKey is required with the browser agent" $es.properties.modelApiKey }}
    {{- if include "agentic.modelBaseUrlFromSecret" . }}
    - secretKey: MODEL_BASE_URL
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.modelBaseUrl is required with model.baseUrlFromSecret" $es.properties.modelBaseUrl }}
    {{- end }}
    # The bearer of the obscura sidecar: read by the sidecar and by the agent beside it, from this Secret only.
    - secretKey: OBSCURA_MCP_TOKEN
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.obscuraMcpToken is required with browser.enabled" $es.properties.obscuraMcpToken }}
{{- end -}}

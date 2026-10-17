{{/*
The ExternalSecrets of the browser agent, of RustFS and of the migration Job, one define each: externalsecrets.yaml renders them, and each
workload's pods carry a checksum of their own one only, so a change to another Secret's shape does not restart them.
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

{{/*
The migration Job's own Secret (`orchestrator.artifacts.migrate.enabled`, ADR 0058 A5.4): the two S3 credentials and nothing else, the same
two properties the orchestrator's Secret and RustFS read. It is the Job's own and not the orchestrator's so that, while the orchestrator still
serves from the directory (`store: fs`), turning the Job on neither adds a key to the orchestrator's Secret nor changes its checksum, which
would restart the pod that holds the volume.
*/}}
{{- define "agentic.es.artifactsMigrate" -}}
{{- $es := .Values.externalSecrets -}}
{{- $key := required "externalSecrets.key is required" $es.key -}}
{{- $job := dict "root" . "component" "artifacts-migrate" -}}
apiVersion: {{ $es.apiVersion }}
kind: ExternalSecret
metadata:
  name: {{ include "agentic.secret.artifactsMigrate" . }}
  labels:
    {{- include "agentic.labels" $job | nindent 4 }}
spec:
  refreshInterval: {{ $es.refreshInterval }}
  secretStoreRef:
    name: {{ $es.secretStoreRef.name }}
    kind: {{ $es.secretStoreRef.kind }}
  target:
    name: {{ include "agentic.secret.artifactsMigrate" . }}
    creationPolicy: Owner
  data:
    # `artifacts.s3.accessKeyId` and `secretAccessKey`, mounted as files: the same two properties as the orchestrator's, so the sides cannot differ.
    - secretKey: artifacts-s3-access-key-id
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.artifactsS3AccessKeyId is required with orchestrator.artifacts.migrate.enabled" $es.properties.artifactsS3AccessKeyId }}
    - secretKey: artifacts-s3-secret-access-key
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.artifactsS3SecretAccessKey is required with orchestrator.artifacts.migrate.enabled" $es.properties.artifactsS3SecretAccessKey }}
{{- end -}}

{{/*
The Secret of the database backups' S3 credentials (`backup.enabled`, issue #151): the two keys the ObjectStore names, filled from two
properties of the AWS secret that are the backup bucket's own (never the artifacts'). It is rendered by cnpg-backup.yaml and not by
externalsecrets.yaml, whose checksum the orchestrator's, the chat's, oauth2-proxy's and the search pod's pods carry: turning backups on
must not restart them. The plugin's sidecar reads the keys from the Secret at each archive, backup and retention call, so a rotated key
needs no restart (verified 2026-10-17, plugin v0.15.1); the label `cnpg.io/reload` is the plugin's page's advice for the Secrets an ObjectStore
names (it has a TODO about whether it applies to the plugin), kept as harmless.
*/}}
{{- define "agentic.es.dbBackup" -}}
{{- $es := .Values.externalSecrets -}}
{{- $key := required "externalSecrets.key is required" $es.key -}}
{{- $backup := dict "root" . "component" "db-backup" -}}
apiVersion: {{ $es.apiVersion }}
kind: ExternalSecret
metadata:
  name: {{ include "agentic.secret.dbBackup" . }}
  labels:
    {{- include "agentic.labels" $backup | nindent 4 }}
spec:
  refreshInterval: {{ $es.refreshInterval }}
  secretStoreRef:
    name: {{ $es.secretStoreRef.name }}
    kind: {{ $es.secretStoreRef.kind }}
  target:
    name: {{ include "agentic.secret.dbBackup" . }}
    creationPolicy: Owner
    template:
      metadata:
        labels:
          cnpg.io/reload: "true"
  data:
    # The keys the ObjectStore's `s3Credentials` name; the bucket's IAM user, which is not the artifacts' (README.md, "Backups").
    - secretKey: ACCESS_KEY_ID
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.backupS3AccessKeyId is required with backup.enabled" $es.properties.backupS3AccessKeyId }}
    - secretKey: ACCESS_SECRET_KEY
      remoteRef:
        key: {{ $key }}
        property: {{ required "externalSecrets.properties.backupS3SecretAccessKey is required with backup.enabled" $es.properties.backupS3SecretAccessKey }}
{{- end -}}

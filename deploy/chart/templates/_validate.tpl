{{/*
Value checks that must stop a render, not surface as a broken rollout or, worse, a quietly open one.
Included from orchestrator-configmap.yaml, which every render contains, so they always run.
*/}}
{{- define "agentic.validate" -}}
{{- /* The deployment's own values: no defaults, so a forgotten one is an error. */ -}}
{{- if not .Values.host -}}
{{- fail "host is required: the public hostname, no scheme (for example agentic.servers.segning.pro)" -}}
{{- end -}}
{{- if not (regexMatch "^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$" (toString .Values.host)) -}}
{{- fail (printf "host must be a lower-case DNS name with no scheme, port or path, got %q" (toString .Values.host)) -}}
{{- end -}}
{{- if not .Values.auth.issuer -}}
{{- fail "auth.issuer is required: the OpenID Connect issuer of the realm, for example https://auth.verif.fyi/realms/vymalo" -}}
{{- end -}}
{{- if not (hasPrefix "https://" (toString .Values.auth.issuer)) -}}
{{- fail "auth.issuer must be https: the orchestrator runs in production and refuses a plain http issuer (ADR 0033)" -}}
{{- end -}}
{{- if hasSuffix "/" (toString .Values.auth.issuer) -}}
{{- fail "auth.issuer must not end with a slash: it is compared with `iss` exactly" -}}
{{- end -}}
{{- if not .Values.auth.clientId -}}
{{- fail "auth.clientId is required: the Keycloak client of oauth2-proxy" -}}
{{- end -}}
{{- if not .Values.auth.rolesClaim -}}
{{- fail "auth.rolesClaim is required: without roles nobody has any permission" -}}
{{- end -}}
{{- /* Roles: nobody reads or acts on another person's thread (the in-review ADR 0039). */ -}}
{{- if not .Values.auth.roles -}}
{{- fail "auth.roles must name at least one role" -}}
{{- end -}}
{{- range $name, $role := .Values.auth.roles -}}
{{- $scope := $role.scope | default "own" -}}
{{- $any := false -}}
{{- if kindIs "string" $scope -}}
{{- if eq $scope "any" -}}{{- $any = true -}}{{- end -}}
{{- else -}}
{{- if or (eq (toString (get $scope "read")) "any") (eq (toString (get $scope "write")) "any") -}}{{- $any = true -}}{{- end -}}
{{- end -}}
{{- if $any -}}
{{- fail (printf "auth.roles.%s: a scope of `any` is refused: nobody reads or acts on another person's thread (ADR 0039); share it instead" $name) -}}
{{- end -}}
{{- end -}}
{{- /* Surfaces: the edge routes agui and the resource API; thread-tools is in-cluster. */ -}}
{{- range .Values.orchestrator.surfaces -}}
{{- if not (has . (list "agui" "thread-tools")) -}}
{{- fail (printf "orchestrator.surfaces: %q is not served by this chart: mcp and the webhooks need keys and routes of the edge it does not have yet (ADR 0041)" .) -}}
{{- end -}}
{{- end -}}
{{- if not (has "agui" .Values.orchestrator.surfaces) -}}
{{- fail "orchestrator.surfaces must include agui: it is what the web speaks" -}}
{{- end -}}
{{- /* The model. */ -}}
{{- if and .Values.model.baseUrl (not (regexMatch "^https?://[^/]" (toString .Values.model.baseUrl))) -}}
{{- fail "model.baseUrl must be an http(s) URL" -}}
{{- end -}}
{{- if and (or .Values.orchestrator.tasks.title.model .Values.orchestrator.tasks.description.model) (not .Values.model.baseUrl) -}}
{{- fail "orchestrator.tasks.title.model or description.model is set but model.baseUrl is empty: a task needs an endpoint" -}}
{{- end -}}
{{- if .Values.chat.enabled -}}
{{- if not .Values.model.baseUrl -}}
{{- fail "chat.enabled needs model.baseUrl: the chat agent talks to a model" -}}
{{- end -}}
{{- if not .Values.chat.model -}}
{{- fail "chat.enabled needs chat.model: the model's name at model.baseUrl" -}}
{{- end -}}
{{- end -}}
{{- /* The agents. */ -}}
{{- if not .Values.agents -}}
{{- fail "agents must list at least one agent" -}}
{{- end -}}
{{- $ids := dict -}}
{{- range $i, $a := .Values.agents -}}
{{- if not $a.id -}}{{- fail (printf "agents[%d].id is required" $i) -}}{{- end -}}
{{- if not $a.name -}}{{- fail (printf "agents[%d] (%s): name is required" $i $a.id) -}}{{- end -}}
{{- if hasKey $ids $a.id -}}{{- fail (printf "agents: the id %q is listed twice" $a.id) -}}{{- end -}}
{{- $_ := set $ids $a.id true -}}
{{- if not (tpl (toString ($a.cardUrl | default "")) $) -}}{{- fail (printf "agents[%d] (%s): cardUrl is required" $i $a.id) -}}{{- end -}}
{{- if $a.tokenEnv -}}
{{- if not (hasKey $.Values.externalSecrets.agentTokens $a.tokenEnv) -}}
{{- fail (printf "agents[%d] (%s): tokenEnv %s has no property in externalSecrets.agentTokens" $i $a.id $a.tokenEnv) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if and .Values.chat.enabled (not (hasKey $ids "chat")) -}}
{{- fail "chat.enabled is true but agents lists no agent with the id chat: the orchestrator would never call it" -}}
{{- end -}}
{{- if and .Values.chat.enabled (not (hasKey .Values.externalSecrets.agentTokens .Values.chat.tokenEnv)) -}}
{{- fail (printf "chat.tokenEnv %s has no property in externalSecrets.agentTokens" .Values.chat.tokenEnv) -}}
{{- end -}}
{{- /* Secrets. */ -}}
{{- if not .Values.externalSecrets.key -}}
{{- fail "externalSecrets.key is required: the AWS Secrets Manager secret that holds every value" -}}
{{- end -}}
{{- /* Images: first-party by an explicit commit tag, third-party by tag and digest. Never latest. */ -}}
{{- range $c := list "orchestrator" "web" -}}
{{- $img := (get $.Values $c).image -}}
{{- if not (regexMatch "^sha-[0-9a-f]{7}$" (toString $img.tag)) -}}
{{- fail (printf "%s.image.tag must be sha-<7 hex digits> (the commit that built it), got %q" $c (toString $img.tag)) -}}
{{- end -}}
{{- end -}}
{{- $pinned := list (dict "n" "oauth2Proxy" "i" .Values.oauth2Proxy.image) (dict "n" "edge" "i" .Values.edge.image) -}}
{{- if .Values.chat.enabled -}}{{- $pinned = append $pinned (dict "n" "chat" "i" .Values.chat.image) -}}{{- end -}}
{{- range $p := $pinned -}}
{{- if not (regexMatch "^sha256:[0-9a-f]{64}$" (toString $p.i.digest)) -}}
{{- fail (printf "%s.image.digest must be sha256:<64 hex digits>: a third-party image is pinned by tag and digest" $p.n) -}}
{{- end -}}
{{- if eq (toString $p.i.tag) "latest" -}}{{- fail (printf "%s.image.tag must not be latest" $p.n) -}}{{- end -}}
{{- end -}}
{{- /* The databases. */ -}}
{{- if lt (int .Values.database.instances) 1 -}}
{{- fail "database.instances must be at least 1" -}}
{{- end -}}
{{- if lt (int .Values.chat.database.instances) 1 -}}
{{- fail "chat.database.instances must be at least 1" -}}
{{- end -}}
{{- end -}}

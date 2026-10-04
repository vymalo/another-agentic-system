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
{{- /* The address from the AWS secret: a real boolean (`--set model.baseUrlFromSecret=false` is one; the string "false" would be on), never beside a written address, and with the name of its property. */ -}}
{{- if not (kindIs "bool" .Values.model.baseUrlFromSecret) -}}
{{- fail (printf "model.baseUrlFromSecret must be true or false, got %v" .Values.model.baseUrlFromSecret) -}}
{{- end -}}
{{- if and .Values.model.baseUrlFromSecret .Values.model.baseUrl -}}
{{- fail "model.baseUrl and model.baseUrlFromSecret are both set: the address is written in the values or kept in the AWS secret, not both" -}}
{{- end -}}
{{- /* The property is only read by the ExternalSecrets: with `externalSecrets.enabled: false` the Secrets are the deployment's own. */ -}}
{{- if and .Values.model.baseUrlFromSecret .Values.externalSecrets.enabled (not .Values.externalSecrets.properties.modelBaseUrl) -}}
{{- fail "model.baseUrlFromSecret needs externalSecrets.properties.modelBaseUrl: the property of the AWS secret that holds the address (model_base_url)" -}}
{{- end -}}
{{- if and (or .Values.orchestrator.tasks.title.model .Values.orchestrator.tasks.description.model) (not (include "agentic.hasModel" .)) -}}
{{- fail "orchestrator.tasks.title.model or description.model is set but model.baseUrl is empty: a task needs an endpoint" -}}
{{- end -}}
{{- if .Values.chat.enabled -}}
{{- if not (include "agentic.hasModel" .) -}}
{{- fail "chat.enabled needs model.baseUrl (or model.baseUrlFromSecret): the chat agent talks to a model" -}}
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
{{- $firstParty := list "orchestrator" "web" -}}
{{- if .Values.webSearch.enabled -}}{{- $firstParty = append $firstParty "webSearch" -}}{{- end -}}
{{- range $c := $firstParty -}}
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
{{- /* Web search and the tool servers. */ -}}
{{- if and .Values.webSearch.enabled (eq (toString .Values.webSearch.image.tag) "sha-0000000") -}}
{{- fail "webSearch.enabled needs webSearch.image.tag to name a built image: sha-0000000 is the placeholder, and the first build of .github/workflows/searxng-mcp.yml on main replaces it" -}}
{{- end -}}
{{- if and .Values.orchestrator.toolServers.websearch.enabled (not .Values.webSearch.enabled) -}}
{{- fail "orchestrator.toolServers.websearch.enabled points at the search pod, which webSearch.enabled=false does not deploy" -}}
{{- end -}}
{{- /* What the orchestrator image refuses at startup (exit 78, a full outage with `Recreate`) fails the render instead:
       the rules of `toolServers` in docs/api/config.md, orch-config's rules.rs and types.rs `ToolServer`. */ -}}
{{- range $id := list "websearch" "context7" -}}
{{- $t := get $.Values.orchestrator.toolServers $id -}}
{{- if $t.enabled -}}
{{- $at := printf "orchestrator.toolServers.%s" $id -}}
{{- $name := toString (default "" $t.name) -}}
{{- if or (regexMatch "^\\s*$" $name) (gt (len (splitList "" $name)) 80) (regexMatch "[[:cntrl:]]" $name) -}}
{{- fail (printf "%s.name must be 1 to 80 characters of one line, not blank (the picker shows it)" $at) -}}
{{- end -}}
{{- $desc := toString (default "" $t.description) -}}
{{- if or (gt (len (splitList "" $desc)) 500) (regexMatch "[[:cntrl:]]" $desc) -}}
{{- fail (printf "%s.description must be at most 500 characters of one line" $at) -}}
{{- end -}}
{{- /* timeoutSecs: 1 to 600, a whole number; 0 is refused, not dropped (hasKey, not `with`). */ -}}
{{- if hasKey $t "timeoutSecs" -}}
{{- $secs := $t.timeoutSecs -}}
{{- /* A values file gives a float64, `--set` an int64. */ -}}
{{- if not (or (kindIs "float64" $secs) (kindIs "int64" $secs) (kindIs "int" $secs)) -}}
{{- fail (printf "%s.timeoutSecs must be a whole number of seconds from 1 to 600, got %v" $at $secs) -}}
{{- end -}}
{{- $secs = float64 $secs -}}
{{- if or (ne (floor $secs) $secs) (lt $secs 1.0) (gt $secs 600.0) -}}
{{- fail (printf "%s.timeoutSecs must be a whole number of seconds from 1 to 600 (the orchestrator refuses anything else at startup), got %v" $at $secs) -}}
{{- end -}}
{{- end -}}
{{- /* agents: ids of agents this chart lists. */ -}}
{{- if $t.agents -}}
{{- if not (kindIs "slice" $t.agents) -}}{{- fail (printf "%s.agents must be a list of agent ids" $at) -}}{{- end -}}
{{- $seenAgents := dict -}}
{{- range $a := $t.agents -}}
{{- if not (hasKey $ids (toString $a)) -}}
{{- fail (printf "%s.agents: %q is not the id of an agent in `agents` (the orchestrator refuses it at startup)" $at (toString $a)) -}}
{{- end -}}
{{- if hasKey $seenAgents (toString $a) -}}{{- fail (printf "%s.agents: %q is listed twice" $at (toString $a)) -}}{{- end -}}
{{- $_ := set $seenAgents (toString $a) true -}}
{{- end -}}
{{- end -}}
{{- /* tools: the server's own tool names, `<id>__<tool>` at most 64 characters. */ -}}
{{- if $t.tools -}}
{{- if not (kindIs "slice" $t.tools) -}}{{- fail (printf "%s.tools must be a list of tool names" $at) -}}{{- end -}}
{{- $seenTools := dict -}}
{{- range $tool := $t.tools -}}
{{- if or (not (kindIs "string" $tool)) (not (regexMatch "^[A-Za-z0-9-][A-Za-z0-9_-]*$" $tool)) (gt (add (len $id) 2 (len $tool)) 64) -}}
{{- fail (printf "%s.tools: %v is not a tool name the relay can expose (A-Z a-z 0-9 _ -, not starting with _, and %s__<tool> at most 64 characters)" $at $tool $id) -}}
{{- end -}}
{{- if hasKey $seenTools $tool -}}{{- fail (printf "%s.tools: %q is listed twice" $at $tool) -}}{{- end -}}
{{- $_ := set $seenTools $tool true -}}
{{- end -}}
{{- end -}}
{{- /* icon: a small data URI. */ -}}
{{- with $t.icon -}}
{{- if or (not (regexMatch "^data:image/(svg\\+xml|png|webp);base64,[A-Za-z0-9+/=]+$" .)) (gt (len .) 8192) (ne (mod (len (regexReplaceAll "^data:image/[a-z+]+;base64," . "")) 4) 0) -}}
{{- fail (printf "%s.icon must be data:image/(svg+xml|png|webp);base64,... of at most 8 KiB (an icon at a URL is never fetched)" $at) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if .Values.orchestrator.toolServers.context7.enabled -}}
{{- if not (regexMatch "^https://[^/@?#[:space:]]+(/[^?#[:space:]]*)?$" (toString .Values.orchestrator.toolServers.context7.url)) -}}
{{- fail "orchestrator.toolServers.context7.url must be an https URL with a host and no user name, password, query or fragment: the key goes in the bearer header, never in the URL" -}}
{{- end -}}
{{- end -}}
{{- if or .Values.orchestrator.toolServers.websearch.enabled .Values.orchestrator.toolServers.context7.enabled -}}
{{- if not (has "thread-tools" .Values.orchestrator.surfaces) -}}
{{- fail "orchestrator.toolServers needs the thread-tools surface in orchestrator.surfaces: the relay is one of its providers (ADR 0024)" -}}
{{- end -}}
{{- end -}}
{{- /* The databases. */ -}}
{{- if lt (int .Values.database.instances) 1 -}}
{{- fail "database.instances must be at least 1" -}}
{{- end -}}
{{- if lt (int .Values.chat.database.instances) 1 -}}
{{- fail "chat.database.instances must be at least 1" -}}
{{- end -}}
{{- end -}}

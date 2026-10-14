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
{{- /* Android App Links and iOS Universal Links (ADR 0047). These files bind a public hostname to the production app, so a
missing identity must fail the render rather than silently serve a link another app could claim. */ -}}
{{- if not (kindIs "bool" .Values.mobileAppLinks.enabled) -}}
{{- fail (printf "mobileAppLinks.enabled must be true or false, got %v" .Values.mobileAppLinks.enabled) -}}
{{- end -}}
{{- if .Values.mobileAppLinks.enabled -}}
{{- if not (kindIs "slice" .Values.mobileAppLinks.android.sha256CertFingerprints) -}}
{{- fail "mobileAppLinks.android.sha256CertFingerprints must be a non-empty list of SHA-256 certificate fingerprints" -}}
{{- end -}}
{{- if not .Values.mobileAppLinks.android.sha256CertFingerprints -}}
{{- fail "mobileAppLinks.enabled needs at least one Android release certificate fingerprint" -}}
{{- end -}}
{{- range .Values.mobileAppLinks.android.sha256CertFingerprints -}}
{{- if or (not (kindIs "string" .)) (not (regexMatch "^[A-F0-9]{2}(:[A-F0-9]{2}){31}$" .)) -}}
{{- fail (printf "mobileAppLinks.android.sha256CertFingerprints: %v must be 32 upper-case hexadecimal bytes separated by colons" .) -}}
{{- end -}}
{{- end -}}
{{- if or (not (kindIs "string" .Values.mobileAppLinks.ios.appId)) (not (regexMatch "^[A-Z0-9]{10}\\.com\\.vymalo\\.agentic$" .Values.mobileAppLinks.ios.appId)) -}}
{{- fail "mobileAppLinks.ios.appId must be the ten-character Apple Team ID followed by .com.vymalo.agentic" -}}
{{- end -}}
{{- if not .Values.auth.browser.enabled -}}
{{- fail "mobileAppLinks.enabled needs auth.browser.enabled: mobile apps send DPoP-bound tokens" -}}
{{- end -}}
{{- if not (has "http://tauri.localhost" .Values.orchestrator.cors.allowedOrigins) -}}
{{- fail "mobileAppLinks.enabled needs http://tauri.localhost in orchestrator.cors.allowedOrigins for Android" -}}
{{- end -}}
{{- if not (has "tauri://localhost" .Values.orchestrator.cors.allowedOrigins) -}}
{{- fail "mobileAppLinks.enabled needs tauri://localhost in orchestrator.cors.allowedOrigins for iOS" -}}
{{- end -}}
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
{{- /* Calls from the apps (ADR 0047): exact origins, never a wildcard, and only with tokens in the browser (the apps send DPoP). */ -}}
{{- if not (kindIs "slice" .Values.orchestrator.cors.allowedOrigins) -}}
{{- fail "orchestrator.cors.allowedOrigins must be a list of origins" -}}
{{- end -}}
{{- range .Values.orchestrator.cors.allowedOrigins -}}
{{- if or (not (kindIs "string" .)) (not (regexMatch "^[a-z][a-z0-9+.-]*://[a-z0-9.-]+(:[0-9]{1,5})?$" .)) -}}
{{- fail (printf "orchestrator.cors.allowedOrigins: %v is not one exact origin (scheme://host[:port], lower case, no path, no trailing slash, never *)" .) -}}
{{- end -}}
{{- /* What the orchestrator refuses in production (exit 78): http:// off this machine, and a default port a browser never sends. */ -}}
{{- if and (hasPrefix "http://" .) (not (regexMatch "^http://(localhost|127\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}|[a-z0-9.-]+\\.localhost)(:[0-9]{1,5})?$" .)) -}}
{{- fail (printf "orchestrator.cors.allowedOrigins: %v is http:// off this machine; the orchestrator runs in production and takes http:// only for localhost, a loopback address or a name under .localhost (http://tauri.localhost)" .) -}}
{{- end -}}
{{- if or (regexMatch "^http://[^/]+:80$" .) (regexMatch "^https://[^/]+:443$" .) -}}
{{- fail (printf "orchestrator.cors.allowedOrigins: %v names its scheme's default port, which a browser leaves out of Origin: write it without the port" .) -}}
{{- end -}}
{{- end -}}
{{- if and .Values.orchestrator.cors.allowedOrigins (not .Values.auth.browser.enabled) -}}
{{- fail "orchestrator.cors.allowedOrigins needs auth.browser.enabled: the apps call the API with DPoP-bound tokens, which the orchestrator verifies only in browser mode" -}}
{{- end -}}
{{- /* Tokens in the browser (ADR 0054): a real boolean (the string false would be on), and what the web needs to sign in. */ -}}
{{- if not (kindIs "bool" .Values.auth.browser.enabled) -}}
{{- fail (printf "auth.browser.enabled must be true or false, got %v" .Values.auth.browser.enabled) -}}
{{- end -}}
{{- if .Values.auth.browser.enabled -}}
{{- if not .Values.auth.issuer -}}
{{- fail "auth.browser.enabled needs auth.issuer: the web signs in at it and its content security policy names its origin" -}}
{{- end -}}
{{- /* The issuer's origin goes into the web's policy header as it is (WEB_CSP_CONNECT_SRC, web/Caddyfile): one origin, nothing else. */ -}}
{{- if not (regexMatch "^https://[a-z0-9.-]+(:[0-9]{1,5})?$" (include "agentic.issuerOrigin" .)) -}}
{{- fail (printf "auth.issuer's origin %q goes into the web's content security policy (WEB_CSP_CONNECT_SRC): it must be https://host[:port], lower case, with no ;, quote or space" (include "agentic.issuerOrigin" .)) -}}
{{- end -}}
{{- if not .Values.auth.browser.clientId -}}
{{- fail "auth.browser.enabled needs auth.browser.clientId: the public Keycloak client of the web (another-agentic-web)" -}}
{{- end -}}
{{- if or (not (kindIs "string" .Values.auth.browser.scope)) (not (regexMatch "^[A-Za-z0-9_.:/-]+( [A-Za-z0-9_.:/-]+)*$" .Values.auth.browser.scope)) (not (has "openid" (splitList " " .Values.auth.browser.scope))) -}}
{{- fail "auth.browser.scope must be space-separated scope names that include openid, for example: openid email profile offline_access" -}}
{{- end -}}
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
{{- /* Sharing: a closed set of modes, the roles it names exist, and a key that would do nothing is refused (as the orchestrator does). */ -}}
{{- if not (has (toString .Values.sharing.mode) (list "disabled" "internal" "public")) -}}
{{- fail (printf "sharing.mode must be disabled, internal or public, got %q" (toString .Values.sharing.mode)) -}}
{{- end -}}
{{- if include "agentic.sharing" . -}}
{{- if not .Values.sharing.roles -}}
{{- fail "sharing.roles must name at least one role of auth.roles: with none, nobody holds thread.share and nobody could make a link" -}}
{{- end -}}
{{- range $name := .Values.sharing.roles -}}
{{- if not (hasKey $.Values.auth.roles (toString $name)) -}}
{{- fail (printf "sharing.roles: %q is not a role of auth.roles" (toString $name)) -}}
{{- end -}}
{{- end -}}
{{- if and .Values.externalSecrets.enabled (not .Values.externalSecrets.properties.sharingSecret) -}}
{{- fail "sharing.mode needs externalSecrets.properties.sharingSecret: the property of the AWS secret that holds the HMAC key of the share links (sharing_secret)" -}}
{{- end -}}
{{- end -}}
{{- if and (not (include "agentic.sharing.public" .)) (or .Values.sharing.public.stepIo .Values.sharing.public.files) -}}
{{- fail "sharing.public.stepIo and sharing.public.files only apply with sharing.mode public (the orchestrator refuses a key that does nothing)" -}}
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
{{- /* The history read (ADR 0059): a switch, and counts the orchestrator would refuse at startup (exit 78) if they were out of range. */ -}}
{{- $history := .Values.orchestrator.history -}}
{{- if not (kindIs "bool" $history.windowed) -}}
{{- fail (printf "orchestrator.history.windowed must be true or false, got %v" $history.windowed) -}}
{{- end -}}
{{- range $key, $range := dict "initialTurns" (list 1 100) "pageTurns" (list 1 100) "maxTurns" (list 1 1000) "maxPageBytes" (list 65536 67108864) -}}
{{- /* `index`, not `get`: a key Helm dropped (a null in the values, which some Helm versions drop from the defaults too) is nil, where `get` says "" */ -}}
{{- $value := index $history $key -}}
{{- if not (kindIs "invalid" $value) -}}
{{- if or (not (or (kindIs "float64" $value) (kindIs "int64" $value) (kindIs "int" $value))) (lt (float64 $value) (float64 (index $range 0))) (gt (float64 $value) (float64 (index $range 1))) (ne (float64 (int64 $value)) (float64 $value)) -}}
{{- fail (printf "orchestrator.history.%s is an integer from %v to %v, or null (the orchestrator's default)" $key (index $range 0) (index $range 1)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- $largest := 100.0 -}}
{{- if not (kindIs "invalid" $history.maxTurns) -}}
{{- $largest = float64 $history.maxTurns -}}
{{- end -}}
{{- range $key := list "initialTurns" "pageTurns" -}}
{{- $value := index $history $key -}}
{{- if and (not (kindIs "invalid" $value)) (gt (float64 $value) $largest) -}}
{{- fail (printf "orchestrator.history.%s is above orchestrator.history.maxTurns (default 100): the server would refuse the page the web asks for" $key) -}}
{{- end -}}
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
{{- $window := .Values.chat.contextWindow -}}
{{- if not (kindIs "invalid" $window) -}}
{{- if not (or (kindIs "float64" $window) (kindIs "int64" $window) (kindIs "int" $window)) -}}
{{- fail "chat.contextWindow is a number of tokens (an integer from 1 to 9007199254740991), or null" -}}
{{- end -}}
{{- if or (lt (float64 $window) 1.0) (gt (float64 $window) 9007199254740991.0) (ne (float64 (int64 $window)) (float64 $window)) -}}
{{- fail "chat.contextWindow is a number of tokens (an integer from 1 to 9007199254740991), or null" -}}
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
{{- /* Aliases (ADR 0049): other names of an agent. Not an id, not another agent's alias, as the orchestrator refuses at startup. */ -}}
{{- $names := deepCopy $ids -}}
{{- range $i, $a := .Values.agents -}}
{{- range $alias := ($a.aliases | default list) -}}
{{- if eq (toString $alias) $a.id -}}{{- fail (printf "agents[%d] (%s): the alias %q is the agent's own id" $i $a.id (toString $alias)) -}}{{- end -}}
{{- if hasKey $names (toString $alias) -}}{{- fail (printf "agents[%d] (%s): the alias %q is already an id or an alias of another agent" $i $a.id (toString $alias)) -}}{{- end -}}
{{- $_ := set $names (toString $alias) true -}}
{{- end -}}
{{- end -}}
{{- if and .Values.chat.enabled (not (hasKey $ids "chat")) -}}
{{- fail "chat.enabled is true but agents lists no agent with the id chat: the orchestrator would never call it" -}}
{{- end -}}
{{- if and .Values.chat.enabled (not (hasKey .Values.externalSecrets.agentTokens .Values.chat.tokenEnv)) -}}
{{- fail (printf "chat.tokenEnv %s has no property in externalSecrets.agentTokens" .Values.chat.tokenEnv) -}}
{{- end -}}
{{- /* The browser agent (ADR 0057). */ -}}
{{- if not (kindIs "bool" .Values.browser.enabled) -}}
{{- fail (printf "browser.enabled must be true or false, got %v" .Values.browser.enabled) -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.browser.chatSubagent) -}}
{{- fail (printf "browser.chatSubagent must be true or false, got %v" .Values.browser.chatSubagent) -}}
{{- end -}}
{{- /* The chat's remote sub-agent (ADR 0057, amended 2026-10-09): the browser's Service, the chat that calls it and the bearer the call
       carries, or nothing. Without one of them the chat would call no Service, nobody would call, or the chat would stop at startup
       (adam-agent refuses an `auth: bearer:VAR` whose variable is empty, exit 78). */ -}}
{{- if .Values.browser.chatSubagent -}}
{{- if not .Values.browser.enabled -}}
{{- fail "browser.chatSubagent needs browser.enabled: the chat's sub-agent `browser` calls the browser agent's Service" -}}
{{- end -}}
{{- if not .Values.chat.enabled -}}
{{- fail "browser.chatSubagent needs chat.enabled: it is a sub-agent of the chat agent" -}}
{{- end -}}
{{- if not (get .Values.externalSecrets.agentTokens (toString .Values.browser.tokenEnv)) -}}
{{- fail (printf "browser.chatSubagent needs the browser's bearer: externalSecrets.agentTokens.%s names the AWS property the chat sends it from" (toString .Values.browser.tokenEnv)) -}}
{{- end -}}
{{- end -}}
{{- if .Values.browser.enabled -}}
{{- if not (include "agentic.hasModel" .) -}}
{{- fail "browser.enabled needs model.baseUrl (or model.baseUrlFromSecret): the browser agent talks to a model" -}}
{{- end -}}
{{- if not (.Values.browser.model | default .Values.chat.model) -}}
{{- fail "browser.enabled needs browser.model (or chat.model): the model's name at model.baseUrl" -}}
{{- end -}}
{{- if ne (toString .Values.browser.replicas) "1" -}}
{{- fail (printf "browser.replicas must be 1, got %v: one browser per task, and adam-agent at the pinned revision moves a run between workers at every step, so a second replica would open another pod's browser in the middle of a task (ADR 0057)" .Values.browser.replicas) -}}
{{- end -}}
{{- if not (hasKey .Values.externalSecrets.agentTokens .Values.browser.tokenEnv) -}}
{{- fail (printf "browser.tokenEnv %s has no property in externalSecrets.agentTokens" .Values.browser.tokenEnv) -}}
{{- end -}}
{{- $p := .Values.browser.obscura.port -}}
{{- if or (not (or (kindIs "float64" $p) (kindIs "int64" $p) (kindIs "int" $p))) (lt (float64 $p) 1.0) (gt (float64 $p) 65535.0) (eq (int $p) 8080) -}}
{{- fail "browser.obscura.port is the loopback port of the browser's MCP server: a number from 1 to 65535, not 8080 (the agent's)" -}}
{{- end -}}
{{- $window := .Values.browser.contextWindow -}}
{{- if not (kindIs "invalid" $window) -}}
{{- if or (not (or (kindIs "float64" $window) (kindIs "int64" $window) (kindIs "int" $window))) (lt (float64 $window) 1.0) (gt (float64 $window) 9007199254740991.0) (ne (float64 (int64 $window)) (float64 $window)) -}}
{{- fail "browser.contextWindow is a number of tokens (an integer from 1 to 9007199254740991), or null" -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- /* The artifact store (ADR 0032): a directory on a volume, or an S3 bucket (AWS, another server, or the RustFS of this release). */ -}}
{{- if not (has (toString .Values.orchestrator.artifacts.store) (list "fs" "s3")) -}}
{{- fail (printf "orchestrator.artifacts.store must be fs or s3, got %q" (toString .Values.orchestrator.artifacts.store)) -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.rustfs.enabled) -}}
{{- fail (printf "rustfs.enabled must be true or false, got %v" .Values.rustfs.enabled) -}}
{{- end -}}
{{- if and .Values.rustfs.enabled (not (include "agentic.artifactsS3" .)) -}}
{{- fail "rustfs.enabled is the store of orchestrator.artifacts.store s3: with fs nothing would use it" -}}
{{- end -}}
{{- if include "agentic.artifactsS3" . -}}
{{- $s3 := .Values.orchestrator.artifacts.s3 -}}
{{- $bucket := include "agentic.artifactsS3.bucket" . -}}
{{- if not (regexMatch "^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$" $bucket) -}}
{{- fail (printf "orchestrator.artifacts.s3.bucket must be a bucket name (3 to 63 of a-z, 0-9, - and ., a letter or a digit at both ends), got %q; with rustfs.enabled it may be empty (rustfs.bucket)" $bucket) -}}
{{- end -}}
{{- /* Without an endpoint the store addresses AWS S3 in virtual-hosted style (`<bucket>.s3.<region>.amazonaws.com`), where a dot in the
       name breaks the certificate's match; with an endpoint (another server, or RustFS) it is path style. */ -}}
{{- if and (contains "." $bucket) (not (include "agentic.artifactsS3.endpoint" .)) -}}
{{- fail (printf "orchestrator.artifacts.s3.bucket %q has a dot: without an endpoint the store reaches AWS S3 in virtual-hosted style, where a dotted name breaks TLS; name the bucket without dots, or set orchestrator.artifacts.s3.endpoint" $bucket) -}}
{{- end -}}
{{- if and .Values.rustfs.enabled $s3.endpoint -}}
{{- fail "orchestrator.artifacts.s3.endpoint must be empty with rustfs.enabled: the endpoint is the RustFS Service of this release" -}}
{{- end -}}
{{- if and $s3.endpoint (not (regexMatch "^https?://[^/@?#]+(/[^@?#]*)?$" (toString $s3.endpoint))) -}}
{{- fail "orchestrator.artifacts.s3.endpoint must be an http(s) URL with a host and no credentials, query or fragment" -}}
{{- end -}}
{{- if and $s3.prefix (or (not (regexMatch "^[A-Za-z0-9._/-]{1,128}$" (toString $s3.prefix))) (contains ".." (toString $s3.prefix))) -}}
{{- fail "orchestrator.artifacts.s3.prefix is at most 128 of A-Z, a-z, 0-9, ., _, - and /, with no .." -}}
{{- end -}}
{{- if not (regexMatch "^[a-z0-9-]+$" (toString $s3.region)) -}}
{{- fail "orchestrator.artifacts.s3.region must be a region name (us-east-1)" -}}
{{- end -}}
{{- $t := $s3.timeoutSecs -}}
{{- if or (not (or (kindIs "float64" $t) (kindIs "int64" $t) (kindIs "int" $t))) (lt (float64 $t) 1.0) (gt (float64 $t) 3600.0) -}}
{{- fail "orchestrator.artifacts.s3.timeoutSecs is a number of seconds from 1 to 3600" -}}
{{- end -}}
{{- end -}}
{{- /* Large files and the public files host (ADR 0058). What the orchestrator refuses at startup (exit 78: orch-config's rules.rs, `uploads`
       and `downloads`) is refused here instead, with the key. A setting of something that is off (an `upload` that is not `enabled`) is not read. */ -}}
{{- $art := .Values.orchestrator.artifacts -}}
{{- $s3on := include "agentic.artifactsS3" . -}}
{{- if not (kindIs "bool" .Values.rustfs.ingress.enabled) -}}
{{- fail (printf "rustfs.ingress.enabled must be true or false, got %v" .Values.rustfs.ingress.enabled) -}}
{{- end -}}
{{- if not (kindIs "bool" $art.upload.enabled) -}}
{{- fail (printf "orchestrator.artifacts.upload.enabled must be true or false, got %v" $art.upload.enabled) -}}
{{- end -}}
{{- if .Values.rustfs.ingress.enabled -}}
{{- $fh := .Values.rustfs.ingress -}}
{{- if not .Values.rustfs.enabled -}}
{{- fail "rustfs.ingress.enabled puts the public files host in front of the RustFS of this release: it needs rustfs.enabled" -}}
{{- end -}}
{{- if not $fh.host -}}
{{- fail "rustfs.ingress.host is required with rustfs.ingress.enabled: the public files host, a DNS name with no scheme (the deployment's own value, for example agentic-files.servers.segning.pro)" -}}
{{- end -}}
{{- if not (regexMatch "^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$" (toString $fh.host)) -}}
{{- fail (printf "rustfs.ingress.host must be a lower-case DNS name with no scheme, port or path, got %q" (toString $fh.host)) -}}
{{- end -}}
{{- if gt (len (toString $fh.host)) 253 -}}
{{- fail (printf "rustfs.ingress.host is %d characters, at most 253 (a DNS name)" (len (toString $fh.host))) -}}
{{- end -}}
{{- range (splitList "." (toString $fh.host)) -}}
{{- if gt (len .) 63 -}}
{{- fail (printf "rustfs.ingress.host: the label %q is %d characters, at most 63 (a DNS label)" . (len .)) -}}
{{- end -}}
{{- end -}}
{{- if eq (toString $fh.host) (toString .Values.host) -}}
{{- fail "rustfs.ingress.host must not be the app's host: the files host routes the bucket and nothing else, the app's host the app" -}}
{{- end -}}
{{- if and .Values.networkPolicy.enabled (not .Values.networkPolicy.ingressControllerNamespace) -}}
{{- fail "rustfs.ingress.enabled with networkPolicy.enabled needs networkPolicy.ingressControllerNamespace, the namespace of the ingress controller (Traefik): without it the proxy admits any source on its port, and the proxy trusts the forwarding headers of the private ranges, so any pod could send them. (networkPolicy.enabled=false is the other way, and is the cluster's to guarantee.)" -}}
{{- end -}}
{{- if not (or $fh.clusterIssuer .Values.ingress.clusterIssuer) -}}
{{- fail "rustfs.ingress.enabled needs a cert-manager ClusterIssuer: rustfs.ingress.clusterIssuer, or ingress.clusterIssuer" -}}
{{- end -}}
{{- include "agentic.checkOrigins" (dict "name" "rustfs.ingress.corsOrigins" "origins" $fh.corsOrigins) -}}
{{- include "agentic.checkWhole" (dict "name" "rustfs.ingress.proxy.replicas" "value" $fh.proxy.replicas "min" 1 "max" 10) -}}
{{- end -}}
{{- /* The public endpoint: the host of every presigned URL. */ -}}
{{- $public := include "agentic.artifactsS3.publicEndpoint" . -}}
{{- if not (kindIs "string" $art.s3.publicEndpoint) -}}
{{- fail "orchestrator.artifacts.s3.publicEndpoint must be a string: an https URL, or empty" -}}
{{- end -}}
{{- if $art.s3.publicEndpoint -}}
{{- if not $s3on -}}
{{- fail "orchestrator.artifacts.s3.publicEndpoint needs orchestrator.artifacts.store: s3: the directory store presigns nothing" -}}
{{- end -}}
{{- if hasPrefix "http://" (lower $public) -}}
{{- fail "orchestrator.artifacts.s3.publicEndpoint must be https: the orchestrator runs in production, and a presigned URL is a credential for its minutes and the files travel on it (an http one is exit 78)" -}}
{{- end -}}
{{- if not (regexMatch "^https://[^/@?#[:space:]]+(/[^@?#[:space:]]*)?$" $public) -}}
{{- fail "orchestrator.artifacts.s3.publicEndpoint must be an https URL with a host and no credentials, query or fragment" -}}
{{- end -}}
{{- if and .Values.rustfs.ingress.enabled (ne $public (printf "https://%s" (include "agentic.filesHost.name" .))) -}}
{{- fail (printf "orchestrator.artifacts.s3.publicEndpoint is %q, and the files host is https://%s: leave it empty (it is then the files host), or name exactly that URL; the proxy routes only the bucket's path, and SigV4 signs the host" $public (include "agentic.filesHost.name" .)) -}}
{{- end -}}
{{- end -}}
{{- /* Downloads. */ -}}
{{- if not (has (toString $art.s3.downloads) (list "stream" "redirect")) -}}
{{- fail (printf "orchestrator.artifacts.s3.downloads must be stream or redirect, got %q" (toString $art.s3.downloads)) -}}
{{- end -}}
{{- if eq (toString $art.s3.downloads) "redirect" -}}
{{- if not $s3on -}}
{{- fail "orchestrator.artifacts.s3.downloads: redirect needs orchestrator.artifacts.store: s3: the directory store cannot presign a URL" -}}
{{- end -}}
{{- if not $public -}}
{{- fail "orchestrator.artifacts.s3.downloads: redirect needs a public endpoint, the host the browser is sent to: rustfs.ingress.enabled (the files host), or orchestrator.artifacts.s3.publicEndpoint" -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "invalid" $art.s3.presignTtlSecs) -}}
{{- if not $s3on -}}
{{- fail "orchestrator.artifacts.s3.presignTtlSecs needs orchestrator.artifacts.store: s3" -}}
{{- end -}}
{{- include "agentic.checkWhole" (dict "name" "orchestrator.artifacts.s3.presignTtlSecs" "value" $art.s3.presignTtlSecs "min" 30 "max" 3600) -}}
{{- end -}}
{{- /* Uploads. */ -}}
{{- $perJob := int64 104857600 -}}
{{- if not (kindIs "invalid" $art.maxPerJobBytes) -}}
{{- include "agentic.checkWhole" (dict "name" "orchestrator.artifacts.maxPerJobBytes" "value" $art.maxPerJobBytes "min" 1 "max" 4294967296) -}}
{{- $perJob = int64 $art.maxPerJobBytes -}}
{{- end -}}
{{- if $art.upload.enabled -}}
{{- if not $s3on -}}
{{- fail "orchestrator.artifacts.upload.enabled needs orchestrator.artifacts.store: s3: the directory store cannot presign a URL" -}}
{{- end -}}
{{- if not $public -}}
{{- fail "orchestrator.artifacts.upload.enabled needs a public endpoint, the host of the presigned URLs an agent reaches the bucket at: rustfs.ingress.enabled (the files host), or orchestrator.artifacts.s3.publicEndpoint" -}}
{{- end -}}
{{- if not (has "thread-tools" .Values.orchestrator.surfaces) -}}
{{- fail "orchestrator.artifacts.upload.enabled needs the thread-tools surface in orchestrator.surfaces: request_upload is a tool of its endpoint (ADR 0058)" -}}
{{- end -}}
{{- if kindIs "invalid" $art.upload.maxBytes -}}
{{- fail "orchestrator.artifacts.upload.enabled needs orchestrator.artifacts.upload.maxBytes: the most one uploaded file may be, in bytes" -}}
{{- end -}}
{{- include "agentic.checkWhole" (dict "name" "orchestrator.artifacts.upload.maxBytes" "value" $art.upload.maxBytes "min" 1 "max" 5368709120) -}}
{{- if gt (int64 $art.upload.maxBytes) $perJob -}}
{{- fail (printf "orchestrator.artifacts.upload.maxBytes is more than the most bytes a job keeps (artifacts.maxPerJobBytes, %d): a file larger than a job may keep could never be kept; raise orchestrator.artifacts.maxPerJobBytes (up to 4294967296) or lower maxBytes" $perJob) -}}
{{- end -}}
{{- if not (kindIs "invalid" $art.upload.ttlSecs) -}}
{{- include "agentic.checkWhole" (dict "name" "orchestrator.artifacts.upload.ttlSecs" "value" $art.upload.ttlSecs "min" 30 "max" 3600) -}}
{{- end -}}
{{- end -}}
{{- /* The Job that moves the directory store's files into the bucket (ADR 0058, A5.4). It writes to the bucket of `store: s3`; with `fs` there is nothing to move to. */ -}}
{{- $mig := $art.migrate -}}
{{- if not (kindIs "bool" $mig.enabled) -}}
{{- fail (printf "orchestrator.artifacts.migrate.enabled must be true or false, got %v" $mig.enabled) -}}
{{- end -}}
{{- if not (kindIs "bool" $mig.dryRun) -}}
{{- fail (printf "orchestrator.artifacts.migrate.dryRun must be true or false, got %v" $mig.dryRun) -}}
{{- end -}}
{{- if $mig.enabled -}}
{{- if not $s3on -}}
{{- fail "orchestrator.artifacts.migrate.enabled needs orchestrator.artifacts.store: s3: the Job copies the files of the directory store into that bucket, and with fs there is no bucket" -}}
{{- end -}}
{{- include "agentic.checkWhole" (dict "name" "orchestrator.artifacts.migrate.activeDeadlineSeconds" "value" $mig.activeDeadlineSeconds "min" 60 "max" 86400) -}}
{{- end -}}
{{- /* Secrets. */ -}}
{{- if not .Values.externalSecrets.key -}}
{{- fail "externalSecrets.key is required: the AWS Secrets Manager secret that holds every value" -}}
{{- end -}}
{{- /* oauth2-proxy's session store: a closed set. `redis` needs the password's property (read by oauth2-proxy and by its Redis) and a Redis that
       cannot be told something else by a value. */ -}}
{{- if not (has (toString .Values.oauth2Proxy.sessionStore) (list "cookie" "redis")) -}}
{{- fail (printf "oauth2Proxy.sessionStore must be cookie or redis, got %q" (toString .Values.oauth2Proxy.sessionStore)) -}}
{{- end -}}
{{- if include "agentic.oauth2Redis" . -}}
{{- if and .Values.externalSecrets.enabled (not .Values.externalSecrets.properties.oauth2RedisPassword) -}}
{{- fail "oauth2Proxy.sessionStore redis needs externalSecrets.properties.oauth2RedisPassword: the property of the AWS secret that holds the Redis password (oauth2_redis_password); add it to the AWS secret before turning this on" -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.oauth2Proxy.redis.persistence.enabled) -}}
{{- fail (printf "oauth2Proxy.redis.persistence.enabled must be true or false, got %v" .Values.oauth2Proxy.redis.persistence.enabled) -}}
{{- end -}}
{{- if not (regexMatch "^[1-9][0-9]*(kb|mb|gb)?$" (toString .Values.oauth2Proxy.redis.maxMemory)) -}}
{{- fail (printf "oauth2Proxy.redis.maxMemory must be a number of bytes or one with kb, mb or gb (for example 64mb), got %q" (toString .Values.oauth2Proxy.redis.maxMemory)) -}}
{{- end -}}
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
{{- if or .Values.chat.enabled .Values.browser.enabled -}}{{- $pinned = append $pinned (dict "n" "chat" "i" .Values.chat.image) -}}{{- end -}}
{{- if .Values.browser.enabled -}}{{- $pinned = append $pinned (dict "n" "browser.obscura" "i" .Values.browser.obscura.image) -}}{{- end -}}
{{- if .Values.rustfs.enabled -}}{{- $pinned = append $pinned (dict "n" "rustfs" "i" .Values.rustfs.image) -}}{{- end -}}
{{- if include "agentic.oauth2Redis" . -}}{{- $pinned = append $pinned (dict "n" "oauth2Proxy.redis" "i" .Values.oauth2Proxy.redis.image) -}}{{- end -}}
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
{{- if not (hasKey $names (toString $a)) -}}
{{- fail (printf "%s.agents: %q is not the id or an alias of an agent in `agents` (the orchestrator refuses it at startup)" $at (toString $a)) -}}
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
{{- /* The other databases live in that cluster, each with a role whose password is a property of the AWS secret. The coders' (the older
       `sharedDatabase.coder` and `sharedDatabase.coders`) are checked by the helper that reads them. */ -}}
{{- $_ := include "agentic.db.coders" . -}}
{{- if .Values.externalSecrets.enabled -}}
{{- if and .Values.chat.enabled (not .Values.externalSecrets.properties.agentDbPassword) -}}
{{- fail "chat.enabled needs externalSecrets.properties.agentDbPassword: the property of the AWS secret that holds the password of the database role `agent` (agent_db_password)" -}}
{{- end -}}
{{- end -}}
{{- if .Values.sharedDatabase.coder.enabled -}}
{{- if not (regexMatch "^[a-z0-9]([a-z0-9.-]*[a-z0-9])?$" (toString .Values.sharedDatabase.coder.secretName)) -}}
{{- fail "sharedDatabase.coder.secretName must be a Kubernetes Secret name (lower-case letters, digits, - and .)" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/* A whole number in a range, or the render fails: call with (dict "name" "a.b" "value" $v "min" 1 "max" 10). A values file gives a float64, `--set` an int64. */}}
{{- define "agentic.checkWhole" -}}
{{- $v := .value -}}
{{- if not (or (kindIs "float64" $v) (kindIs "int64" $v) (kindIs "int" $v)) -}}
{{- fail (printf "%s must be a whole number from %d to %d, got %v" .name (int64 .min) (int64 .max) $v) -}}
{{- end -}}
{{- $f := float64 $v -}}
{{- if or (ne (floor $f) $f) (lt $f (float64 .min)) (gt $f (float64 .max)) -}}
{{- fail (printf "%s must be a whole number from %d to %d, got %v" .name (int64 .min) (int64 .max) $v) -}}
{{- end -}}
{{- end -}}

{{/*
A list of exact origins, or the render fails: call with (dict "name" "a.b" "origins" $list). The rules of `orchestrator.cors.allowedOrigins`
(scheme://host[:port], lower case, no path, never *, http:// only for this machine, no default port) and no origin twice: the files host's
proxy compares each with the request's Origin header, exactly, and a `*` there would be a wildcard.
*/}}
{{- define "agentic.checkOrigins" -}}
{{- if not (kindIs "slice" .origins) -}}
{{- fail (printf "%s must be a list of origins" .name) -}}
{{- end -}}
{{- $seen := dict -}}
{{- range .origins -}}
{{- if or (not (kindIs "string" .)) (not (regexMatch "^[a-z][a-z0-9+.-]*://[a-z0-9.-]+(:[0-9]{1,5})?$" .)) -}}
{{- fail (printf "%s: %v is not one exact origin (scheme://host[:port], lower case, no path, no trailing slash, never *)" $.name .) -}}
{{- end -}}
{{- if and (hasPrefix "http://" .) (not (regexMatch "^http://(localhost|127\\.[0-9]{1,3}\\.[0-9]{1,3}\\.[0-9]{1,3}|[a-z0-9.-]+\\.localhost)(:[0-9]{1,5})?$" .)) -}}
{{- fail (printf "%s: %v is http:// off this machine; a browser reads files from an https origin (http:// is taken for localhost, a loopback address or a name under .localhost)" $.name .) -}}
{{- end -}}
{{- if or (regexMatch "^http://[^/]+:80$" .) (regexMatch "^https://[^/]+:443$" .) -}}
{{- fail (printf "%s: %v names its scheme's default port, which a browser leaves out of Origin: write it without the port" $.name .) -}}
{{- end -}}
{{- if hasKey $seen . -}}
{{- fail (printf "%s: %v is listed twice" $.name .) -}}
{{- end -}}
{{- $_ := set $seen . true -}}
{{- end -}}
{{- end -}}

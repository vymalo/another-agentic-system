# syntax=docker/dockerfile:1.7
# Build context is the REPOSITORY ROOT (the API types are generated from docs/api/chat-api.yaml, which
# references the vendored AG-UI JSON Schema by file):
#   docker build -f web/Dockerfile -t web .
FROM node:24-bookworm-slim AS base
ENV PNPM_HOME=/pnpm \
    PATH=/pnpm:$PATH \
    NEXT_TELEMETRY_DISABLED=1
RUN corepack enable

FROM base AS deps
WORKDIR /app/web
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
# patchedDependencies (pnpm-workspace.yaml) must be on disk when pnpm resolves the install
COPY web/patches ./patches
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile

FROM base AS build
WORKDIR /app
COPY docs/api/chat-api.yaml docs/api/chat-api.yaml
COPY orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json orchestrator/crates/agui-proto/schema/ag-ui-1.0.schema.json
COPY web/ web/
COPY --from=deps /app/web/node_modules web/node_modules
WORKDIR /app/web
# Runs gen:api (types from ../docs/api/chat-api.yaml), then `next build`, a static export to out/ (ADR 0047), then
# scripts/csp-meta.ts, which writes each page's script hashes into it (the content security policy's meta half).
# NEXT_PUBLIC_SIGN_IN_PATH is the edge's sign-in path (oauth2-proxy: /oauth2/start); Next inlines it
# at build time, so it is a build argument. Empty (the default): a 401 is an error line, not a redirect.
ARG NEXT_PUBLIC_SIGN_IN_PATH=""
# NEXT_PUBLIC_BUILD_REVISION is the commit this image is built from (the workflow passes the sha). Next inlines it, and the web
# sends it as the header `X-Web-Revision` when it asks for a thread export, so the file says which web saved it (ADR 0053).
# Empty (the default): the export says `unknown`.
ARG NEXT_PUBLIC_BUILD_REVISION=""
RUN pnpm build

# The runtime is Caddy serving the export (web/Caddyfile): no Node, no node_modules. The same Caddy as the chart's edge, pinned
# by tag and digest. It listens on 3000 as an unprivileged user and writes nothing (`persist_config off`, no certificates).
FROM caddy:2.11.4-alpine@sha256:6aeddd44c3078b0f9a35206472a11420648a79c184603ef95957d0a20044cb2b AS runtime
COPY web/Caddyfile /etc/caddy/Caddyfile
COPY --from=build /app/web/out /srv
# The issuer's origin, which the policy lets the page connect to (the chart sets it in browser mode); empty: this origin only.
ENV WEB_CSP_CONNECT_SRC="" \
    XDG_CONFIG_HOME=/tmp/caddy-config \
    XDG_DATA_HOME=/tmp/caddy-data
USER 1000:1000
EXPOSE 3000
CMD ["caddy", "run", "--config", "/etc/caddy/Caddyfile", "--adapter", "caddyfile"]

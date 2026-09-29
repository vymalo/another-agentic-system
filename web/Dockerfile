# syntax=docker/dockerfile:1.7
# Build context is the REPOSITORY ROOT (the API types are generated from docs/api/chat-api.yaml):
#   docker build -f web/Dockerfile -t web .
FROM node:24-bookworm-slim AS base
ENV PNPM_HOME=/pnpm \
    PATH=/pnpm:$PATH \
    NEXT_TELEMETRY_DISABLED=1
RUN corepack enable

FROM base AS deps
WORKDIR /app/web
COPY web/package.json web/pnpm-lock.yaml web/pnpm-workspace.yaml ./
RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile

FROM base AS build
WORKDIR /app
COPY docs/api/chat-api.yaml docs/api/chat-api.yaml
COPY web/ web/
COPY --from=deps /app/web/node_modules web/node_modules
WORKDIR /app/web
# Runs gen:api (types from ../docs/api/chat-api.yaml), then `next build` (standalone output).
# MOCK_API_ORIGIN must stay unset: production routes /api/* at the ingress, same origin.
RUN pnpm build

FROM node:24-bookworm-slim AS runtime
WORKDIR /app
ENV NODE_ENV=production \
    NEXT_TELEMETRY_DISABLED=1 \
    PORT=3000 \
    HOSTNAME=0.0.0.0
COPY --from=build --chown=node:node /app/web/.next/standalone ./
COPY --from=build --chown=node:node /app/web/.next/static ./.next/static
COPY --from=build --chown=node:node /app/web/public ./public
USER node
EXPOSE 3000
CMD ["node", "server.js"]

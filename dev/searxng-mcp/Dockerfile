# The web-search MCP server of the live stack (compose.live.yaml `searxng-mcp`; dev/README.md, "Web search for real") and, as
# the image ghcr.io/vymalo/another-agentic-system/searxng-mcp (.github/workflows/searxng-mcp.yml), of the deploy chart's
# `webSearch` (deploy/chart/README.md). It has
# no dependencies, only node's own modules, so nothing is installed and there is no lockfile to keep.
#
# node:24-alpine3.23 and its digest: the same base as dev/mock-mcp-search/Dockerfile and dev/smee/Dockerfile (Docker Hub
# registry API, library/node index, verified 2026-09-30).
FROM node:24-alpine3.23@sha256:9ec4a2e289874ed0d722e1772ec2de45d2801541db8612f3638b26f128c69ac2
WORKDIR /app
COPY server.mjs ./
ENV NODE_ENV=production PORT=8080
EXPOSE 8080
# The image's `node` user, numeric so that a runtime can check it.
USER 1000:1000
ENTRYPOINT ["node", "server.mjs"]

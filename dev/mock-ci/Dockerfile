# The CI stand-in of the local stack (dev/README.md, "CI"): polls the git server for branches an agent
# pushed and reports a signed result to the orchestrator's webhook. Tag and digest verified 2026-09-30
# (Docker Hub registry API, library/alpine:3.23 index).
FROM alpine:3.23@sha256:85fe1e81d6758c208f3e1eed4338a1997e19d4be002d4dd32d3100c9a8c010a0
RUN apk add --no-cache git curl openssl \
 && mkdir -p /var/lib/mock-ci \
 && adduser -D -u 10001 ci \
 && chown ci /var/lib/mock-ci
COPY mock-ci.sh /usr/local/bin/mock-ci.sh
USER ci
ENTRYPOINT ["/bin/sh", "/usr/local/bin/mock-ci.sh"]

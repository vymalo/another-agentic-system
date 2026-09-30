# smee-client for the `smee` profile of compose.yaml (dev/README.md, "Going live"): it forwards the webhooks
# that smee.io holds for a channel to the orchestrator. The project publishes no container image (verified
# 2026-09-30: no probot/smee-client on Docker Hub; the only one, deltaprojects/smee-client, was last pushed
# in 2020), so this is the node image with the npm package, both pinned.
#
# node:24-alpine3.23 and its digest: Docker Hub registry API, library/node index, 2026-09-30.
# smee-client 5.0.0 is the `latest` dist-tag on npm (published 2025-11-13; needs node ^20.18 or >=22).
FROM node:24-alpine3.23@sha256:9ec4a2e289874ed0d722e1772ec2de45d2801541db8612f3638b26f128c69ac2
RUN npm install --global --ignore-scripts --no-audit --no-fund smee-client@5.0.0
COPY entrypoint.sh /usr/local/bin/smee-entrypoint.sh
USER 1000:1000 # the image's `node` user, numeric so that a runtime can check it
ENTRYPOINT ["/bin/sh", "/usr/local/bin/smee-entrypoint.sh"]

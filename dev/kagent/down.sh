#!/usr/bin/env sh
# Deletes the kind cluster `kagent` that dev/kagent/up.sh made, with everything in it (Substrate, kagent, the mock model and their volumes).
# It touches nothing else: the compose stack is stopped with `docker compose ... down`, and the images kind pulled are in the node container,
# which goes with the cluster (the node image itself stays in the local docker: `docker image rm kindest/node` frees about 1 GB).
set -eu

command -v kind >/dev/null 2>&1 || { echo "down.sh: kind is required and was not found on the PATH" >&2; exit 2; }
if kind get clusters 2>/dev/null | grep -qx kagent; then
  kind delete cluster --name kagent
else
  echo "no kind cluster named kagent"
fi

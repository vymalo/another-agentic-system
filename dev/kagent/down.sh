#!/usr/bin/env sh
# Deletes the kind cluster that dev/kagent/up.sh (kagent 1.x, the cluster `kagent`) or dev/kagent/up-0.10.sh (kagent 0.10, `kagent010`, with
# KAGENT_VERSION=0.10) made, with everything in it (Substrate, kagent, the mock model and their volumes).
# It touches nothing else: the compose stack is stopped with `docker compose ... down`, and the images kind pulled are in the node container,
# which goes with the cluster (the node image itself stays in the local docker: `docker image rm kindest/node` frees about 1 GB).
set -eu

command -v kind >/dev/null 2>&1 || { echo "down.sh: kind is required and was not found on the PATH" >&2; exit 2; }
case ${KAGENT_VERSION:-1.x} in
  1.x) cluster=kagent ;;
  0.10) cluster=kagent010 ;;
  *) echo "down.sh: KAGENT_VERSION is 1.x or 0.10, not '$KAGENT_VERSION'" >&2; exit 2 ;;
esac
if kind get clusters 2>/dev/null | grep -qx "$cluster"; then
  kind delete cluster --name "$cluster"
else
  echo "no kind cluster named $cluster"
fi

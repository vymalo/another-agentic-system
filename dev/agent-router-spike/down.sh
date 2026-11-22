#!/usr/bin/env sh
# Deletes the spike's kind cluster. Its images stay in Docker; remove them by hand if wanted.
set -eu
kind delete cluster --name agent-router-spike

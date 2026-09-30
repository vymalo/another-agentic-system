#!/bin/sh
# Starts smee-client for the channel in SMEE_URL and forwards to SMEE_TARGET (set by compose.yaml).
# Without a channel it says what to do and exits: the profile is opt-in, and an empty channel would make
# smee-client ask smee.io for a new one, which nothing here could tell GitHub about.
set -eu

case ${SMEE_URL:-} in
  https://* | http://*) ;;
  *)
    echo "SMEE_URL is not set (or is not a URL). Open https://smee.io/new, put the channel URL in .env as SMEE_URL and run again." >&2
    echo "smee.io is a third party: it sees every payload GitHub sends. See dev/README.md, 'Going live'." >&2
    exit 1
    ;;
esac

echo "forwarding the webhooks of ${SMEE_URL} to ${SMEE_TARGET}"
exec smee --url "$SMEE_URL" --target "${SMEE_TARGET:?SMEE_TARGET is not set}"

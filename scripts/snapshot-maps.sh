#!/usr/bin/env bash
# Compatibility entry point. Map querying, rendering, R2 upload, and freshness
# tracking live in the Rust `travel snapshot-maps` command.
set -euo pipefail
PLAN="${1:?usage: scripts/snapshot-maps.sh <plan-id> <dest-slug>}"
DEST="${2:?usage: scripts/snapshot-maps.sh <plan-id> <dest-slug>}"
exec ./bin/travel snapshot-maps --plan-id "$PLAN" --dest "$DEST"

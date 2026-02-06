#!/bin/bash
# Turso query helper
# Usage: ./scripts/turso-query.sh "SELECT * FROM offers LIMIT 5"

set -e
source "$(dirname "$0")/../.env"

SQL="${1:-SELECT name FROM sqlite_master WHERE type='table'}"

curl -s -X POST "https://travel-2026-yanggf8.aws-ap-northeast-1.turso.io/v2/pipeline" \
  -H "Authorization: Bearer $TURSO_TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"requests\": [{\"type\": \"execute\", \"stmt\": {\"sql\": \"$SQL\"}}]}" \
  | jq '.results[0].response.result.rows // .results[0]'

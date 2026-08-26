#!/usr/bin/env bash
set -euo pipefail
SERVER="${SOLAR_SERVER:-http://127.0.0.1:3000}"
DB="${SOLAR_HOT_DB:-solar-hot}"
NAME="${SOLAR_WORLD_NAME:-solar-dev}"
KEY="${SOLAR_WORLD_KEY:-1}"
curl -fsS -X POST "$SERVER/v1/database/$DB/call/solar_world_create_or_import" \
  -H 'content-type: application/json' \
  --data "[$KEY,\"$NAME\",\"SOLAR\",\"solar-dev\",0,-64,384,0.5,64.0,0.5,0.0,0.0,0,\"dev\",0,\"\"]" >/dev/null
curl -fsS -X POST "$SERVER/v1/database/$DB/call/solar_world_finish_import" \
  -H 'content-type: application/json' --data "[$KEY,true,\"\"]" >/dev/null
echo "Solar world '$NAME' ($KEY) is active in $DB"

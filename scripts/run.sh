#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORKSPACE="$(cd "$ROOT/../.." && pwd)"
STOMZY_ROOT="${SOLAR_STOMZY_ROOT:-$WORKSPACE/stomzy}"
DEFAULT_HOT="$WORKSPACE/local-dev/spacetime/hot"
if [[ ! -f "$DEFAULT_HOT/Cargo.toml" ]]; then DEFAULT_HOT="$WORKSPACE/spacetime/hot"; fi
HOT_MODULE="${SOLAR_HOT_MODULE:-$DEFAULT_HOT}"
HOT_DB="${SOLAR_HOT_DB:-solar-hot}"
WORLD_NAME="${SOLAR_WORLD_NAME:-solar-dev}"
WORLD_KEY="${SOLAR_WORLD_KEY:-1}"
DATA_DIR="${SOLAR_DATA_DIR:-$ROOT/scripts/.data}"
LOG_DIR="$ROOT/scripts/.logs"
mkdir -p "$DATA_DIR" "$LOG_DIR"
command -v spacetime >/dev/null || { echo 'spacetime CLI is required on PATH' >&2; exit 1; }
[[ -f "$STOMZY_ROOT/gradlew" && -f "$HOT_MODULE/Cargo.toml" ]] || { echo 'Stomzy or Hot module checkout missing' >&2; exit 1; }
spacetime start --listen-addr 127.0.0.1:3000 --data-dir "$DATA_DIR" >"$LOG_DIR/spacetime.out.log" 2>"$LOG_DIR/spacetime.err.log" &
stdb=$!
trap 'kill "$stdb" 2>/dev/null || true' EXIT INT TERM
for _ in $(seq 1 30); do curl -fsS http://127.0.0.1:3000/ >/dev/null 2>&1 && break; sleep 1; done
curl -fsS http://127.0.0.1:3000/ >/dev/null
spacetime publish -s http://127.0.0.1:3000 -p "$HOT_MODULE" "$HOT_DB" --delete-data=on-conflict -y
if [[ "${SOLAR_SEED_WORLD:-false}" == true ]]; then "$ROOT/scripts/seed-world.sh"; fi
export SPACETIME_ENABLED=true SPACETIME_URI=ws://127.0.0.1:3000 SPACETIME_DB="$HOT_DB"
export STOMZY_SOLAR_ONLY=true
export STOMZY_HOT_MODE=local SPACETIME_HOT_URI=ws://127.0.0.1:3000 SPACETIME_HOT_DB="$HOT_DB"
export MINESTOM_SERVER_ID=solar-dev MINESTOM_WORLD_SOURCE=solar MINESTOM_SPAWN_WORLD="$WORLD_NAME"
export SOLAR_WORLD_KEY="$WORLD_KEY" SOLAR_IMPORT_FROM_POLAR=false STDB_MAINCLOUD_URI=""
cd "$STOMZY_ROOT"
if [[ "${SOLAR_NO_BUILD:-false}" != true ]]; then ./gradlew :demo:installDist --no-daemon; fi
exec "$STOMZY_ROOT/demo/build/install/demo/bin/demo"

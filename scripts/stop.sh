#!/usr/bin/env bash
set -euo pipefail
DATA_DIR="${SOLAR_DATA_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.data}"
pkill -f "spacetime start.*--data-dir $DATA_DIR" 2>/dev/null || true

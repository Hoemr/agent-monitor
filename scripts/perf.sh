#!/usr/bin/env bash
# Runs the full performance acceptance suite against the release binary.
# Total runtime: ~15 minutes (3 x 300 s), plus a longer memory run.
set -euo pipefail
cd "$(dirname "$0")/.."
python3 scripts/perf.py --mode edge_hidden --seconds "${1:-300}" --label hidden
python3 scripts/perf.py --mode collapsed --seconds "${2:-300}" --label idle-empty --empty
python3 scripts/perf.py --mode expanded --seconds "${3:-300}" --label expanded
echo "results: docs/perf/*.json"

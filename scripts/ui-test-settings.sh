#!/usr/bin/env bash
# Headless Settings-window acceptance: renders src/settings.html?mock=1 in an
# iframe and drives real DOM events (switches, segmented controls, sliders,
# steppers, inline resets, Escape, arrow-key navigation). Requires Google Chrome.
set -euo pipefail
cd "$(dirname "$0")/.."
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[ -x "$CHROME" ] || { echo "Chrome not found; set CHROME=/path/to/chrome"; exit 1; }
OUT=$("$CHROME" --headless=new --disable-gpu --allow-file-access-from-files \
  --window-size=900,760 --virtual-time-budget=15000 \
  --dump-dom "file://$PWD/scripts/ui-selftest-settings.html" 2>/dev/null)
python3 - "$OUT" <<'PY'
import html, re, sys
s = sys.argv[1]
m = re.search(r'<pre id="results">(.*?)</pre>', s, re.S)
if not m:
    print("NO RESULTS"); sys.exit(1)
text = html.unescape(m.group(1))
print(text)
sys.exit(0 if text.startswith("ALL PASS") else 1)
PY

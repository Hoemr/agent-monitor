#!/usr/bin/env bash
# Headless UI acceptance for BOTH windows. Each suite renders the shipped page
# with ?mock=1 inside an iframe and drives real DOM events (click/drag/hover/
# keyboard/settings events), then requires the first result line to be ALL PASS.
#
# Requires Google Chrome (set CHROME=/path/to/chrome to override).
set -uo pipefail
cd "$(dirname "$0")/.."
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
[ -x "$CHROME" ] || { echo "Chrome not found; set CHROME=/path/to/chrome"; exit 1; }

status=0

echo "== desktop pet (src/index.html) =="
OUT=$("$CHROME" --headless=new --disable-gpu --allow-file-access-from-files \
  --virtual-time-budget=20000 --dump-dom "file://$PWD/scripts/ui-selftest.html" 2>/dev/null)
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
[ $? -eq 0 ] || status=1

echo
echo "== settings window (src/settings.html) =="
if ! bash scripts/ui-test-settings.sh; then
  status=1
fi

echo
if [ "$status" -eq 0 ]; then
  echo "UI ACCEPTANCE: ALL SUITES PASS"
else
  echo "UI ACCEPTANCE: FAILURES (see above)"
fi
exit "$status"

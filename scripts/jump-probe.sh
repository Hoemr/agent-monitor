#!/usr/bin/env bash
# Reproduces the jump capability probes recorded in docs/JUMP.md.
# Read-only except that it activates apps / switches Codex+DSH sessions; it
# restores focus where possible. Requires Automation/Accessibility for the shell.
set -uo pipefail
cd "$(dirname "$0")/.."
BIN="${AM_BIN:-src-tauri/target/release/agent-monitor}"
[ -x "$BIN" ] || { echo "build first: (cd src-tauri && cargo build --release)"; exit 1; }

echo "== window inventory"
for p in "ChatGPT" "DeepSeek Harness" "Code" "ZCode" "Terminal"; do
  printf '%-18s ' "$p"
  osascript -e "tell application \"System Events\" to tell process \"$p\" to get title of every window" 2>&1 | head -1
done

echo "== frontmost"
osascript -e 'tell application "System Events" to get name of first application process whose frontmost is true'

echo "== task snapshot (ids + declared levels)"
"$BIN" --dump-tasks | python3 -c 'import json,sys; d=json.load(sys.stdin); [print(t["id"], t["level"], t["status"], "|", t["title"][:40]) for t in d["tasks"]]' | head -20

echo "== jump each agent once (manual step: choose a real id from above)"
echo "   $BIN --jump codex:<thread-id>"
echo "   $BIN --jump dsh:<session-id>"
echo "   $BIN --jump claude-code:<session-uuid>"
echo "   $BIN --jump zcode:<task-id>"

echo "== ZCode launch probe"
open -b dev.zcode.app
sleep 3
if pgrep -fl 'ZCode.app/Contents/MacOS/ZCode' >/dev/null; then
  osascript -e 'tell application "System Events" to tell process "ZCode" to get title of every window'
else
  echo "no ZCode process after open -b (documented blocker)"
fi

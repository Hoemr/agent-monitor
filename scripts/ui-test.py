#!/usr/bin/env python3
"""Run the shipped DOM suites on macOS or Windows without shell argument limits."""
import html
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_CHROME = (
    Path(os.environ.get("PROGRAMFILES", "C:/Program Files")) / "Google/Chrome/Application/chrome.exe"
    if sys.platform == "win32" else
    Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
)


def main():
    chrome = Path(os.environ.get("CHROME", str(DEFAULT_CHROME)))
    if not chrome.is_file():
        raise SystemExit("Chrome not found; set CHROME to the browser executable")
    passed = True
    for suite in ["ui-selftest.html", "ui-selftest-settings.html", "ui-selftest-windows.html", "ui-selftest-reduced-motion.html", "ui-selftest-island.html"]:
        media_flags = ["--force-prefers-reduced-motion"] if "reduced-motion" in suite else []
        result = subprocess.run([
            str(chrome), "--headless=new", "--disable-gpu", "--allow-file-access-from-files",
            "--window-size=900,760", "--virtual-time-budget=25000", "--dump-dom",
            *media_flags,
            (ROOT / "scripts" / suite).as_uri(),
        ], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=90)
        dom = result.stdout.decode("utf-8", errors="replace")
        match = re.search(r'<pre id="results">(.*?)</pre>', dom, re.S)
        report = html.unescape(match.group(1)) if match else "NO RESULTS"
        ok = result.returncode == 0 and report.startswith("ALL PASS")
        passed = passed and ok
        checks = sum(line.startswith("PASS ") for line in report.splitlines())
        print(f"{suite}: {'ALL PASS' if ok else 'FAILED'} ({checks} checks)", flush=True)
        if not ok:
            print(report, flush=True)
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())

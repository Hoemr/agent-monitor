#!/usr/bin/env python3
"""Performance sampler for Agent Monitor.

Measures the *real* release binary (no mocks): CPU time delta and RSS sampled
every 5 s, then reports mean/p95/max CPU and RSS growth.

Usage:
  python3 scripts/perf.py --mode edge_hidden --seconds 300 --label hidden
  python3 scripts/perf.py --mode collapsed --seconds 300 --label idle-empty --empty
  python3 scripts/perf.py --mode expanded --seconds 300 --label expanded

The app is launched with AGENT_MONITOR_START_MODE (acceptance aid) and, for the
"no tasks" case, AGENT_MONITOR_EMPTY=1 (skips adapters; not a product path).
"""
import argparse
import csv
import json
import os
import signal
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.environ.get("AM_BIN", os.path.join(ROOT, "src-tauri", "target", "release", "agent-monitor"))


def cputime_seconds(pid):
    out = subprocess.run(["ps", "-o", "cputime=", "-p", str(pid)],
                         capture_output=True, text=True).stdout.strip()
    if not out:
        return None
    # formats: MM:SS.cc, HH:MM:SS.cc, D-HH:MM:SS
    days = 0
    if "-" in out:
        d, out = out.split("-", 1)
        days = int(d)
    parts = out.split(":")
    parts = [float(p) for p in parts]
    secs = 0.0
    for p in parts:
        secs = secs * 60 + p
    return days * 86400 + secs


def rss_kb(pid):
    out = subprocess.run(["ps", "-o", "rss=", "-p", str(pid)],
                         capture_output=True, text=True).stdout.strip()
    try:
        return int(out)
    except ValueError:
        return None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", default="collapsed", choices=["collapsed", "expanded", "edge_hidden"])
    ap.add_argument("--seconds", type=int, default=300)
    ap.add_argument("--label", default="run")
    ap.add_argument("--empty", action="store_true")
    ap.add_argument("--warmup", type=int, default=8)
    args = ap.parse_args()

    if not os.path.exists(BIN):
        sys.exit(f"binary not found: {BIN} (build with: cargo build --release)")
    subprocess.run(["pkill", "-f", "agent-monitor"], capture_output=True)
    time.sleep(1)

    env = dict(os.environ)
    env["AGENT_MONITOR_START_MODE"] = args.mode
    if args.empty:
        env["AGENT_MONITOR_EMPTY"] = "1"
    log = f"/tmp/am-perf-{args.label}.log"
    with open(log, "w") as fh:
        proc = subprocess.Popen([BIN], env=env, stdout=fh, stderr=fh)
    time.sleep(args.warmup)
    if proc.poll() is not None:
        sys.exit(f"app exited during warmup, see {log}")

    samples = []
    start_cpu = cputime_seconds(proc.pid)
    start_rss = rss_kb(proc.pid)
    t0 = time.time()
    last_cpu, last_wall = start_cpu, t0
    while time.time() - t0 < args.seconds:
        time.sleep(5)
        cpu = cputime_seconds(proc.pid)
        rss = rss_kb(proc.pid)
        now = time.time()
        if cpu is None or rss is None:
            break
        wall_delta = now - last_wall
        cpu_pct = 100.0 * (cpu - last_cpu) / wall_delta if wall_delta > 0 else 0.0
        samples.append({
            "t_s": round(now - t0, 1),
            "cpu_pct": round(cpu_pct, 3),
            "rss_mb": round(rss / 1024.0, 2),
        })
        last_cpu, last_wall = cpu, now

    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()

    cpu_values = [s["cpu_pct"] for s in samples]
    rss_values = [s["rss_mb"] for s in samples]
    cpu_values_sorted = sorted(cpu_values)
    p95 = cpu_values_sorted[int(len(cpu_values_sorted) * 0.95) - 1] if cpu_values_sorted else 0.0
    summary = {
        "label": args.label,
        "mode": args.mode,
        "empty_snapshot": args.empty,
        "duration_s": round(time.time() - t0, 1),
        "samples": len(samples),
        "cpu_mean_pct": round(sum(cpu_values) / len(cpu_values), 3) if cpu_values else None,
        "cpu_p95_pct": round(p95, 3),
        "cpu_max_pct": round(max(cpu_values), 3) if cpu_values else None,
        "rss_start_mb": round(start_rss / 1024.0, 2) if start_rss else None,
        "rss_end_mb": rss_values[-1] if rss_values else None,
        "rss_growth_mb": round(rss_values[-1] - start_rss / 1024.0, 2)
        if rss_values and start_rss else None,
        "environment": {
            "os": subprocess.run(["sw_vers", "-productVersion"], capture_output=True, text=True).stdout.strip(),
            "arch": subprocess.run(["uname", "-m"], capture_output=True, text=True).stdout.strip(),
            "binary": os.path.relpath(BIN, ROOT),
            "profile": "release" if "/release/" in BIN else "debug",
        },
        "samples_detail": samples,
        "log": log,
    }
    os.makedirs(os.path.join(ROOT, "docs", "perf"), exist_ok=True)
    out_csv = os.path.join(ROOT, "docs", "perf", f"{args.label}.csv")
    with open(out_csv, "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=["t_s", "cpu_pct", "rss_mb"])
        w.writeheader()
        w.writerows(samples)
    out_json = os.path.join(ROOT, "docs", "perf", f"{args.label}.json")
    with open(out_json, "w") as fh:
        json.dump(summary, fh, indent=2)
    print(json.dumps({k: v for k, v in summary.items() if k != "samples_detail"}, indent=2))


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Controlled total-system CPU & RSS benchmark for Slackless and Official Slack.

Uses exact cumulative CPU time deltas (`ps -o time=`) rather than `ps %cpu`
exponentially decayed averages, and simultaneously tracks macOS system daemons
(`WindowServer`, `coreaudiod`, `VTEncoderXPCService`, `VTDecoderXPCService`,
`aned`, `screencaptureui`, `replayd`) so hardware/framework offloads are measured
honestly across the entire system.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import time

SYSTEM_DAEMONS = (
    "WindowServer",
    "coreaudiod",
    "VTEncoderXPCService",
    "VTDecoderXPCService",
    "aned",
    "screencaptureui",
    "replayd",
)


def parse_ps_time(raw: str) -> float:
    """Parse macOS `ps -o time=` (`MM:SS.cc` or `H:MM:SS.cc` or `D-HH:MM:SS`) into seconds."""
    raw = raw.strip().replace(",", ".")
    days = 0
    if "-" in raw:
        d_part, raw = raw.split("-", 1)
        days = int(d_part)
    parts = raw.split(":")
    if len(parts) == 2:
        mins, secs = int(parts[0]), float(parts[1])
        return days * 86400 + mins * 60 + secs
    if len(parts) == 3:
        hrs, mins, secs = int(parts[0]), int(parts[1]), float(parts[2])
        return days * 86400 + hrs * 3600 + mins * 60 + secs
    return float(raw)


def snapshot_processes() -> dict[int, tuple[str, float, float]]:
    """Return {pid: (basename, rss_mb, cpu_seconds)} for all running processes."""
    env = dict(os.environ, LC_ALL="C")
    out = subprocess.check_output(
        ["ps", "-axo", "pid=,rss=,time=,comm="], text=True, env=env
    )
    snap: dict[int, tuple[str, float, float]] = {}
    for line in out.splitlines():
        line = line.strip()
        if not line:
            continue
        parts = line.split(None, 3)
        if len(parts) < 4:
            continue
        try:
            pid = int(parts[0])
            rss_mb = int(parts[1]) / 1024.0
            cpu_s = parse_ps_time(parts[2])
            comm = os.path.basename(parts[3])
        except ValueError:
            continue
        snap[pid] = (comm, rss_mb, cpu_s)
    return snap


def find_app_pids(snap: dict[int, tuple[str, float, float]], app: str) -> list[int]:
    if app == "slackless":
        return [pid for pid, (comm, _, _) in snap.items() if comm == "slackless-gui"]
    # Official Slack Electron process tree
    return [
        pid
        for pid, (comm, _, _) in snap.items()
        if comm == "Slack" or comm.startswith("Slack Helper")
    ]


def measure_window(
    app: str,
    label: str,
    secs: float = 6.0,
    baseline_daemons: dict[str, float] | None = None,
) -> dict[str, float]:
    s0 = snapshot_processes()
    t0 = time.monotonic()
    time.sleep(secs)
    s1 = snapshot_processes()
    dt = max(time.monotonic() - t0, 0.001)

    app_pids = find_app_pids(s1, app)
    app_cpu_s = 0.0
    app_rss_mb = 0.0
    for pid in app_pids:
        _, rss1, c1 = s1[pid]
        app_rss_mb += rss1
        if pid in s0:
            app_cpu_s += max(0.0, c1 - s0[pid][2])
    app_cpu_pct = (app_cpu_s / dt) * 100.0

    daemon_pcts: dict[str, float] = {}
    for dname in SYSTEM_DAEMONS:
        d_cpu_s = 0.0
        for pid, (comm, _, c1) in s1.items():
            if comm == dname and pid in s0:
                d_cpu_s += max(0.0, c1 - s0[pid][2])
        daemon_pcts[dname] = (d_cpu_s / dt) * 100.0

    daemon_total = sum(daemon_pcts.values())
    delta_str = ""
    if baseline_daemons is not None:
        deltas = {
            k: daemon_pcts[k] - baseline_daemons.get(k, 0.0) for k in SYSTEM_DAEMONS
        }
        non_trivial = [
            f"{k}:{v:+.1f}%" for k, v in deltas.items() if abs(v) >= 0.8
        ]
        delta_total = sum(deltas.values())
        delta_str = f" | sys_daemons_delta={delta_total:+5.1f}% ({', '.join(non_trivial) or 'none'})"
    else:
        top = [f"{k}:{v:.1f}%" for k, v in daemon_pcts.items() if v >= 0.8]
        delta_str = f" | sys_daemons={daemon_total:5.1f}% ({', '.join(top) or 'none'})"

    print(
        f"{label:30s} | app({len(app_pids)}p)={app_cpu_pct:5.1f}% CPU  {app_rss_mb:6.0f} MB{delta_str}",
        flush=True,
    )
    return daemon_pcts


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--app",
        choices=("slackless", "slack"),
        default="slackless",
        help="Target application process tree to measure",
    )
    parser.add_argument(
        "--label",
        default="Current state",
        help="Label for a single-shot measurement",
    )
    parser.add_argument(
        "--secs",
        type=float,
        default=6.0,
        help="Measurement window in seconds (exact cumulative CPU delta)",
    )
    parser.add_argument(
        "--baseline-secs",
        type=float,
        default=0.0,
        help="Optional pre-measurement daemon baseline window (seconds)",
    )
    args = parser.parse_args()

    base = None
    if args.baseline_secs > 0:
        base = measure_window(args.app, "0. Baseline (daemons)", args.baseline_secs)
    measure_window(args.app, args.label, args.secs, base)
    return 0


if __name__ == "__main__":
    sys.exit(main())

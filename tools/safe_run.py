#!/usr/bin/env python3
"""Compatibility CLI for direct binary runs under shared Molt custody.

Historically this script owned its own process group, RSS sampler, and signal
teardown. That made it a parallel process-custody authority.
It now preserves the old command-line shape while delegating all execution,
sampling, timeout, and termination to tools/memory_guard.py.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

EXIT_TIMEOUT = 124
EXIT_OOM = 137
EXIT_SPAWN = 125

ROOT = Path(__file__).resolve().parents[1]
MEMORY_GUARD = ROOT / "tools" / "memory_guard.py"
SUMMARY_ROOT = ROOT / "tmp" / "safe_run"


def _parse_args(argv: list[str]) -> tuple[argparse.Namespace, list[str]]:
    # Split on the first standalone "--" so the child command keeps its own flags.
    if "--" in argv:
        sep = argv.index("--")
        ours, cmd = argv[:sep], argv[sep + 1 :]
    else:
        # No explicit separator: take leading --opt[=val] / known flags for us,
        # the rest is the command. We only own a fixed, known flag set, so the
        # first token that is not one of ours starts the command.
        ours, cmd = [], []
        known_val = {"--rss-mb", "--timeout", "--poll", "--label"}
        known_flag = {"--quiet", "--json"}
        i = 0
        while i < len(argv):
            tok = argv[i]
            if tok in known_val:
                ours += [tok, argv[i + 1]] if i + 1 < len(argv) else [tok]
                i += 2
                continue
            if tok.split("=", 1)[0] in known_val and "=" in tok:
                ours.append(tok)
                i += 1
                continue
            if tok in known_flag:
                ours.append(tok)
                i += 1
                continue
            cmd = argv[i:]
            break
        else:
            cmd = []

    p = argparse.ArgumentParser(prog="safe_run.py", add_help=True)
    p.add_argument("--rss-mb", type=int, default=2048)
    p.add_argument("--timeout", type=float, default=30.0)
    p.add_argument("--poll", type=float, default=0.2)
    p.add_argument("--label", default=None)
    p.add_argument("--quiet", action="store_true")
    p.add_argument("--json", action="store_true")
    ns = p.parse_args(ours)
    return ns, cmd


def _summary_path(label: str) -> Path:
    safe = "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in label)
    SUMMARY_ROOT.mkdir(parents=True, exist_ok=True)
    return SUMMARY_ROOT / f"{os.getpid()}-{safe}.summary.json"


def _load_summary(path: Path) -> dict[str, object]:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return {}


def _rss_mib(record: object) -> float:
    if not isinstance(record, dict):
        return 0.0
    rss_kb = record.get("rss_kb")
    if isinstance(rss_kb, int | float):
        return float(rss_kb) / 1024.0
    rss_gb = record.get("rss_gb")
    if isinstance(rss_gb, int | float):
        return float(rss_gb) * 1024.0
    return 0.0


def _status(returncode: int, summary: dict[str, object]) -> str:
    if summary.get("timed_out"):
        return "timeout"
    if summary.get("violation") is not None:
        return "oom"
    if returncode == EXIT_TIMEOUT:
        return "timeout"
    if returncode == EXIT_OOM:
        return "oom"
    return "ok"


def _guard_command(
    ns: argparse.Namespace, cmd: list[str], summary_path: Path
) -> list[str]:
    rss_gb = ns.rss_mb / 1024.0
    return [
        sys.executable,
        str(MEMORY_GUARD),
        "--max-rss-gb",
        f"{rss_gb:.6f}",
        "--max-total-rss-gb",
        f"{rss_gb:.6f}",
        "--poll-interval",
        str(ns.poll),
        "--timeout",
        str(ns.timeout),
        "--summary-json",
        str(summary_path),
        "--",
        *cmd,
    ]


def main(argv: list[str]) -> int:
    ns, cmd = _parse_args(argv)
    if not cmd:
        print("safe_run.py: no command given (use `-- CMD ARGS`)", file=sys.stderr)
        return EXIT_SPAWN

    label = ns.label or os.path.basename(cmd[0])
    summary_path = _summary_path(label)
    start = time.monotonic()
    try:
        completed = subprocess.run(_guard_command(ns, cmd, summary_path), check=False)
    except FileNotFoundError:
        print(f"safe_run.py: command not found: {cmd[0]}", file=sys.stderr)
        return EXIT_SPAWN
    except Exception as exc:  # noqa: BLE001 - report any spawn failure cleanly
        print(
            f"safe_run.py: failed to start memory guard for {cmd[0]}: {exc}",
            file=sys.stderr,
        )
        return EXIT_SPAWN

    summary = _load_summary(summary_path)
    try:
        summary_path.unlink(missing_ok=True)
    except OSError:
        pass

    rc = completed.returncode
    elapsed = float(summary.get("elapsed_s") or (time.monotonic() - start))
    peak_mib = max(_rss_mib(summary.get("peak")), _rss_mib(summary.get("peak_total")))
    status = _status(rc, summary)
    if ns.json:
        print(
            "SAFE_RUN "
            + json.dumps(
                {
                    "label": label,
                    "status": status,
                    "exit": rc,
                    "peak_rss_mib": round(peak_mib),
                    "elapsed_s": round(elapsed, 3),
                    "rss_limit_mib": ns.rss_mb,
                    "timeout_s": ns.timeout,
                }
            ),
            file=sys.stderr,
        )
    elif not (ns.quiet and status == "ok"):
        print(
            f"SAFE_RUN [{label}] status={status} exit={rc} "
            f"peak_rss={peak_mib:.0f}MiB elapsed={elapsed:.2f}s "
            f"limit_rss={ns.rss_mb}MiB limit_t={ns.timeout:g}s",
            file=sys.stderr,
        )
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

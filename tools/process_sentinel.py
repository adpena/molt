#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
import contextlib
from dataclasses import dataclass
import json
import os
from pathlib import Path
import signal
import sys
import time

_THIS_FILE = Path(__file__).resolve()
_REPO_ROOT = _THIS_FILE.parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools import memory_guard  # noqa: E402


DEFAULT_MAX_PROCESS_RSS_GB = memory_guard.DEFAULT_MAX_RSS_GB
DEFAULT_MAX_TOTAL_RSS_GB = memory_guard.DEFAULT_MAX_GLOBAL_RSS_GB
DEFAULT_POLL_INTERVAL_SEC = 0.2
DEFAULT_GRACE_SEC = 0.5

MOLT_PROCESS_TOKENS = (
    "/molt/target/",
    "/tools/memory_guard.py",
    "/tests/molt_diff.py",
    "/tools/bench.py",
    "/tools/cpython_regrtest.py",
    "/src/molt/cli.py",
    "python -m molt",
    "molt.cli",
    "molt-backend",
    "molt_diff.py",
    "bench_exception_heavy",
    "exception-repro",
    "cpython_regrtest",
    "cargo build --package molt-backend",
    "cargo build -p molt-backend",
    "cargo test -p molt-backend",
    "runtime/molt-backend/src/",
)


@dataclass(frozen=True, slots=True)
class ProcessGroup:
    pgid: int
    samples: tuple[memory_guard.ProcessSample, ...]
    matched: bool

    @property
    def total_rss_kb(self) -> int:
        return sum(sample.rss_kb for sample in self.samples)

    @property
    def peak(self) -> memory_guard.ProcessSample | None:
        if not self.samples:
            return None
        return max(self.samples, key=lambda sample: sample.rss_kb)

    @property
    def pids(self) -> list[int]:
        return sorted(sample.pid for sample in self.samples)


@dataclass(frozen=True, slots=True)
class SentinelViolation:
    pgid: int
    reason: str
    total_rss_kb: int
    peak_pid: int | None
    peak_rss_kb: int | None
    pids: tuple[int, ...]
    command: str

    @property
    def total_rss_gb(self) -> float:
        return self.total_rss_kb / (1024 * 1024)

    @property
    def peak_rss_gb(self) -> float | None:
        if self.peak_rss_kb is None:
            return None
        return self.peak_rss_kb / (1024 * 1024)


def repo_root() -> Path:
    return _REPO_ROOT


def _normalized_repo_token(root: Path) -> str:
    return root.resolve().as_posix()


def is_molt_process(
    sample: memory_guard.ProcessSample,
    *,
    root: Path,
    self_pid: int | None = None,
) -> bool:
    if self_pid is not None and sample.pid == self_pid:
        return False
    command = sample.command
    if _normalized_repo_token(root) in command:
        return True
    return any(token in command for token in MOLT_PROCESS_TOKENS)


def process_groups(
    samples: Mapping[int, memory_guard.ProcessSample],
    *,
    root: Path,
    self_pid: int | None = None,
    self_pgid: int | None = None,
) -> list[ProcessGroup]:
    grouped: dict[int, list[memory_guard.ProcessSample]] = {}
    matched: set[int] = set()
    for sample in samples.values():
        pgid = sample.pgid if sample.pgid is not None else sample.pid
        if self_pgid is not None and pgid == self_pgid:
            continue
        grouped.setdefault(pgid, []).append(sample)
        if is_molt_process(sample, root=root, self_pid=self_pid):
            matched.add(pgid)
    groups = [
        ProcessGroup(
            pgid=pgid,
            samples=tuple(sorted(group, key=lambda item: item.pid)),
            matched=pgid in matched,
        )
        for pgid, group in grouped.items()
        if pgid in matched
    ]
    return sorted(groups, key=lambda group: group.pgid)


def find_violations(
    groups: Sequence[ProcessGroup],
    *,
    max_process_kb: int,
    max_total_kb: int,
    kill_all: bool = False,
) -> list[SentinelViolation]:
    violations: list[SentinelViolation] = []
    for group in groups:
        peak = group.peak
        if kill_all:
            reason = "kill_all"
        elif peak is not None and peak.rss_kb > max_process_kb:
            reason = "process_rss"
        elif group.total_rss_kb > max_total_kb:
            reason = "group_rss"
        else:
            continue
        violations.append(
            SentinelViolation(
                pgid=group.pgid,
                reason=reason,
                total_rss_kb=group.total_rss_kb,
                peak_pid=None if peak is None else peak.pid,
                peak_rss_kb=None if peak is None else peak.rss_kb,
                pids=tuple(group.pids),
                command="" if peak is None else peak.command,
            )
        )
    return violations


def _violation_payload(violation: SentinelViolation) -> dict[str, object]:
    return {
        "pgid": violation.pgid,
        "reason": violation.reason,
        "total_rss_kb": violation.total_rss_kb,
        "total_rss_gb": violation.total_rss_gb,
        "peak_pid": violation.peak_pid,
        "peak_rss_kb": violation.peak_rss_kb,
        "peak_rss_gb": violation.peak_rss_gb,
        "pids": list(violation.pids),
        "command": violation.command,
    }


def _format_violation(violation: SentinelViolation) -> str:
    peak = "-"
    if violation.peak_rss_gb is not None:
        peak = f"{violation.peak_rss_gb:.2f}GB"
    return (
        "[PROCESS-SENTINEL] "
        f"{violation.reason} pgid={violation.pgid} "
        f"total={violation.total_rss_gb:.2f}GB peak={peak} "
        f"pids={list(violation.pids)} command={violation.command}"
    )


def emit_violations(
    violations: Sequence[SentinelViolation],
    *,
    json_mode: bool,
    stream,
) -> None:
    for violation in violations:
        if json_mode:
            print(
                json.dumps(_violation_payload(violation), sort_keys=True), file=stream
            )
        else:
            print(_format_violation(violation), file=stream)
    with contextlib.suppress(Exception):
        stream.flush()


def terminate_group(pgid: int, *, grace: float) -> None:
    if pgid <= 0 or pgid == os.getpgrp():
        return
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(pgid, signal.SIGTERM)
    deadline = time.monotonic() + max(0.0, grace)
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except (ProcessLookupError, PermissionError):
            return
        time.sleep(0.05)
    with contextlib.suppress(ProcessLookupError, PermissionError):
        os.killpg(pgid, signal.SIGKILL)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Repo-scoped sentinel for Molt build/test/bench process groups."
    )
    parser.add_argument(
        "--repo-root",
        default=str(repo_root()),
        help="Repository root used to scope matching processes.",
    )
    parser.add_argument(
        "--max-process-rss-gb",
        type=float,
        default=DEFAULT_MAX_PROCESS_RSS_GB,
        help=f"Per-process RSS ceiling (default: {DEFAULT_MAX_PROCESS_RSS_GB}).",
    )
    parser.add_argument(
        "--max-total-rss-gb",
        type=float,
        default=DEFAULT_MAX_TOTAL_RSS_GB,
        help=(
            "Per-process-group RSS ceiling for matched Molt groups "
            f"(default: {DEFAULT_MAX_TOTAL_RSS_GB})."
        ),
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=DEFAULT_POLL_INTERVAL_SEC,
        help=f"Polling interval in seconds (default: {DEFAULT_POLL_INTERVAL_SEC}).",
    )
    parser.add_argument(
        "--grace-sec",
        type=float,
        default=DEFAULT_GRACE_SEC,
        help=f"SIGTERM grace period before SIGKILL (default: {DEFAULT_GRACE_SEC}).",
    )
    parser.add_argument(
        "--kill-all",
        action="store_true",
        help="Terminate every currently matched Molt process group.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Report violations without terminating process groups.",
    )
    parser.add_argument(
        "--once",
        action="store_true",
        help="Run one scan and exit.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON lines instead of compact text.",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        max_process_kb = memory_guard.max_rss_kb_from_gb(args.max_process_rss_gb)
        max_total_kb = memory_guard.max_global_rss_kb_from_gb(args.max_total_rss_gb)
        if args.poll_interval <= 0:
            raise ValueError("poll interval must be greater than 0")
        if args.grace_sec < 0:
            raise ValueError("grace period must be non-negative")
    except ValueError as exc:
        print(f"process_sentinel: {exc}", file=sys.stderr)
        return 2

    root = Path(args.repo_root).expanduser()
    stream = sys.stdout if args.json else sys.stderr
    while True:
        groups = process_groups(
            memory_guard.sample_processes(),
            root=root,
            self_pid=os.getpid(),
            self_pgid=os.getpgrp(),
        )
        violations = find_violations(
            groups,
            max_process_kb=max_process_kb,
            max_total_kb=max_total_kb,
            kill_all=args.kill_all,
        )
        emit_violations(violations, json_mode=args.json, stream=stream)
        if not args.dry_run:
            for violation in violations:
                terminate_group(violation.pgid, grace=args.grace_sec)
        if args.once:
            return 1 if violations else 0
        time.sleep(args.poll_interval)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

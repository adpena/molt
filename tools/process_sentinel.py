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

from tools import guarded_entrypoints, memory_guard  # noqa: E402


DEFAULT_MAX_PROCESS_RSS_GB = memory_guard.DEFAULT_MAX_RSS_GB
DEFAULT_MAX_GROUP_RSS_GB = memory_guard.DEFAULT_MAX_TOTAL_RSS_GB
DEFAULT_MAX_GLOBAL_RSS_GB = memory_guard.DEFAULT_MAX_GLOBAL_RSS_GB
DEFAULT_POLL_INTERVAL_SEC = 0.10
DEFAULT_GRACE_SEC = 0.5
DEFAULT_MAX_RUNTIME_SEC = 120.0
DEFAULT_STALE_ORPHAN_SEC = 60.0 * 60.0
DEFAULT_STALE_PYTEST_SEC = 15.0 * 60.0

GUARDED_ENTRYPOINT_TOKENS = guarded_entrypoints.guarded_entrypoint_tokens(_REPO_ROOT)

MOLT_PROCESS_TOKENS = tuple(
    dict.fromkeys(
        (
            "/tools/memory_guard.py",
            "/tests/molt_diff.py",
            *GUARDED_ENTRYPOINT_TOKENS,
            "python -m molt",
            "molt.cli",
            "molt-backend",
            "molt_diff.py",
            "bench_individual.py",
            "bench_wasm.py",
            "bench_exception_heavy",
            "exception-repro",
            "cpython_regrtest",
            "nightly_test_suite.py",
            "adapt_monty_tests.py",
            "run_molt_conformance.py",
            "run_monty_conformance.py",
            "cargo build --package molt-backend",
            "cargo build -p molt-backend",
            "cargo test -p molt-backend",
            "runtime/molt-backend/src/",
        )
    )
)

REPO_SCOPED_PROCESS_TOKENS = tuple(
    dict.fromkeys(
        (
            "/.molt_cache/home/bin/",
            "/.venv/bin/python",
            "/target/debug/",
            "/target/dev-fast/",
            "/target/release-fast/",
            "/tmp/",
            "/dist/",
            "/build/",
            "/wasm/",
            "/bench/results/",
            "/tests/molt_diff.py",
            "/tests/",
            *GUARDED_ENTRYPOINT_TOKENS,
        )
    )
)

PYTEST_PROCESS_TOKENS = (
    " pytest",
    "/pytest",
    "python -m pytest",
    "python3 -m pytest",
)

INSPECTION_COMMAND_TOKENS = (
    "tools/process_sentinel.py",
    "process_sentinel.py",
    " ps -",
    "ps -",
    " rg ",
    "rg '",
    'rg "',
    " grep ",
    "grep '",
    'grep "',
    "git diff",
    "git status",
    "find ",
    " find ",
    "head ",
    " head ",
    "tail ",
    " tail ",
    "cat ",
    " cat ",
    "nl -ba",
    "wc ",
    " wc ",
    "sed -n",
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

    @property
    def oldest_elapsed_sec(self) -> int | None:
        ages = [
            sample.elapsed_sec
            for sample in self.samples
            if sample.elapsed_sec is not None
        ]
        if not ages:
            return None
        return max(ages)

    @property
    def external_parent_pids(self) -> list[int]:
        pids = set(self.pids)
        return sorted(
            {
                sample.ppid
                for sample in self.samples
                if sample.ppid > 0 and sample.ppid not in pids
            }
        )

    @property
    def is_orphaned(self) -> bool:
        parents = self.external_parent_pids
        return (
            bool(self.samples)
            and bool(parents)
            and all(parent == 1 for parent in parents)
        )

    @property
    def command_text(self) -> str:
        return "\n".join(sample.command for sample in self.samples)

    @property
    def looks_like_pytest(self) -> bool:
        command = self.command_text
        return any(token in command for token in PYTEST_PROCESS_TOKENS)


@dataclass(frozen=True, slots=True)
class SentinelViolation:
    pgid: int
    reason: str
    total_rss_kb: int
    peak_pid: int | None
    peak_rss_kb: int | None
    pids: tuple[int, ...]
    command: str
    oldest_elapsed_sec: int | None = None
    stale_sec: float | None = None
    orphaned: bool = False

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
    if any(token in command for token in INSPECTION_COMMAND_TOKENS):
        return False
    repo_token = _normalized_repo_token(root)
    if repo_token in command and any(
        f"{repo_token}{token}" in command for token in REPO_SCOPED_PROCESS_TOKENS
    ):
        return True
    return any(token in command for token in MOLT_PROCESS_TOKENS)


def process_groups(
    samples: Mapping[int, memory_guard.ProcessSample],
    *,
    root: Path,
    self_pid: int | None = None,
    self_pgid: int | None = None,
    known_pgids: set[int] | None = None,
) -> list[ProcessGroup]:
    grouped: dict[int, list[memory_guard.ProcessSample]] = {}
    matched: set[int] = set() if known_pgids is None else set(known_pgids)
    for sample in samples.values():
        pgid = sample.pgid if sample.pgid is not None else sample.pid
        if self_pgid is not None and pgid == self_pgid:
            continue
        grouped.setdefault(pgid, []).append(sample)
        if is_molt_process(sample, root=root, self_pid=self_pid):
            matched.add(pgid)
    changed = True
    while changed:
        changed = False
        matched_pids = {
            sample.pid for pgid in matched for sample in grouped.get(pgid, ())
        }
        if not matched_pids:
            break
        for sample in samples.values():
            pgid = sample.pgid if sample.pgid is not None else sample.pid
            if self_pgid is not None and pgid == self_pgid:
                continue
            if pgid not in matched and sample.ppid in matched_pids:
                matched.add(pgid)
                changed = True
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
    max_group_kb: int,
    max_global_kb: int,
    kill_all: bool = False,
    stale_orphan_sec: float | None = None,
    stale_pytest_sec: float | None = None,
) -> list[SentinelViolation]:
    violations: list[SentinelViolation] = []
    global_total_kb = sum(group.total_rss_kb for group in groups)
    global_tripped = bool(groups) and global_total_kb > max_global_kb
    for group in groups:
        peak = group.peak
        stale_reason, stale_sec = _stale_violation_reason(
            group,
            stale_orphan_sec=stale_orphan_sec,
            stale_pytest_sec=stale_pytest_sec,
        )
        if kill_all:
            reason = "kill_all"
        elif peak is not None and peak.rss_kb > max_process_kb:
            reason = "process_rss"
        elif group.total_rss_kb > max_group_kb:
            reason = "group_rss"
        elif global_tripped:
            reason = "global_rss"
        elif stale_reason is not None:
            reason = stale_reason
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
                oldest_elapsed_sec=group.oldest_elapsed_sec,
                stale_sec=stale_sec,
                orphaned=group.is_orphaned,
            )
        )
    return violations


def _stale_violation_reason(
    group: ProcessGroup,
    *,
    stale_orphan_sec: float | None,
    stale_pytest_sec: float | None,
) -> tuple[str | None, float | None]:
    age = group.oldest_elapsed_sec
    if age is None or not group.is_orphaned:
        return None, None
    if (
        stale_pytest_sec is not None
        and group.looks_like_pytest
        and age >= stale_pytest_sec
    ):
        return "stale_pytest_orphan", stale_pytest_sec
    if stale_orphan_sec is not None and age >= stale_orphan_sec:
        return "stale_orphan", stale_orphan_sec
    return None, None


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
        "oldest_elapsed_sec": violation.oldest_elapsed_sec,
        "stale_sec": violation.stale_sec,
        "orphaned": violation.orphaned,
    }


def violation_payload(violation: SentinelViolation) -> dict[str, object]:
    return _violation_payload(violation)


def _format_violation(violation: SentinelViolation) -> str:
    peak = "-"
    if violation.peak_rss_gb is not None:
        peak = f"{violation.peak_rss_gb:.2f}GB"
    return (
        "[PROCESS-SENTINEL] "
        f"{violation.reason} pgid={violation.pgid} "
        f"total={violation.total_rss_gb:.2f}GB peak={peak} "
        f"age={violation.oldest_elapsed_sec if violation.oldest_elapsed_sec is not None else '-'}s "
        f"orphaned={violation.orphaned} "
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
        default=None,
        help="Per-process RSS ceiling (default: adaptive from live available memory).",
    )
    parser.add_argument(
        "--max-total-rss-gb",
        type=float,
        default=None,
        help=(
            "Per-process-group RSS ceiling for matched Molt groups "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--max-global-rss-gb",
        type=float,
        default=None,
        help=(
            "Cumulative RSS ceiling across all matched Molt process groups "
            "(default: adaptive from live available memory)."
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
        "--stale-orphan-sec",
        type=float,
        default=None,
        help=(
            "Terminate orphaned repo-scoped Molt process groups older than this "
            "many seconds. Defaults to off unless provided."
        ),
    )
    parser.add_argument(
        "--stale-pytest-sec",
        type=float,
        default=None,
        help=(
            "Terminate orphaned pytest-style Molt process groups older than this "
            "many seconds. Defaults to off unless provided."
        ),
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
        "--until-clean-sec",
        type=float,
        default=None,
        help=(
            "Keep scanning until no matching process group is seen for this many "
            "seconds, then exit. Useful for draining delayed stale launches."
        ),
    )
    parser.add_argument(
        "--max-runtime-sec",
        type=float,
        default=DEFAULT_MAX_RUNTIME_SEC,
        help=(
            "Maximum runtime for --until-clean-sec mode "
            f"(default: {DEFAULT_MAX_RUNTIME_SEC})."
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON lines instead of compact text.",
    )
    return parser


def _resolved_limits_from_args(
    args: argparse.Namespace,
    *,
    accounted_rss_kb: int,
) -> memory_guard.ResolvedMemoryLimits:
    initial_budget = memory_guard.adaptive_memory_budget(
        "MOLT_SENTINEL",
        accounted_rss_kb=accounted_rss_kb,
    )
    max_process_gb = (
        initial_budget.max_process_rss_gb
        if args.max_process_rss_gb is None
        else args.max_process_rss_gb
    )
    max_group_gb = (
        initial_budget.max_total_rss_gb
        if args.max_total_rss_gb is None
        else args.max_total_rss_gb
    )
    max_global_gb = (
        initial_budget.max_global_rss_gb
        if args.max_global_rss_gb is None
        else args.max_global_rss_gb
    )
    return memory_guard.resolve_memory_limits(
        max_process_rss_kb=memory_guard.max_rss_kb_from_gb(max_process_gb),
        max_total_rss_kb=memory_guard.max_rss_kb_from_gb(max_group_gb),
        max_global_rss_kb=memory_guard.max_global_rss_kb_from_gb(max_global_gb),
        adaptive_budget_provider=lambda accounted: memory_guard.adaptive_memory_budget(
            "MOLT_SENTINEL",
            accounted_rss_kb=accounted,
        ),
        dynamic_process_rss=args.max_process_rss_gb is None,
        dynamic_total_rss=args.max_total_rss_gb is None,
        dynamic_global_rss=args.max_global_rss_gb is None,
        accounted_rss_kb=accounted_rss_kb,
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        _resolved_limits_from_args(args, accounted_rss_kb=0)
        if args.poll_interval <= 0:
            raise ValueError("poll interval must be greater than 0")
        if args.grace_sec < 0:
            raise ValueError("grace period must be non-negative")
        if args.stale_orphan_sec is not None and args.stale_orphan_sec <= 0:
            raise ValueError("stale orphan seconds must be greater than 0")
        if args.stale_pytest_sec is not None and args.stale_pytest_sec <= 0:
            raise ValueError("stale pytest seconds must be greater than 0")
        if args.until_clean_sec is not None and args.until_clean_sec <= 0:
            raise ValueError("until-clean seconds must be greater than 0")
        if args.max_runtime_sec is not None and args.max_runtime_sec <= 0:
            raise ValueError("max runtime seconds must be greater than 0")
        if args.once and args.until_clean_sec is not None:
            raise ValueError("--once and --until-clean-sec are mutually exclusive")
    except ValueError as exc:
        print(f"process_sentinel: {exc}", file=sys.stderr)
        return 2

    root = Path(args.repo_root).expanduser()
    stream = sys.stdout if args.json else sys.stderr
    started = time.monotonic()
    clean_since: float | None = None
    known_pgids: set[int] = set()
    while True:
        groups = process_groups(
            memory_guard.sample_processes(),
            root=root,
            self_pid=os.getpid(),
            self_pgid=os.getpgrp(),
            known_pgids=known_pgids,
        )
        known_pgids.update(group.pgid for group in groups)
        current_limits = _resolved_limits_from_args(
            args,
            accounted_rss_kb=sum(group.total_rss_kb for group in groups),
        )
        violations = find_violations(
            groups,
            max_process_kb=current_limits.max_process_rss_kb,
            max_group_kb=current_limits.max_total_rss_kb
            if current_limits.max_total_rss_kb is not None
            else 0,
            max_global_kb=current_limits.max_global_rss_kb
            if current_limits.max_global_rss_kb is not None
            else 0,
            kill_all=args.kill_all,
            stale_orphan_sec=args.stale_orphan_sec,
            stale_pytest_sec=args.stale_pytest_sec,
        )
        emit_violations(violations, json_mode=args.json, stream=stream)
        now = time.monotonic()
        if not args.dry_run:
            for violation in violations:
                terminate_group(violation.pgid, grace=args.grace_sec)
        if args.once:
            return 1 if violations else 0
        if args.until_clean_sec is not None:
            if groups:
                clean_since = None
            else:
                if clean_since is None:
                    clean_since = now
                if now - clean_since >= args.until_clean_sec:
                    return 0
            if (
                args.max_runtime_sec is not None
                and now - started >= args.max_runtime_sec
            ):
                return 1
        time.sleep(args.poll_interval)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

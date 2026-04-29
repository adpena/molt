#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Callable, Mapping, Sequence
import contextlib
from dataclasses import dataclass
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time


DEFAULT_MAX_RSS_GB = 25.0
DEFAULT_MAX_TOTAL_RSS_GB = 28.0
DEFAULT_POLL_INTERVAL_SEC = 1.0
GUARD_RETURN_CODE = 137


@dataclass(frozen=True, slots=True)
class ProcessSample:
    pid: int
    ppid: int
    rss_kb: int
    command: str
    pgid: int | None = None


@dataclass(frozen=True, slots=True)
class RssViolation:
    pid: int
    rss_kb: int
    command: str
    scope: str = "process"

    @property
    def rss_gb(self) -> float:
        return self.rss_kb / (1024 * 1024)


@dataclass(frozen=True, slots=True)
class GuardResult:
    returncode: int
    violation: RssViolation | None
    peak: RssViolation | None
    peak_total: RssViolation | None
    stdout: str
    stderr: str


def parse_process_table(text: str) -> dict[int, ProcessSample]:
    samples: dict[int, ProcessSample] = {}
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        pid: int
        ppid: int
        rss_kb: int
        command: str
        pgid: int | None
        parts = line.split(None, 4)
        if len(parts) >= 5:
            try:
                pid = int(parts[0])
                ppid = int(parts[1])
                pgid = int(parts[2])
                rss_kb = int(parts[3])
                command = parts[4]
            except ValueError:
                legacy_parts = line.split(None, 3)
                if len(legacy_parts) < 4:
                    continue
                try:
                    pid = int(legacy_parts[0])
                    ppid = int(legacy_parts[1])
                    rss_kb = int(legacy_parts[2])
                except ValueError:
                    continue
                command = legacy_parts[3]
                pgid = None
        else:
            legacy_parts = line.split(None, 3)
            if len(legacy_parts) < 4:
                continue
            try:
                pid = int(legacy_parts[0])
                ppid = int(legacy_parts[1])
                rss_kb = int(legacy_parts[2])
            except ValueError:
                continue
            command = legacy_parts[3]
            pgid = None
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=ppid,
            rss_kb=rss_kb,
            command=command,
            pgid=pgid,
        )
    return samples


def sample_processes() -> dict[int, ProcessSample]:
    result = subprocess.run(
        ["ps", "-axo", "pid=,ppid=,pgid=,rss=,command="],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        return {}
    return parse_process_table(result.stdout)


def descendant_pids(samples: Mapping[int, ProcessSample], root_pid: int) -> set[int]:
    descendants = {root_pid}
    changed = True
    while changed:
        changed = False
        for sample in samples.values():
            if sample.pid in descendants:
                continue
            if sample.ppid in descendants:
                descendants.add(sample.pid)
                changed = True
    return descendants


def watched_pids(samples: Mapping[int, ProcessSample], root_pid: int) -> set[int]:
    watched = descendant_pids(samples, root_pid)
    for sample in samples.values():
        if sample.pgid == root_pid:
            watched.add(sample.pid)
    return watched


def peak_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
) -> RssViolation | None:
    watched = watched_pids(samples, root_pid)
    candidates = [sample for pid, sample in samples.items() if pid in watched]
    if not candidates:
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )


def total_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
) -> RssViolation | None:
    watched = watched_pids(samples, root_pid)
    candidates = [sample for pid, sample in samples.items() if pid in watched]
    if not candidates:
        return None
    return RssViolation(
        pid=root_pid,
        rss_kb=sum(sample.rss_kb for sample in candidates),
        command="process tree aggregate",
        scope="process_tree",
    )


def find_rss_violation(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
) -> RssViolation | None:
    watched = watched_pids(samples, root_pid)
    candidates = [
        sample
        for pid, sample in samples.items()
        if pid in watched and sample.rss_kb > max_rss_kb
    ]
    if not candidates:
        if max_total_rss_kb is None:
            return None
        aggregate = total_rss(samples, root_pid=root_pid)
        if aggregate is not None and aggregate.rss_kb > max_total_rss_kb:
            return aggregate
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )


def max_rss_kb_from_gb(value: float) -> int:
    if value <= 0:
        raise ValueError("max RSS must be greater than 0 GB")
    if value >= 30:
        raise ValueError("max RSS must stay below 30 GB")
    return int(value * 1024 * 1024)


def _terminate_process_group(pid: int) -> None:
    try:
        os.killpg(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    except OSError:
        with contextlib.suppress(ProcessLookupError):
            os.kill(pid, signal.SIGTERM)
        return
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        try:
            os.killpg(pid, 0)
        except ProcessLookupError:
            return
        except OSError:
            return
        time.sleep(0.05)
    try:
        os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


def run_guarded(
    command: Sequence[str],
    *,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
    poll_interval: float,
    sampler: Callable[[], Mapping[int, ProcessSample]] = sample_processes,
    capture_output: bool = True,
) -> GuardResult:
    if not command:
        raise ValueError("command is required")
    if poll_interval <= 0:
        raise ValueError("poll interval must be greater than 0")
    proc = subprocess.Popen(
        list(command),
        stdout=subprocess.PIPE if capture_output else None,
        stderr=subprocess.PIPE if capture_output else None,
        text=True,
        start_new_session=True,
    )
    violation: RssViolation | None = None
    peak: RssViolation | None = None
    peak_total: RssViolation | None = None
    while True:
        samples = sampler()
        observed_peak = peak_rss(samples, root_pid=proc.pid)
        if observed_peak is not None and (
            peak is None or observed_peak.rss_kb > peak.rss_kb
        ):
            peak = observed_peak
        observed_total = total_rss(samples, root_pid=proc.pid)
        if observed_total is not None and (
            peak_total is None or observed_total.rss_kb > peak_total.rss_kb
        ):
            peak_total = observed_total
        violation = find_rss_violation(
            samples,
            root_pid=proc.pid,
            max_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
        )
        if violation is not None:
            _terminate_process_group(proc.pid)
            break
        if proc.poll() is not None:
            break
        time.sleep(poll_interval)
    stdout, stderr = proc.communicate()
    returncode = proc.returncode
    if violation is not None:
        returncode = GUARD_RETURN_CODE
    return GuardResult(
        returncode=returncode,
        violation=violation,
        peak=peak,
        peak_total=peak_total,
        stdout=stdout or "",
        stderr=stderr or "",
    )


def _rss_record_payload(record: RssViolation | None) -> dict[str, object] | None:
    if record is None:
        return None
    return {
        "pid": record.pid,
        "rss_kb": record.rss_kb,
        "rss_gb": record.rss_gb,
        "command": record.command,
        "scope": record.scope,
    }


def _write_summary_json(
    path: str,
    *,
    command: Sequence[str],
    max_rss_kb: int,
    max_total_rss_kb: int | None,
    result: GuardResult,
) -> None:
    summary_path = Path(path)
    if summary_path.parent:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "command": list(command),
        "returncode": result.returncode,
        "max_rss_kb": max_rss_kb,
        "max_rss_gb": max_rss_kb / (1024 * 1024),
        "max_total_rss_kb": max_total_rss_kb,
        "max_total_rss_gb": (
            None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
        ),
        "violation": _rss_record_payload(result.violation),
        "peak": _rss_record_payload(result.peak),
        "peak_total": _rss_record_payload(result.peak_total),
    }
    summary_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Run a command with a process-tree/process-group RSS ceiling."
    )
    parser.add_argument(
        "--max-rss-gb",
        type=float,
        default=DEFAULT_MAX_RSS_GB,
        help=(
            "Abort if any child process exceeds this RSS; must be <30 "
            f"(default: {DEFAULT_MAX_RSS_GB})."
        ),
    )
    parser.add_argument(
        "--max-total-rss-gb",
        type=float,
        default=DEFAULT_MAX_TOTAL_RSS_GB,
        help=(
            "Abort if the watched process tree exceeds this aggregate RSS; "
            "must be <30 "
            f"(default: {DEFAULT_MAX_TOTAL_RSS_GB})."
        ),
    )
    parser.add_argument(
        "--poll-interval",
        type=float,
        default=DEFAULT_POLL_INTERVAL_SEC,
        help=(
            "Process sampling interval in seconds "
            f"(default: {DEFAULT_POLL_INTERVAL_SEC})."
        ),
    )
    parser.add_argument(
        "--summary-json",
        help="Write command result, violation, and peak RSS details as JSON.",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        print("memory_guard: command is required", file=sys.stderr)
        return 2
    try:
        max_rss_kb = max_rss_kb_from_gb(args.max_rss_gb)
        max_total_rss_kb = max_rss_kb_from_gb(args.max_total_rss_gb)
        poll_interval = float(args.poll_interval)
        if poll_interval <= 0:
            raise ValueError("poll interval must be greater than 0")
    except ValueError as exc:
        print(f"memory_guard: {exc}", file=sys.stderr)
        return 2
    result = run_guarded(
        command,
        max_rss_kb=max_rss_kb,
        max_total_rss_kb=max_total_rss_kb,
        poll_interval=poll_interval,
        capture_output=False,
    )
    if args.summary_json:
        try:
            _write_summary_json(
                args.summary_json,
                command=command,
                max_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                result=result,
            )
        except OSError as exc:
            print(f"memory_guard: failed to write summary JSON: {exc}", file=sys.stderr)
            return 2 if result.returncode == 0 else result.returncode
    if result.violation is not None:
        limit_gb = (
            args.max_total_rss_gb
            if result.violation.scope == "process_tree"
            else args.max_rss_gb
        )
        print(
            "memory_guard: RSS limit exceeded: "
            f"pid={result.violation.pid} "
            f"rss={result.violation.rss_gb:.2f}GB "
            f"limit={limit_gb:.2f}GB "
            f"scope={result.violation.scope} "
            f"command={result.violation.command}",
            file=sys.stderr,
        )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Mapping, Sequence
import contextlib
from dataclasses import dataclass
from datetime import UTC, datetime
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

REPO_SCOPED_MOLT_ARTIFACT_ROOT_TOKENS = tuple(
    dict.fromkeys(
        (
            "/.molt_cache/home/bin/",
            "/target/debug/",
            "/target/dev-fast/",
            "/target/release-fast/",
            "/tmp/",
            "/dist/",
            "/build/",
            "/wasm/",
            "/bench/results/",
        )
    )
)

REPO_SCOPED_MOLT_ENTRYPOINT_TOKENS = tuple(
    dict.fromkeys(
        (
            "/tests/molt_diff.py",
            *GUARDED_ENTRYPOINT_TOKENS,
        )
    )
)

REPO_SCOPED_MOLT_ARTIFACT_IDENTITY_TOKENS = (
    "_molt",
    "-molt",
    "/molt",
    "\\molt",
    "molt_",
    "molt-",
    "molt.",
    "molt-backend",
    "molt_runtime",
    "bench_exception_heavy",
    "exception-repro",
)

PYTEST_PROCESS_TOKENS = (
    " pytest",
    "/pytest",
    "python -m pytest",
    "python3 -m pytest",
)

HOST_CONTROL_PLANE_TOKENS = memory_guard.HOST_CONTROL_PLANE_TOKENS

INSPECTION_COMMAND_TOKENS = (
    "tools/process_sentinel.py",
    "process_sentinel.py",
    "tools/memory_guard_core/windows_snapshot.py",
    "--molt-windows-process-snapshot-json",
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
    samples: tuple[memory_guard.ProcessSample, ...] = ()
    external_parent_pids: tuple[int, ...] = ()
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


def _normalized_path_text(path: str) -> str:
    normalized = path.replace("\\", "/").rstrip("/")
    if normalized.startswith("//?/"):
        normalized = normalized[4:]
    return normalized


def _ordered_unique(items: Sequence[str]) -> tuple[str, ...]:
    seen: set[str] = set()
    result: list[str] = []
    for item in items:
        if not item or item in seen:
            continue
        seen.add(item)
        result.append(item)
    return tuple(result)


def _normalized_repo_tokens(root: Path) -> tuple[str, ...]:
    candidates = [
        _normalized_path_text(root.as_posix()),
        _normalized_path_text(str(root)),
    ]
    with contextlib.suppress(OSError, RuntimeError):
        resolved = root.resolve(strict=False)
        candidates.extend(
            [
                _normalized_path_text(resolved.as_posix()),
                _normalized_path_text(str(resolved)),
            ]
        )
    drive_relative: list[str] = []
    for candidate in candidates:
        if len(candidate) > 2 and candidate[1] == ":" and candidate[2] == "/":
            drive_relative.append(candidate[2:])
    return _ordered_unique([*candidates, *drive_relative])


def _command_contains(command: str, token: str) -> bool:
    if _is_windows_process_model():
        return token.casefold() in command.casefold()
    return token in command


def _repo_scoped_command_match(command: str, root: Path) -> bool:
    normalized_command = _normalized_path_text(command)
    entrypoint_tokens = tuple(
        _normalized_path_text(token) for token in REPO_SCOPED_MOLT_ENTRYPOINT_TOKENS
    )
    artifact_root_tokens = tuple(
        _normalized_path_text(token)
        for token in REPO_SCOPED_MOLT_ARTIFACT_ROOT_TOKENS
    )
    for repo_token in _normalized_repo_tokens(root):
        if not _command_contains(normalized_command, repo_token):
            continue
        if any(
            _command_contains(normalized_command, f"{repo_token}{token}")
            for token in entrypoint_tokens
        ):
            return True
        for token in artifact_root_tokens:
            anchor = f"{repo_token}{token}"
            if not _command_contains(normalized_command, anchor):
                continue
            tail_start = normalized_command.casefold().find(anchor.casefold())
            if tail_start < 0:
                continue
            tail = normalized_command[tail_start + len(anchor) :]
            if any(
                _command_contains(tail, identity_token)
                for identity_token in REPO_SCOPED_MOLT_ARTIFACT_IDENTITY_TOKENS
            ):
                return True
    return False


def _sample_pgid(sample: memory_guard.ProcessSample) -> int:
    return sample.pgid if sample.pgid is not None else sample.pid


def is_host_control_plane_process(sample: memory_guard.ProcessSample) -> bool:
    return memory_guard.is_host_control_plane_process(sample)


def protected_process_group_ids(
    samples: Mapping[int, memory_guard.ProcessSample],
    *,
    self_pid: int | None = None,
    self_pgid: int | None = None,
) -> set[int]:
    return memory_guard.protected_process_group_ids(
        samples,
        self_pid=self_pid,
        self_pgid=self_pgid,
    )


def _safe_getpgrp() -> int | None:
    return memory_guard._safe_getpgrp()


def _is_windows_process_model() -> bool:
    return os.name == "nt"


def _group_samples_by_pgid(
    samples: Mapping[int, memory_guard.ProcessSample],
) -> dict[int, list[memory_guard.ProcessSample]]:
    grouped: dict[int, list[memory_guard.ProcessSample]] = {}
    for sample in samples.values():
        grouped.setdefault(_sample_pgid(sample), []).append(sample)
    return grouped


def _owned_process_ids(
    samples: Mapping[int, memory_guard.ProcessSample],
    *,
    root: Path,
    self_pid: int | None,
    known_process_identities: Mapping[int, memory_guard.ProcessIdentity] | None,
) -> set[int]:
    owned: set[int] = set()
    if known_process_identities is not None:
        for pid, identity in known_process_identities.items():
            sample = samples.get(pid)
            if sample is not None and memory_guard.process_identity(sample) == identity:
                owned.add(pid)
    for sample in samples.values():
        if is_molt_process(sample, root=root, self_pid=self_pid):
            owned.add(sample.pid)
    changed = True
    while changed:
        changed = False
        for sample in samples.values():
            if sample.pid not in owned and sample.ppid in owned:
                owned.add(sample.pid)
                changed = True
    return owned


def _windows_snapshot_helper_tree_ids(
    samples: Mapping[int, memory_guard.ProcessSample],
) -> set[int]:
    helper_pids = {
        sample.pid
        for sample in samples.values()
        if _command_contains(
            _normalized_path_text(sample.command),
            _normalized_path_text("--molt-windows-process-snapshot-json"),
        )
    }
    if not helper_pids:
        return set()
    blocked = set(helper_pids)
    changed = True
    while changed:
        changed = False
        for sample in samples.values():
            if sample.pid not in blocked and sample.ppid in blocked:
                blocked.add(sample.pid)
                changed = True
    return blocked


def _candidate_process_group_ids(
    samples: Mapping[int, memory_guard.ProcessSample],
    owned_pids: set[int],
) -> set[int]:
    return {
        _sample_pgid(sample) for sample in samples.values() if sample.pid in owned_pids
    }


def _group_is_fully_owned(
    group: Sequence[memory_guard.ProcessSample],
    owned_pids: set[int],
) -> bool:
    return bool(group) and all(sample.pid in owned_pids for sample in group)


def is_molt_process(
    sample: memory_guard.ProcessSample,
    *,
    root: Path,
    self_pid: int | None = None,
) -> bool:
    if self_pid is not None and sample.pid == self_pid:
        return False
    command = sample.command
    if is_host_control_plane_process(sample):
        return False
    normalized_command = _normalized_path_text(command)
    if any(
        _command_contains(normalized_command, _normalized_path_text(token))
        for token in INSPECTION_COMMAND_TOKENS
    ):
        return False
    if _repo_scoped_command_match(command, root):
        return True
    return any(
        _command_contains(normalized_command, repo_token)
        for repo_token in _normalized_repo_tokens(root)
    ) and any(
        _command_contains(normalized_command, token) for token in MOLT_PROCESS_TOKENS
    )


def process_groups(
    samples: Mapping[int, memory_guard.ProcessSample],
    *,
    root: Path,
    self_pid: int | None = None,
    self_pgid: int | None = None,
    known_process_identities: Mapping[int, memory_guard.ProcessIdentity] | None = None,
    owned_pids: set[int] | None = None,
) -> list[ProcessGroup]:
    grouped = _group_samples_by_pgid(samples)
    protected_pgids = protected_process_group_ids(
        samples,
        self_pid=self_pid,
        self_pgid=self_pgid,
    )
    owned = (
        set(owned_pids)
        if owned_pids is not None
        else _owned_process_ids(
            samples,
            root=root,
            self_pid=self_pid,
            known_process_identities=known_process_identities,
        )
    )
    owned.difference_update(_windows_snapshot_helper_tree_ids(samples))
    matched = _candidate_process_group_ids(samples, owned)
    groups = [
        ProcessGroup(
            pgid=pgid,
            samples=tuple(sorted(group, key=lambda item: item.pid)),
            matched=pgid in matched,
        )
        for pgid, group in grouped.items()
        if pgid in matched
        and pgid not in protected_pgids
        and _group_is_fully_owned(group, owned)
    ]
    return sorted(groups, key=lambda group: group.pgid)


def skipped_protected_process_groups(
    samples: Mapping[int, memory_guard.ProcessSample],
    *,
    root: Path,
    self_pid: int | None = None,
    self_pgid: int | None = None,
    observed_pgids: set[int] | None = None,
    known_process_identities: Mapping[int, memory_guard.ProcessIdentity] | None = None,
) -> list[ProcessGroup]:
    grouped = _group_samples_by_pgid(samples)
    protected_pgids = protected_process_group_ids(
        samples,
        self_pid=self_pid,
        self_pgid=self_pgid,
    )
    owned = _owned_process_ids(
        samples,
        root=root,
        self_pid=self_pid,
        known_process_identities=known_process_identities,
    )
    matched = _candidate_process_group_ids(samples, owned)
    if observed_pgids:
        matched.update(observed_pgids)
    return sorted(
        (
            ProcessGroup(
                pgid=pgid,
                samples=tuple(sorted(group, key=lambda item: item.pid)),
                matched=False,
            )
            for pgid, group in grouped.items()
            if pgid in matched and pgid in protected_pgids
        ),
        key=lambda group: group.pgid,
    )


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
                samples=group.samples,
                external_parent_pids=tuple(group.external_parent_pids),
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
        "process_samples": [
            memory_guard.process_sample_payload(sample) for sample in violation.samples
        ],
        "external_parent_pids": list(violation.external_parent_pids),
        "oldest_elapsed_sec": violation.oldest_elapsed_sec,
        "stale_sec": violation.stale_sec,
        "orphaned": violation.orphaned,
    }


def violation_payload(violation: SentinelViolation) -> dict[str, object]:
    return _violation_payload(violation)


def _utc_timestamp() -> str:
    return datetime.now(UTC).isoformat(timespec="seconds").replace("+00:00", "Z")


def _elapsed_text(elapsed_s: float | None) -> str:
    return "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"


def _next_action_for_violation(violation: SentinelViolation) -> str:
    if violation.reason in {"process_rss", "group_rss", "global_rss"}:
        return (
            "inspect allocations and guard telemetry; reduce parallelism only as "
            "containment, or raise the explicit RSS budget if the workload is "
            "known-good and policy allows it"
        )
    if violation.reason in {"stale_orphan", "stale_pytest_orphan"}:
        return (
            "inspect the recorded command and parent lifecycle; make the owning "
            "harness shut down children explicitly or run it inside a suite-level "
            "repo sentinel"
        )
    if violation.reason == "kill_all":
        return (
            "rerun the interrupted build/test/bench command from a clean process "
            "state after checking logs for the terminated group"
        )
    return "inspect the process command, logs, and guard budgets before rerunning"


def _process_sentinel_repro_payload(
    violation: SentinelViolation,
    *,
    root: Path,
    args: argparse.Namespace,
    raw_argv: Sequence[str],
    limits: memory_guard.ResolvedMemoryLimits,
) -> dict[str, object]:
    command = [violation.command] if violation.command else list(sys.argv)
    timeout_s = None if args.once else args.max_runtime_sec
    payload = memory_guard.repro_context_payload(
        command=command,
        cwd=root,
        environ=os.environ,
        max_process_rss_kb=limits.max_process_rss_kb,
        max_total_rss_kb=limits.max_total_rss_kb,
        max_global_rss_kb=limits.max_global_rss_kb,
        child_rlimit_kb=None,
        timeout_s=timeout_s,
        poll_interval_s=args.poll_interval,
        summary_json=None,
    )
    payload["sentinel"] = {
        "argv": [sys.executable, str(_THIS_FILE), *raw_argv],
        "repo_root": str(root.resolve(strict=False)),
        "dry_run": bool(args.dry_run),
        "kill_all": bool(args.kill_all),
        "once": bool(args.once),
        "until_clean_sec": args.until_clean_sec,
        "max_runtime_sec": args.max_runtime_sec,
        "grace_sec": args.grace_sec,
        "stale_orphan_sec": args.stale_orphan_sec,
        "stale_pytest_sec": args.stale_pytest_sec,
    }
    return payload


def _incident_payload(
    violation: SentinelViolation,
    *,
    incident_at: str,
    elapsed_s: float | None,
    dry_run: bool,
    grace_sec: float,
    repro: dict[str, object] | None = None,
) -> dict[str, object]:
    timestamp_key = "observed_at" if dry_run else "killed_at"
    payload: dict[str, object] = {
        "event": "process_sentinel_violation",
        "action": "dry_run" if dry_run else "terminate",
        timestamp_key: incident_at,
        "elapsed_s": elapsed_s,
        "grace_sec": grace_sec,
        "kill_scope": "repo",
        "killer_label": "tools/process_sentinel.py",
        "killer_pid": os.getpid(),
        "killer_session_id": os.environ.get("MOLT_SESSION_ID", ""),
        "victim_pgid": violation.pgid,
        "victim_command": violation.command,
        "owner_match_reason": "repo_scope",
        "termination": {
            "signal": memory_guard.term_signal_payload(),
            "fallback_signal": memory_guard.fallback_kill_signal_payload(),
            "grace_sec": grace_sec,
            "rss_triggered": violation.reason
            in {"process_rss", "group_rss", "global_rss"},
        },
        "next_action": _next_action_for_violation(violation),
        "violation": _violation_payload(violation),
    }
    if repro is not None:
        payload["repro"] = repro
    return payload


def _format_violation(
    violation: SentinelViolation,
    *,
    incident_at: str,
    elapsed_s: float | None,
    dry_run: bool,
    grace_sec: float,
) -> str:
    peak = "-"
    if violation.peak_rss_gb is not None:
        peak = f"{violation.peak_rss_gb:.2f}GB"
    action = "dry_run" if dry_run else "terminate"
    timestamp_label = "observed_at" if dry_run else "killed_at"
    lines = [
        "[PROCESS-SENTINEL] "
        f"action={action} "
        f"{violation.reason} pgid={violation.pgid} "
        f"total={violation.total_rss_gb:.2f}GB peak={peak} "
        f"age={violation.oldest_elapsed_sec if violation.oldest_elapsed_sec is not None else '-'}s "
        f"orphaned={violation.orphaned} "
        f"pids={list(violation.pids)} {timestamp_label}={incident_at} "
        f"elapsed={_elapsed_text(elapsed_s)} grace={grace_sec:.2f}s "
        f"command={violation.command}",
        f"[PROCESS-SENTINEL] next action: {_next_action_for_violation(violation)}",
    ]
    return "\n".join(lines)


def emit_violations(
    violations: Sequence[SentinelViolation],
    *,
    json_mode: bool,
    stream,
    incident_at: str,
    elapsed_s: float | None,
    dry_run: bool,
    grace_sec: float,
    repro_payloads: Mapping[int, dict[str, object]] | None = None,
) -> None:
    for violation in violations:
        if json_mode:
            print(
                json.dumps(
                    _incident_payload(
                        violation,
                        incident_at=incident_at,
                        elapsed_s=elapsed_s,
                        dry_run=dry_run,
                        grace_sec=grace_sec,
                        repro=None
                        if repro_payloads is None
                        else repro_payloads.get(violation.pgid),
                    ),
                    sort_keys=True,
                ),
                file=stream,
            )
        else:
            print(
                _format_violation(
                    violation,
                    incident_at=incident_at,
                    elapsed_s=elapsed_s,
                    dry_run=dry_run,
                    grace_sec=grace_sec,
                ),
                file=stream,
            )
    with contextlib.suppress(Exception):
        stream.flush()


def _windows_group_expected_identity(
    violation: SentinelViolation,
) -> memory_guard.ProcessIdentity | None:
    if not _is_windows_process_model():
        return None
    for sample in violation.samples:
        if _sample_pgid(sample) == violation.pgid:
            return memory_guard.process_identity(sample)
    return None


def process_group_expected_identities(
    violation: SentinelViolation,
) -> dict[int, memory_guard.ProcessIdentity]:
    return {
        sample.pid: memory_guard.process_identity(sample)
        for sample in violation.samples
    }


def sample_processes_for_sentinel() -> dict[int, memory_guard.ProcessSample]:
    if _is_windows_process_model():
        return memory_guard.sample_processes_windows_hard_timeout()
    return memory_guard.sample_processes()


def terminate_group(
    pgid: int,
    *,
    grace: float,
    root: Path | None = None,
    expected_identity: memory_guard.ProcessIdentity | None = None,
    expected_identities: Mapping[int, memory_guard.ProcessIdentity] | None = None,
) -> None:
    self_pgid = _safe_getpgrp()
    if pgid <= 0 or (self_pgid is not None and pgid == self_pgid):
        return
    root = repo_root() if root is None else root
    samples = sample_processes_for_sentinel()
    protected_pgids = protected_process_group_ids(
        samples,
        self_pid=os.getpid(),
        self_pgid=self_pgid,
    )
    if pgid in protected_pgids:
        return
    if _is_windows_process_model():
        sample = samples.get(pgid)
        if sample is None:
            return
        expected_pid_identity = (
            expected_identities.get(sample.pid)
            if expected_identities is not None
            else expected_identity
        )
        if expected_pid_identity is None:
            expected_pid_identity = memory_guard.process_identity(sample)
        if memory_guard.process_identity(sample) != expected_pid_identity:
            return
        if not is_molt_process(sample, root=root, self_pid=os.getpid()):
            return
        action = memory_guard._send_pid_signal_if_identity_action(
            pgid,
            expected_pid_identity,
            signal.SIGTERM,
            sampler=sample_processes_for_sentinel,
        )
        if action.result != "sent":
            return
        time.sleep(max(0.0, grace))
        memory_guard._send_pid_signal_if_identity_action(
            pgid,
            expected_pid_identity,
            memory_guard.fallback_kill_signal(),
            sampler=sample_processes_for_sentinel,
        )
        return
    if expected_identities is not None:
        action = memory_guard._send_process_group_signal_if_identities_match_action(
            pgid,
            expected_identities,
            signal.SIGTERM,
            sampler=sample_processes_for_sentinel,
        )
        if action.result != "sent":
            return
    else:
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(pgid, signal.SIGTERM)
    deadline = time.monotonic() + max(0.0, grace)
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except (ProcessLookupError, PermissionError):
            return
        time.sleep(0.05)
    samples = sample_processes_for_sentinel()
    protected_pgids = protected_process_group_ids(
        samples,
        self_pid=os.getpid(),
        self_pgid=_safe_getpgrp(),
    )
    if pgid in protected_pgids:
        return
    if expected_identities is not None:
        memory_guard._send_process_group_signal_if_identities_match_action(
            pgid,
            expected_identities,
            memory_guard.fallback_kill_signal(),
            sampler=sample_processes_for_sentinel,
        )
    else:
        with contextlib.suppress(ProcessLookupError, PermissionError):
            os.killpg(pgid, memory_guard.fallback_kill_signal())


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
        "--max-tree-rss-gb",
        "--max-group-rss-gb",
        dest="max_total_rss_gb",
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


def _validate_explicit_memory_limit(
    value: float | None,
    *,
    label: str,
    hard_limit_gb: float,
) -> None:
    if value is not None and value >= hard_limit_gb:
        raise ValueError(f"{label} RSS must stay below {hard_limit_gb:g} GB")


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
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    args = parser.parse_args(argv)
    try:
        _validate_explicit_memory_limit(
            args.max_process_rss_gb,
            label="max process",
            hard_limit_gb=memory_guard.DEFAULT_HARD_MAX_RSS_GB,
        )
        _validate_explicit_memory_limit(
            args.max_total_rss_gb,
            label="max group",
            hard_limit_gb=memory_guard.DEFAULT_HARD_MAX_RSS_GB,
        )
        _validate_explicit_memory_limit(
            args.max_global_rss_gb,
            label="max global",
            hard_limit_gb=memory_guard.DEFAULT_HARD_MAX_GLOBAL_RSS_GB,
        )
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
    known_process_identities: dict[int, memory_guard.ProcessIdentity] = {}

    def scan_groups() -> list[ProcessGroup]:
        groups = process_groups(
            sample_processes_for_sentinel(),
            root=root,
            self_pid=os.getpid(),
            self_pgid=_safe_getpgrp(),
            known_process_identities=known_process_identities,
        )
        for group in groups:
            for sample in group.samples:
                known_process_identities[sample.pid] = memory_guard.process_identity(
                    sample
                )
        return groups

    def process_observed_groups(
        groups: Sequence[ProcessGroup],
        *,
        observed_at: float,
    ) -> list[SentinelViolation]:
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
        repro_payloads = (
            {
                violation.pgid: _process_sentinel_repro_payload(
                    violation,
                    root=root,
                    args=args,
                    raw_argv=raw_argv,
                    limits=current_limits,
                )
                for violation in violations
            }
            if args.json and violations
            else None
        )
        emit_violations(
            violations,
            json_mode=args.json,
            stream=stream,
            incident_at=_utc_timestamp(),
            elapsed_s=observed_at - started,
            dry_run=args.dry_run,
            grace_sec=args.grace_sec,
            repro_payloads=repro_payloads,
        )
        if not args.dry_run:
            for violation in violations:
                terminate_group(
                    violation.pgid,
                    grace=args.grace_sec,
                    root=root,
                    expected_identity=_windows_group_expected_identity(violation),
                    expected_identities=process_group_expected_identities(violation),
                )
        return violations

    while True:
        groups = scan_groups()
        now = time.monotonic()
        violations = process_observed_groups(groups, observed_at=now)
        if args.once:
            return 1 if violations else 0
        if args.until_clean_sec is not None:
            if groups:
                clean_since = None
            else:
                if clean_since is None:
                    clean_since = now
                if now - clean_since >= args.until_clean_sec:
                    final_groups = scan_groups()
                    if final_groups:
                        final_now = time.monotonic()
                        process_observed_groups(final_groups, observed_at=final_now)
                        clean_since = None
                    else:
                        return 0
            if (
                args.max_runtime_sec is not None
                and now - started >= args.max_runtime_sec
            ):
                return 1
        time.sleep(args.poll_interval)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

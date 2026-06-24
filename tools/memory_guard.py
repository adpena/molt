#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Callable, Mapping, Sequence
import contextlib
from dataclasses import dataclass
import json
import os
import platform
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import threading
import time
from typing import Any


DEFAULT_POLL_INTERVAL_SEC = 0.10
DEFAULT_FAST_START_POLL_INTERVAL_SEC = 0.02
DEFAULT_FAST_START_DURATION_SEC = 2.0
DEFAULT_SAMPLES_MAX_MB = 2.0
DEFAULT_TERMINATION_WAIT_SEC = 2.0
DEFAULT_INCIDENT_SUMMARY_KEEP = 32
ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.memory_guard_core.common import (  # noqa: E402
    utc_compact_timestamp as _utc_compact_timestamp,
    utc_timestamp as _utc_timestamp,
)
from tools.memory_guard_core.memory_limits import (  # noqa: E402
    DEFAULT_GLOBAL_FRACTION_OF_USABLE as DEFAULT_GLOBAL_FRACTION_OF_USABLE,
    DEFAULT_HARD_MAX_CHILD_RLIMIT_GB as DEFAULT_HARD_MAX_CHILD_RLIMIT_GB,
    DEFAULT_HARD_MAX_GLOBAL_RSS_GB as DEFAULT_HARD_MAX_GLOBAL_RSS_GB,
    DEFAULT_HARD_MAX_RSS_GB as DEFAULT_HARD_MAX_RSS_GB,
    DEFAULT_MAX_GLOBAL_RSS_GB as DEFAULT_MAX_GLOBAL_RSS_GB,
    DEFAULT_MAX_RSS_GB as DEFAULT_MAX_RSS_GB,
    DEFAULT_MAX_TOTAL_RSS_GB as DEFAULT_MAX_TOTAL_RSS_GB,
    DEFAULT_MEMORY_RESERVE_FRACTION as DEFAULT_MEMORY_RESERVE_FRACTION,
    DEFAULT_MEMORY_RESERVE_MAX_GB as DEFAULT_MEMORY_RESERVE_MAX_GB,
    DEFAULT_MEMORY_RESERVE_MIN_GB as DEFAULT_MEMORY_RESERVE_MIN_GB,
    DEFAULT_PROCESS_FRACTION_OF_TOTAL as DEFAULT_PROCESS_FRACTION_OF_TOTAL,
    DEFAULT_TOTAL_FRACTION_OF_GLOBAL as DEFAULT_TOTAL_FRACTION_OF_GLOBAL,
    AdaptiveMemoryBudget as AdaptiveMemoryBudget,
    ResolvedMemoryLimits as ResolvedMemoryLimits,
    _darwin_available_memory_bytes as _darwin_available_memory_bytes,
    _darwin_physical_memory_bytes as _darwin_physical_memory_bytes,
    _float_env as _float_env,
    _gb_from_bytes as _gb_from_bytes,
    _linux_meminfo_bytes as _linux_meminfo_bytes,
    _normalize_env_prefix as _normalize_env_prefix,
    _parse_darwin_vm_stat_available_bytes as _parse_darwin_vm_stat_available_bytes,
    _prefixed_names as _prefixed_names,
    adaptive_memory_budget as adaptive_memory_budget,
    available_memory_bytes as available_memory_bytes,
    child_rlimit_kb_from_gb as child_rlimit_kb_from_gb,
    default_child_rlimit_gb as default_child_rlimit_gb,
    max_global_rss_kb_from_gb as max_global_rss_kb_from_gb,
    max_rss_kb_from_gb as max_rss_kb_from_gb,
    physical_memory_bytes as physical_memory_bytes,
    resolve_memory_limits as resolve_memory_limits,
)
from tools.memory_guard_core.payloads import (  # noqa: E402
    _rss_record_payload as _rss_record_payload,
    guarded_child_process_payload as guarded_child_process_payload,
    memory_limits_payload as memory_limits_payload,
    termination_action_payload as termination_action_payload,
    termination_report_payload as termination_report_payload,
    termination_reports_payload as termination_reports_payload,
)
from tools.memory_guard_core.cargo_quarantine import (  # noqa: E402
    DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP as DEFAULT_CARGO_INCREMENTAL_QUARANTINE_KEEP,
    CargoIncrementalQuarantine as CargoIncrementalQuarantine,
    CargoIncrementalQuarantineMove as CargoIncrementalQuarantineMove,
    _cargo_incremental_dirs as _cargo_incremental_dirs,
    _cargo_incremental_quarantine_message as _cargo_incremental_quarantine_message,
    _cargo_incremental_quarantine_payload as _cargo_incremental_quarantine_payload,
    _cargo_quarantine_id as _cargo_quarantine_id,
    _cargo_quarantine_parent as _cargo_quarantine_parent,
    _cargo_quarantine_payload_required as _cargo_quarantine_payload_required,
    _cargo_target_dir as _cargo_target_dir,
    _command_invokes_cargo_build_state as _command_invokes_cargo_build_state,
    _command_tokens as _command_tokens,
    _effective_guard_cwd as _effective_guard_cwd,
    _prune_cargo_incremental_quarantine as _prune_cargo_incremental_quarantine,
    _quarantine_cargo_incremental_state as _quarantine_cargo_incremental_state,
    _samples_include_cargo_build_state as _samples_include_cargo_build_state,
    _token_executable_name as _token_executable_name,
    _write_cargo_quarantine_receipt as _write_cargo_quarantine_receipt,
)
from tools.memory_guard_core.windows_snapshot import (  # noqa: E402
    WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES as WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES,
    _filetime_to_unix_seconds as _filetime_to_unix_seconds,
    _windows_process_needs_full_command_line as _windows_process_needs_full_command_line,
    _windows_process_snapshot_rows as _windows_process_snapshot_rows,
)
PYTEST_OUTER_GUARD_SUMMARY_DIR = ROOT / "tmp" / "pytest-memory-guard"
GUARD_RETURN_CODE = 137
TIMEOUT_RETURN_CODE = 124
INTERNAL_COMMAND_ENV = "MOLT_MEMORY_GUARD_COMMAND_JSON"
INTERNAL_WORKER_ENV = "MOLT_MEMORY_GUARD_INTERNAL"
INTERNAL_CHILD_RUNNER_ENV = "MOLT_MEMORY_GUARD_CHILD_RUNNER"
INTERNAL_CHILD_COMMAND_ENV = "MOLT_MEMORY_GUARD_CHILD_COMMAND_JSON"
INTERNAL_CHILD_RLIMIT_KB_ENV = "MOLT_MEMORY_GUARD_CHILD_RLIMIT_KB"
INTERNAL_CHILD_STARTED_FD_ENV = "MOLT_MEMORY_GUARD_CHILD_STARTED_FD"
ACTIVE_ENV = "MOLT_MEMORY_GUARD_ACTIVE"
ACTIVE_GUARD_PID_ENV = "MOLT_MEMORY_GUARD_PID"
ACTIVE_GUARD_TOKEN_ENV = "MOLT_MEMORY_GUARD_TOKEN"
ACTIVE_GUARD_MARKER_ENV = "MOLT_MEMORY_GUARD_MARKER"
ACTIVE_GUARD_MARKER_DIR = ROOT / "tmp" / "memory_guard" / "active"
ACTIVE_GUARD_MARKER_KEEP = 128
_INTERNAL_ENV_KEYS = (
    INTERNAL_COMMAND_ENV,
    INTERNAL_WORKER_ENV,
    INTERNAL_CHILD_RUNNER_ENV,
    INTERNAL_CHILD_COMMAND_ENV,
    INTERNAL_CHILD_RLIMIT_KB_ENV,
    INTERNAL_CHILD_STARTED_FD_ENV,
)
HOST_CONTROL_PLANE_TOKENS = (
    "/Applications/Codex.app/",
    "Codex.app/Contents/",
    "Codex (Renderer)",
    "Codex Helper",
    "OpenAI.Codex_",
    "\\app\\Codex.exe",
    "\\app\\resources\\codex.exe",
    "codex.exe\" app-server",
    "codex app-server",
    "codex_chronicle",
    "/cua_node/bin/node_repl",
    "\\runtimes\\cua_node\\",
    "node_repl.exe",
    "/Applications/Claude.app/",
    "claude --",
    "\\claude.exe",
    "\\claude.cmd",
    "\\claude-code.exe",
    "\\node_modules\\@anthropic-ai\\claude-code\\",
    "Claude.app/Contents/",
    "/.claude/",
    "@anthropic-ai/claude-code",
    "CLAUDE_PLUGIN_DATA=",
)
HOST_CONTROL_PLANE_EXECUTABLE_NAMES = frozenset(
    {
        "claude",
        "claude-code",
        "claude-code.exe",
        "claude.cmd",
        "claude.exe",
        "codex.exe",
        "node_repl.exe",
    }
)


def termination_wait_seconds(env: Mapping[str, str] | None = None) -> float:
    source = os.environ if env is None else env
    for name in (
        "MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC",
        "MOLT_MEMORY_GUARD_TERMINATE_WAIT_SEC",
    ):
        raw = source.get(name, "").strip()
        if not raw:
            continue
        lowered = raw.lower()
        if lowered in {"0", "false", "off", "no"}:
            return 0.0
        try:
            parsed = float(raw)
        except ValueError:
            continue
        if parsed >= 0:
            return parsed
    return DEFAULT_TERMINATION_WAIT_SEC


def _is_windows_process_model() -> bool:
    return os.name == "nt"


def _write_active_guard_marker(pid: int) -> tuple[str, Path]:
    if pid <= 0:
        raise ValueError("active guard marker requires a live pid")
    token = os.urandom(16).hex()
    ACTIVE_GUARD_MARKER_DIR.mkdir(parents=True, exist_ok=True)
    marker_path = ACTIVE_GUARD_MARKER_DIR / f"guard-{pid}-{token}.json"
    tmp_path = marker_path.with_name(f".{marker_path.name}.{os.getpid()}.tmp")
    payload = {
        "schema_version": 1,
        "pid": pid,
        "token": token,
        "path": str(Path(__file__).resolve()),
        "created_at": _utc_timestamp(),
    }
    tmp_path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp_path, marker_path)
    _prune_active_guard_markers()
    return token, marker_path


def _prune_active_guard_markers() -> None:
    with contextlib.suppress(OSError):
        markers = sorted(
            ACTIVE_GUARD_MARKER_DIR.glob("guard-*.json"),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
        for marker in markers[ACTIVE_GUARD_MARKER_KEEP:]:
            with contextlib.suppress(OSError):
                marker.unlink()


def signal_payload(sig: int) -> dict[str, int | str]:
    try:
        name = signal.Signals(sig).name
    except ValueError:
        name = str(sig)
    return {"signal": int(sig), "name": name}


def term_signal_payload() -> dict[str, int | str]:
    return signal_payload(signal.SIGTERM)


def fallback_kill_signal() -> int:
    return getattr(signal, "SIGKILL", signal.SIGTERM)


def fallback_kill_signal_payload() -> dict[str, int | str]:
    return signal_payload(fallback_kill_signal())


@dataclass(frozen=True, slots=True)
class ProcessSample:
    pid: int
    ppid: int
    rss_kb: int
    command: str
    pgid: int | None = None
    elapsed_sec: int | None = None
    started_at_ns: int | None = None


ProcessIdentity = tuple[int | None, str, int | None]


def process_identity(sample: ProcessSample) -> ProcessIdentity:
    return (sample.pgid, sample.command, sample.started_at_ns)


@dataclass(slots=True)
class ProcessTreeTracker:
    root_pid: int
    known_pids: set[int] | None = None
    known_pgids: set[int] | None = None
    known_identities: dict[int, ProcessIdentity] | None = None

    def __post_init__(self) -> None:
        if self.known_pids is None:
            self.known_pids = {self.root_pid}
        else:
            self.known_pids.add(self.root_pid)
        if self.known_pgids is None:
            self.known_pgids = {self.root_pid}
        else:
            self.known_pgids.add(self.root_pid)
        if self.known_identities is None:
            self.known_identities = {}

    def update(self, samples: Mapping[int, ProcessSample]) -> set[int]:
        """Return currently observed members of this process tree.

        PID lineage is the only authority for discovering descendants.  Process
        groups are signal-delivery metadata for already-proven descendants; they
        must never make unrelated co-tenants part of the owned tree.
        """

        assert self.known_pids is not None
        assert self.known_pgids is not None
        assert self.known_identities is not None
        for pid in list(self.known_pids):
            sample = samples.get(pid)
            if sample is None:
                continue
            identity = process_identity(sample)
            known_identity = self.known_identities.get(pid)
            if known_identity is None:
                self.known_identities[pid] = identity
            elif known_identity != identity:
                self.known_pids.remove(pid)
                self.known_identities.pop(pid, None)
        changed = True
        while changed:
            changed = False
            for sample in samples.values():
                sample_pgid = sample.pgid if sample.pgid is not None else sample.pid
                if sample.pid in self.known_pids or sample.ppid in self.known_pids:
                    if sample.pid not in self.known_pids:
                        self.known_pids.add(sample.pid)
                        self.known_identities[sample.pid] = process_identity(sample)
                        changed = True
                    if (
                        sample.pid != self.root_pid or sample_pgid == self.root_pid
                    ) and sample_pgid not in self.known_pgids:
                        self.known_pgids.add(sample_pgid)
                        changed = True
        return {pid for pid in self.known_pids if pid in samples}


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
    stdout: str | bytes
    stderr: str | bytes
    timed_out: bool = False
    elapsed_s: float | None = None
    limit_at_violation: ResolvedMemoryLimits | None = None
    orphaned_process_groups: tuple[int, ...] = ()
    cargo_incremental_quarantine: CargoIncrementalQuarantine | None = None
    guard_signal: int | None = None
    child_process: GuardedChildProcess | None = None
    termination_reports: tuple[GuardTerminationReport, ...] = ()


@dataclass(frozen=True, slots=True)
class ChildExitResourceUsage:
    max_rss_kb: int


@dataclass(frozen=True, slots=True)
class GuardedLaunch:
    command: list[str]
    env: Mapping[str, str] | None
    pass_fds: tuple[int, ...] = ()
    close_fds: tuple[int, ...] = ()
    started_read_fd: int | None = None
    preexec_fn: Callable[[], None] | None = None


@dataclass(frozen=True, slots=True)
class GuardedChildProcess:
    pid: int
    pgid: int | None
    sid: int | None
    command: tuple[str, ...]
    started_at: str


@dataclass(frozen=True, slots=True)
class GuardTerminationAction:
    target_kind: str
    target_id: int
    signal: int | None
    signal_name: str | None
    result: str
    error: str | None = None


@dataclass(frozen=True, slots=True)
class GuardTerminationReport:
    reason: str
    started_at: str
    completed_at: str
    root_pid: int
    root_pgid: int | None
    root_sid: int | None
    grace_sec: float
    watched_pids: tuple[int, ...]
    protected_pgids: tuple[int, ...]
    escaped_pids: tuple[int, ...]
    remaining_pgids: tuple[int, ...]
    remaining_pids: tuple[int, ...]
    actions: tuple[GuardTerminationAction, ...]


@dataclass(frozen=True, slots=True)
class GuardOrphanCleanupResult:
    process_groups: tuple[int, ...] = ()
    termination_reports: tuple[GuardTerminationReport, ...] = ()


def _elapsed_seconds_from_ps(value: str) -> int | None:
    raw = value.strip()
    if not raw:
        return None
    if raw.isdigit():
        return int(raw)
    days = 0
    time_part = raw
    if "-" in raw:
        day_part, time_part = raw.split("-", 1)
        if not day_part.isdigit():
            return None
        days = int(day_part)
    fields = time_part.split(":")
    if not 1 <= len(fields) <= 3 or any(not field.isdigit() for field in fields):
        return None
    values = [int(field) for field in fields]
    if len(values) == 3:
        hours, minutes, seconds = values
    elif len(values) == 2:
        hours = 0
        minutes, seconds = values
    else:
        hours = 0
        minutes = 0
        seconds = values[0]
    return (((days * 24) + hours) * 60 + minutes) * 60 + seconds


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
        elapsed_sec: int | None = None
        parts = line.split(None, 5)
        if len(parts) >= 6:
            try:
                pid = int(parts[0])
                ppid = int(parts[1])
                pgid = int(parts[2])
                rss_kb = int(parts[3])
                elapsed_sec = _elapsed_seconds_from_ps(parts[4])
                if elapsed_sec is None:
                    raise ValueError("elapsed process age is not parseable")
                command = parts[5]
            except ValueError:
                legacy_parts = line.split(None, 4)
                if len(legacy_parts) < 5:
                    continue
                try:
                    pid = int(legacy_parts[0])
                    ppid = int(legacy_parts[1])
                    pgid = int(legacy_parts[2])
                    rss_kb = int(legacy_parts[3])
                except ValueError:
                    fallback_parts = line.split(None, 3)
                    if len(fallback_parts) < 4:
                        continue
                    try:
                        pid = int(fallback_parts[0])
                        ppid = int(fallback_parts[1])
                        rss_kb = int(fallback_parts[2])
                    except ValueError:
                        continue
                    command = fallback_parts[3]
                    pgid = None
                else:
                    command = legacy_parts[4]
        elif len(parts) >= 5:
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
            elapsed_sec=elapsed_sec,
        )
    return samples


def parse_windows_process_snapshot_rows(
    rows: Sequence[
        tuple[int, int, int, str, int | None]
        | tuple[int, int, int, str, int | None, int | None]
    ],
) -> dict[int, ProcessSample]:
    samples: dict[int, ProcessSample] = {}
    for row in rows:
        if len(row) == 5:
            pid, ppid, rss_kb, command, elapsed_sec = row
            started_at_ns = None
        else:
            pid, ppid, rss_kb, command, elapsed_sec, started_at_ns = row
        if pid <= 0:
            continue
        samples[pid] = ProcessSample(
            pid=pid,
            ppid=max(0, ppid),
            rss_kb=max(0, rss_kb),
            command=command.strip() or f"pid:{pid}",
            pgid=None,
            elapsed_sec=elapsed_sec,
            started_at_ns=started_at_ns,
        )
    return samples


def sample_processes_posix() -> dict[int, ProcessSample]:
    try:
        result = subprocess.run(
            ["ps", "-axo", "pid=,ppid=,pgid=,rss=,etime=,command="],
            capture_output=True,
            text=True,
            timeout=2.0,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired, TypeError):
        return {}
    if result.returncode != 0:
        return {}
    return parse_process_table(result.stdout)


def sample_processes_windows() -> dict[int, ProcessSample]:
    try:
        rows = _windows_process_snapshot_rows()
    except (OSError, TypeError, AttributeError):
        return {}
    return parse_windows_process_snapshot_rows(rows)


def sample_processes() -> dict[int, ProcessSample]:
    if _is_windows_process_model():
        return sample_processes_windows()
    return sample_processes_posix()

def _sample_pgid(sample: ProcessSample) -> int:
    return sample.pgid if sample.pgid is not None else sample.pid


def _command_executable_name(command: str) -> str:
    text = command.strip()
    if not text:
        return ""
    if text[0] in {"'", '"'}:
        quote = text[0]
        end = text.find(quote, 1)
        token = text[1:end] if end > 0 else text[1:]
    else:
        token = text.split(None, 1)[0]
    return token.replace("\\", "/").rsplit("/", 1)[-1].casefold()


def is_host_control_plane_process(sample: ProcessSample) -> bool:
    command = sample.command.casefold()
    return (
        any(token.casefold() in command for token in HOST_CONTROL_PLANE_TOKENS)
        or _command_executable_name(sample.command)
        in HOST_CONTROL_PLANE_EXECUTABLE_NAMES
    )


def _ancestor_pids(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
) -> set[int]:
    if pid is None or pid <= 0:
        return set()
    ancestors: set[int] = set()
    current = pid
    while current > 0 and current not in ancestors:
        ancestors.add(current)
        sample = samples.get(current)
        if sample is None or sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return ancestors


def protected_process_group_ids(
    samples: Mapping[int, ProcessSample],
    *,
    self_pid: int | None = None,
    self_pgid: int | None = None,
) -> set[int]:
    protected: set[int] = set()
    if self_pgid is not None and self_pgid > 0:
        protected.add(self_pgid)
    ancestor_ids = _ancestor_pids(samples, self_pid)
    self_descendant_ids = descendant_pids(samples, self_pid) if self_pid else set()
    host_control_plane_pids = {
        sample.pid
        for sample in samples.values()
        if is_host_control_plane_process(sample)
    }
    for sample in samples.values():
        if sample.pid in ancestor_ids or is_host_control_plane_process(sample):
            protected.add(_sample_pgid(sample))
            continue
        sample_ancestors = _ancestor_pids(samples, sample.pid)
        if (
            host_control_plane_pids.intersection(sample_ancestors)
            and sample.pid not in self_descendant_ids
        ):
            protected.add(_sample_pgid(sample))
    return protected


def _root_pid_is_kill_eligible(
    samples: Mapping[int, ProcessSample],
    root_pid: int,
    *,
    protected_pgids: set[int],
    root_owned: bool,
) -> bool:
    if root_pid <= 0 or root_pid == os.getpid():
        return False
    sample = samples.get(root_pid)
    if sample is None:
        return root_owned
    return _sample_pgid(sample) not in protected_pgids and not is_host_control_plane_process(
        sample
    )


def _current_protected_process_group_ids(
    samples: Mapping[int, ProcessSample],
) -> set[int]:
    return protected_process_group_ids(
        samples,
        self_pid=os.getpid(),
        self_pgid=_safe_getpgrp(),
    )


def _filter_protected_watched_pids(
    samples: Mapping[int, ProcessSample],
    watched: set[int],
) -> set[int]:
    protected_pgids = _current_protected_process_group_ids(samples)
    if not protected_pgids:
        return watched
    filtered: set[int] = set()
    for pid in watched:
        sample = samples.get(pid)
        if sample is not None and _sample_pgid(sample) in protected_pgids:
            continue
        filtered.add(pid)
    return filtered


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


def watched_pids(
    samples: Mapping[int, ProcessSample],
    root_pid: int,
    *,
    tracker: ProcessTreeTracker | None = None,
) -> set[int]:
    if tracker is not None:
        return _filter_protected_watched_pids(samples, tracker.update(samples))
    watched = descendant_pids(samples, root_pid)
    return _filter_protected_watched_pids(samples, watched)


def peak_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
) -> RssViolation | None:
    observed = (
        watched
        if watched is not None
        else watched_pids(samples, root_pid, tracker=tracker)
    )
    candidates = [sample for pid, sample in samples.items() if pid in observed]
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
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
) -> RssViolation | None:
    observed = (
        watched
        if watched is not None
        else watched_pids(samples, root_pid, tracker=tracker)
    )
    candidates = [sample for pid, sample in samples.items() if pid in observed]
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
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
) -> RssViolation | None:
    observed = (
        watched
        if watched is not None
        else watched_pids(samples, root_pid, tracker=tracker)
    )
    candidates = [
        sample
        for pid, sample in samples.items()
        if pid in observed and sample.rss_kb > max_rss_kb
    ]
    if not candidates:
        if max_total_rss_kb is None:
            return None
        aggregate = total_rss(samples, root_pid=root_pid, watched=observed)
        if aggregate is not None and aggregate.rss_kb > max_total_rss_kb:
            return aggregate
        return None
    worst = max(candidates, key=lambda sample: sample.rss_kb)
    return RssViolation(
        pid=worst.pid,
        rss_kb=worst.rss_kb,
        command=worst.command,
    )


def _rusage_maxrss_kb(rusage: object) -> int:
    raw = int(getattr(rusage, "ru_maxrss", 0) or 0)
    if raw <= 0:
        return 0
    if sys.platform == "darwin":
        return max(1, raw // 1024)
    return raw


def _poll_wait4_child(proc: subprocess.Popen[str]) -> ChildExitResourceUsage | None:
    if os.name != "posix" or not hasattr(os, "wait4"):
        return None
    if proc.returncode is not None:
        return None
    try:
        pid, status, rusage = os.wait4(proc.pid, os.WNOHANG)
    except ChildProcessError:
        return None
    if pid == 0:
        return None
    proc.returncode = os.waitstatus_to_exitcode(status)
    return ChildExitResourceUsage(max_rss_kb=_rusage_maxrss_kb(rusage))


def _set_env_gb_ceiling(env: dict[str, str], name: str, limit_kb: int) -> None:
    limit_gb = limit_kb / (1024 * 1024)
    raw = env.get(name)
    if raw is not None and raw.strip():
        with contextlib.suppress(ValueError):
            parsed = float(raw)
            if parsed > 0:
                limit_gb = min(limit_gb, parsed)
    env[name] = f"{limit_gb:.6f}"


def _inject_guard_memory_contract_env(
    env: dict[str, str],
    *,
    max_rss_kb: int,
    child_rlimit_kb: int | None,
) -> None:
    limit_candidates = [max_rss_kb]
    if child_rlimit_kb is not None and child_rlimit_kb > 0:
        limit_candidates.append(child_rlimit_kb)
    limit_kb = min(limit_candidates)
    _set_env_gb_ceiling(env, "MOLT_BACKEND_MEMORY_AVAILABLE_GB", limit_kb)
    _set_env_gb_ceiling(env, "MOLT_BACKEND_MAX_RSS_GB", limit_kb)


def _samples_max_bytes_from_mb(value: float | None) -> int | None:
    if value is None:
        value = DEFAULT_SAMPLES_MAX_MB
    if value <= 0:
        return None
    return max(1024, int(value * 1024 * 1024))


def _rotate_jsonl_if_needed(
    path: Path, incoming_bytes: int, max_bytes: int | None
) -> None:
    if max_bytes is None:
        return
    try:
        current_size = path.stat().st_size
    except FileNotFoundError:
        return
    except OSError:
        return
    if current_size + incoming_bytes <= max_bytes:
        return
    rotated = path.with_name(f"{path.name}.1")
    with contextlib.suppress(OSError):
        rotated.unlink()
    with contextlib.suppress(OSError):
        path.replace(rotated)


def _append_sample_jsonl(
    path: str,
    *,
    root_pid: int,
    peak: RssViolation | None,
    total: RssViolation | None,
    violation: RssViolation | None,
    max_bytes: int | None = None,
) -> None:
    sample_path = Path(path)
    if sample_path.parent:
        sample_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "ts": time.time(),
        "root_pid": root_pid,
        "peak": _rss_record_payload(peak),
        "total": _rss_record_payload(total),
        "violation": _rss_record_payload(violation),
    }
    line = json.dumps(payload, sort_keys=True) + "\n"
    _rotate_jsonl_if_needed(sample_path, len(line.encode("utf-8")), max_bytes)
    with sample_path.open("a", encoding="utf-8") as handle:
        handle.write(line)


def _record_gb(record: object) -> str:
    if not isinstance(record, dict):
        return "-"
    value = record.get("rss_gb")
    if isinstance(value, (int, float)):
        return f"{value:.2f}GB"
    return "-"


def _format_sample_payload(payload: dict[str, object]) -> str:
    violation = payload.get("violation")
    if violation is not None:
        return f"memory_guard sample: TRIP violation={_record_gb(violation)}"
    return (
        "memory_guard sample: "
        f"peak={_record_gb(payload.get('peak'))} "
        f"total={_record_gb(payload.get('total'))}"
    )


def _stream_sample_payload(payload: dict[str, object], stream: str) -> None:
    if not stream:
        return
    target = sys.stdout if "stdout" in stream else sys.stderr
    try:
        if "json" in stream:
            print(json.dumps(payload, sort_keys=True), file=target, flush=True)
        else:
            print(_format_sample_payload(payload), file=target, flush=True)
    except Exception:
        return


def _record_sample(
    *,
    root_pid: int,
    peak: RssViolation | None,
    total: RssViolation | None,
    violation: RssViolation | None,
    limits: ResolvedMemoryLimits | None = None,
    samples_jsonl: str | None,
    samples_jsonl_max_bytes: int | None,
    stream: str,
) -> None:
    payload = {
        "ts": time.time(),
        "root_pid": root_pid,
        "peak": _rss_record_payload(peak),
        "total": _rss_record_payload(total),
        "violation": _rss_record_payload(violation),
    }
    if limits is not None:
        payload["limits"] = memory_limits_payload(limits)
    if samples_jsonl is not None:
        sample_path = Path(samples_jsonl)
        if sample_path.parent:
            sample_path.parent.mkdir(parents=True, exist_ok=True)
        line = json.dumps(payload, sort_keys=True) + "\n"
        _rotate_jsonl_if_needed(
            sample_path,
            len(line.encode("utf-8")),
            samples_jsonl_max_bytes,
        )
        with sample_path.open("a", encoding="utf-8") as handle:
            handle.write(line)
    _stream_sample_payload(payload, stream)


def _terminate_single_process_group(pgid: int, *, grace: float) -> bool:
    if pgid <= 0:
        return True
    if _is_windows_process_model():
        return _terminate_single_pid(pgid, grace=grace)
    if os.name == "posix":
        if pgid == os.getpgrp():
            return True
        samples = sample_processes()
        if pgid in _current_protected_process_group_ids(samples):
            return True
    try:
        os.killpg(pgid, signal.SIGTERM)
    except KeyboardInterrupt:
        return False
    except ProcessLookupError:
        return True
    except OSError:
        with contextlib.suppress(ProcessLookupError):
            os.kill(pgid, signal.SIGTERM)
        return False
    deadline = time.monotonic() + max(0.0, grace)
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return True
        except OSError:
            return True
        try:
            time.sleep(0.02)
        except KeyboardInterrupt:
            return False
    return False


def _terminate_single_pid(pid: int, *, grace: float) -> bool:
    if pid <= 0 or pid == os.getpid():
        return True
    samples = sample_processes()
    sample = samples.get(pid)
    if sample is not None:
        if is_host_control_plane_process(sample):
            return True
        if _sample_pgid(sample) in _current_protected_process_group_ids(samples):
            return True
    try:
        os.kill(pid, signal.SIGTERM)
    except KeyboardInterrupt:
        return False
    except ProcessLookupError:
        return True
    except OSError:
        return False
    deadline = time.monotonic() + max(0.0, grace)
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        except OSError:
            return True
        try:
            time.sleep(0.02)
        except KeyboardInterrupt:
            return False
    return False


def _pid_exited_or_unobservable(pid: int, *, grace: float) -> bool:
    deadline = time.monotonic() + max(0.0, grace)
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return True
        except OSError:
            return True
        time.sleep(0.02)
    return False


def _process_group_exited_or_unobservable(pgid: int, *, grace: float) -> bool:
    if _is_windows_process_model():
        return _pid_exited_or_unobservable(pgid, grace=grace)
    deadline = time.monotonic() + max(0.0, grace)
    while time.monotonic() < deadline:
        try:
            os.killpg(pgid, 0)
        except ProcessLookupError:
            return True
        except OSError:
            return True
        time.sleep(0.02)
    return False


def _signal_name(signum: int | None) -> str | None:
    if signum is None:
        return None
    with contextlib.suppress(ValueError):
        return signal.Signals(signum).name
    return None


def _termination_action(
    *,
    target_kind: str,
    target_id: int,
    signum: int | None,
    result: str,
    error: str | None = None,
) -> GuardTerminationAction:
    return GuardTerminationAction(
        target_kind=target_kind,
        target_id=target_id,
        signal=signum,
        signal_name=_signal_name(signum),
        result=result,
        error=error,
    )


def _send_pid_signal_action(pid: int, signum: int) -> GuardTerminationAction:
    try:
        os.kill(pid, signum)
    except ProcessLookupError:
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="missing",
        )
    except (PermissionError, OSError) as exc:
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="failed",
            error=str(exc),
        )
    return _termination_action(
        target_kind="process",
        target_id=pid,
        signum=signum,
        result="sent",
    )


def _send_process_group_signal_action(
    pgid: int,
    signum: int,
) -> GuardTerminationAction:
    try:
        os.killpg(pgid, signum)
    except ProcessLookupError:
        return _termination_action(
            target_kind="process_group",
            target_id=pgid,
            signum=signum,
            result="missing",
        )
    except (PermissionError, OSError) as exc:
        return _termination_action(
            target_kind="process_group",
            target_id=pgid,
            signum=signum,
            result="failed",
            error=str(exc),
        )
    return _termination_action(
        target_kind="process_group",
        target_id=pgid,
        signum=signum,
        result="sent",
    )


def _send_pid_signal_if_identity_action(
    pid: int,
    identity: ProcessIdentity | None,
    signum: int,
    *,
    sampler: Callable[[], Mapping[int, ProcessSample]],
) -> GuardTerminationAction:
    samples = sampler()
    sample = samples.get(pid)
    if sample is None:
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="missing",
        )
    if identity is not None and process_identity(sample) != identity:
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="skipped_identity_mismatch",
        )
    if is_host_control_plane_process(sample):
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="skipped_host_control_plane",
        )
    if _sample_pgid(sample) in _current_protected_process_group_ids(samples):
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="skipped_protected_group_member",
        )
    return _send_pid_signal_action(pid, signum)


def _send_process_group_signal_if_identities_match_action(
    pgid: int,
    identities: Mapping[int, ProcessIdentity],
    signum: int,
    *,
    sampler: Callable[[], Mapping[int, ProcessSample]],
) -> GuardTerminationAction:
    samples = sampler()
    protected_pgids = _current_protected_process_group_ids(samples)
    if pgid in protected_pgids:
        return _termination_action(
            target_kind="process_group",
            target_id=pgid,
            signum=signum,
            result="skipped_protected_group",
        )
    members = _process_group_members(samples, pgid)
    if not members:
        return _termination_action(
            target_kind="process_group",
            target_id=pgid,
            signum=signum,
            result="missing",
        )
    for sample in members:
        identity = identities.get(sample.pid)
        if identity is None or process_identity(sample) != identity:
            return _termination_action(
                target_kind="process_group",
                target_id=pgid,
                signum=signum,
                result="skipped_identity_mismatch",
            )
        if is_host_control_plane_process(sample):
            return _termination_action(
                target_kind="process_group",
                target_id=pgid,
                signum=signum,
                result="skipped_host_control_plane",
            )
    return _send_process_group_signal_action(pgid, signum)


def _terminate_process_group_if_identities_match_action(
    pgid: int,
    identities: Mapping[int, ProcessIdentity],
    *,
    sampler: Callable[[], Mapping[int, ProcessSample]],
    grace: float,
) -> GuardTerminationAction:
    action = _send_process_group_signal_if_identities_match_action(
        pgid,
        identities,
        signal.SIGTERM,
        sampler=sampler,
    )
    if action.result != "sent":
        return action
    terminated = _process_group_exited_or_unobservable(pgid, grace=grace)
    return _termination_action(
        target_kind="process_group",
        target_id=pgid,
        signum=signal.SIGTERM,
        result="completed_or_missing" if terminated else "still_live",
    )


def _process_group_members(
    samples: Mapping[int, ProcessSample],
    pgid: int,
) -> tuple[ProcessSample, ...]:
    return tuple(sample for sample in samples.values() if _sample_pgid(sample) == pgid)


def _process_group_is_fully_owned(
    samples: Mapping[int, ProcessSample],
    pgid: int,
    *,
    owned_pids: set[int],
    protected_pgids: set[int],
) -> bool:
    if pgid <= 0 or pgid in protected_pgids:
        return False
    members = _process_group_members(samples, pgid)
    return bool(members) and all(sample.pid in owned_pids for sample in members)


def terminate_watched_processes(
    root_pid: int,
    *,
    samples: Mapping[int, ProcessSample] | None = None,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
    grace: float = 0.25,
    root_owned: bool = False,
    reason: str = "terminate_watched_processes",
    sampler: Callable[[], Mapping[int, ProcessSample]] | None = None,
) -> GuardTerminationReport:
    if sampler is None:
        sampler = sample_processes
    started_at = _utc_timestamp()
    actions: list[GuardTerminationAction] = []

    def finish(
        *,
        root_pgid: int | None = None,
        root_sid: int | None = None,
        watched_pids: set[int] | None = None,
        protected_pgids: set[int] | None = None,
        escaped_pids: set[int] | None = None,
        remaining_pgids: set[int] | None = None,
        remaining_pids: set[int] | None = None,
        finish_reason: str | None = None,
    ) -> GuardTerminationReport:
        return GuardTerminationReport(
            reason=reason if finish_reason is None else finish_reason,
            started_at=started_at,
            completed_at=_utc_timestamp(),
            root_pid=root_pid,
            root_pgid=root_pgid,
            root_sid=root_sid,
            grace_sec=grace,
            watched_pids=tuple(sorted(watched_pids or ())),
            protected_pgids=tuple(sorted(protected_pgids or ())),
            escaped_pids=tuple(sorted(escaped_pids or ())),
            remaining_pgids=tuple(sorted(remaining_pgids or ())),
            remaining_pids=tuple(sorted(remaining_pids or ())),
            actions=tuple(actions),
        )

    if root_pid <= 0:
        actions.append(
            _termination_action(
                target_kind="process",
                target_id=root_pid,
                signum=None,
                result="skipped_invalid_pid",
            )
        )
        return finish(finish_reason="invalid_root_pid")
    if _is_windows_process_model():
        observed_samples = sampler() if samples is None else samples
        observed_identities = {
            pid: process_identity(sample) for pid, sample in observed_samples.items()
        }
        observed = (
            watched
            if watched is not None
            else watched_pids(observed_samples, root_pid, tracker=tracker)
        )
        protected_pgids = _current_protected_process_group_ids(observed_samples)
        owned_pids = _filter_protected_watched_pids(observed_samples, set(observed))
        root_sample = observed_samples.get(root_pid)
        root_group_pgid = None if root_sample is None else _sample_pgid(root_sample)
        if _root_pid_is_kill_eligible(
            observed_samples,
            root_pid,
            protected_pgids=protected_pgids,
            root_owned=root_owned,
        ):
            owned_pids.add(root_pid)
        else:
            if root_pid == os.getpid():
                result = "skipped_guard_process"
            elif root_sample is not None and is_host_control_plane_process(root_sample):
                result = "skipped_host_control_plane"
            elif root_sample is not None and _sample_pgid(root_sample) in protected_pgids:
                result = "skipped_protected_root_group"
            else:
                result = "skipped_unowned_root_pid"
            actions.append(
                _termination_action(
                    target_kind="process",
                    target_id=root_pid,
                    signum=None,
                    result=result,
                )
            )
        identity_sampler = (lambda: observed_samples) if samples is not None else sampler
        remaining_pids: set[int] = set()
        for pid in sorted(owned_pids, reverse=True):
            if pid <= 0:
                continue
            if pid == os.getpid():
                actions.append(
                    _termination_action(
                        target_kind="process",
                        target_id=pid,
                        signum=signal.SIGTERM,
                        result="skipped_guard_process",
                    )
                )
                continue
            sample = observed_samples.get(pid)
            if sample is not None and is_host_control_plane_process(sample):
                actions.append(
                    _termination_action(
                        target_kind="process",
                        target_id=pid,
                        signum=signal.SIGTERM,
                        result="skipped_host_control_plane",
                    )
                )
                continue
            if sample is not None and _sample_pgid(sample) in protected_pgids:
                actions.append(
                    _termination_action(
                        target_kind="process",
                        target_id=pid,
                        signum=signal.SIGTERM,
                        result="skipped_protected_group_member",
                    )
                )
                continue
            identity = observed_identities.get(pid)
            if identity is None:
                if pid == root_pid and root_owned:
                    actions.append(_send_pid_signal_action(pid, signal.SIGTERM))
                    time.sleep(max(0.0, grace))
                    actions.append(_send_pid_signal_action(pid, fallback_kill_signal()))
                    continue
                actions.append(
                    _termination_action(
                        target_kind="process",
                        target_id=pid,
                        signum=signal.SIGTERM,
                        result="skipped_missing_identity",
                    )
                )
                continue
            action = _terminate_pid_if_identity_action(
                pid,
                identity,
                sampler=identity_sampler,
                grace=grace,
            )
            actions.append(action)
            if action.result == "still_live":
                remaining_pids.add(pid)
        for pid in sorted(remaining_pids, reverse=True):
            identity = observed_identities.get(pid)
            if identity is None and pid == root_pid and root_owned:
                actions.append(_send_pid_signal_action(pid, fallback_kill_signal()))
                continue
            actions.append(
                _send_pid_signal_if_identity_action(
                    pid,
                    identity,
                    fallback_kill_signal(),
                    sampler=identity_sampler,
                )
            )
        return finish(
            root_pgid=root_group_pgid,
            watched_pids=owned_pids,
            protected_pgids=protected_pgids,
            remaining_pids=remaining_pids,
            finish_reason="windows_pid_tree",
        )
    if os.name != "posix":
        actions.append(_send_pid_signal_action(root_pid, signal.SIGTERM))
        time.sleep(max(0.0, grace))
        actions.append(_send_pid_signal_action(root_pid, fallback_kill_signal()))
        return finish(
            watched_pids={root_pid},
            remaining_pids={root_pid},
            finish_reason="non_posix_root_pid",
        )
    observed_samples = sampler() if samples is None else samples
    observed_identities = {
        pid: process_identity(sample) for pid, sample in observed_samples.items()
    }
    observed = (
        watched
        if watched is not None
        else watched_pids(observed_samples, root_pid, tracker=tracker)
    )
    protected_pgids = _current_protected_process_group_ids(observed_samples)
    root_sample = observed_samples.get(root_pid)
    root_group_pgid = (
        _sample_pgid(root_sample)
        if root_sample is not None
        else _safe_getpgid(root_pid) or root_pid
    )
    root_sid = _safe_getsid(root_pid)
    observed = _filter_protected_watched_pids(observed_samples, set(observed))
    pids: set[int] = set()
    if _root_pid_is_kill_eligible(
        observed_samples,
        root_pid,
        protected_pgids=protected_pgids,
        root_owned=root_owned,
    ):
        pids.add(root_pid)
    else:
        actions.append(
            _termination_action(
                target_kind="process_group",
                target_id=root_group_pgid,
                signum=None,
                result="skipped_protected_root_group",
            )
        )
    escaped_pids: set[int] = set()
    for pid in observed:
        if pid <= 0:
            continue
        sample = observed_samples.get(pid)
        pids.add(pid)
        if sample is not None:
            sample_pgid = _sample_pgid(sample)
            if sample_pgid in protected_pgids:
                pids.discard(pid)
                actions.append(
                    _termination_action(
                        target_kind="process",
                        target_id=pid,
                        signum=None,
                        result="skipped_protected_group_member",
                    )
                )
                continue
            if sample_pgid != root_group_pgid:
                escaped_pids.add(pid)
    remaining_pgids: set[int] = set()
    root_group_fully_owned = _process_group_is_fully_owned(
        observed_samples,
        root_group_pgid,
        owned_pids=pids,
        protected_pgids=protected_pgids,
    )
    if root_group_fully_owned:
        action = _terminate_process_group_if_identities_match_action(
            root_group_pgid,
            observed_identities,
            sampler=sampler,
            grace=grace,
        )
        actions.append(action)
        if action.result == "still_live":
            remaining_pgids.add(root_group_pgid)
    elif root_group_pgid not in protected_pgids and root_group_pgid > 0:
        actions.append(
            _termination_action(
                target_kind="process_group",
                target_id=root_group_pgid,
                signum=None,
                result="skipped_not_fully_owned",
            )
        )
    remaining_pids: set[int] = set()
    for pid in sorted(escaped_pids):
        identity = observed_identities.get(pid)
        if identity is None:
            actions.append(
                _termination_action(
                    target_kind="process",
                    target_id=pid,
                    signum=signal.SIGTERM,
                    result="skipped_missing_identity",
                )
            )
            continue
        action = _terminate_pid_if_identity_action(
            pid,
            identity,
            sampler=sampler,
            grace=grace,
        )
        actions.append(action)
        if action.result == "still_live":
            remaining_pids.add(pid)
    for pgid in sorted(remaining_pgids):
        actions.append(
            _send_process_group_signal_if_identities_match_action(
                pgid,
                observed_identities,
                signal.SIGKILL,
                sampler=sampler,
            )
        )
    for pid in sorted(pids | remaining_pids):
        if pid == os.getpid():
            actions.append(
                _termination_action(
                    target_kind="process",
                    target_id=pid,
                    signum=signal.SIGKILL,
                    result="skipped_guard_process",
                )
            )
            continue
        actions.append(
            _send_pid_signal_if_identity_action(
                pid,
                observed_identities.get(pid),
                signal.SIGKILL,
                sampler=sampler,
            )
        )
    return finish(
        root_pgid=root_group_pgid,
        root_sid=root_sid,
        watched_pids=set(observed),
        protected_pgids=protected_pgids,
        escaped_pids=escaped_pids,
        remaining_pgids=remaining_pgids,
        remaining_pids=remaining_pids,
    )


def cleanup_tracked_orphans(
    root_pid: int,
    *,
    tracker: ProcessTreeTracker,
    sampler: Callable[[], Mapping[int, ProcessSample]] | None = None,
    remembered_samples: Mapping[int, ProcessSample] | None = None,
    remembered_watched: set[int] | None = None,
    grace: float = 0.25,
) -> GuardOrphanCleanupResult:
    """Terminate descendants still alive after the guarded root process exits."""

    if root_pid <= 0:
        return GuardOrphanCleanupResult()
    if sampler is None:
        sampler = sample_processes
    sampler_failure: BaseException | None = None
    try:
        samples = sampler()
    except (KeyboardInterrupt, Exception) as exc:
        sampler_failure = exc
        if remembered_watched is None:
            raise
        samples = {} if remembered_samples is None else remembered_samples
    observed = tracker.update(samples)
    if not observed and remembered_watched is not None:
        observed = set(remembered_watched)
    watched = _filter_protected_watched_pids(samples, observed)
    live_pgids: set[int] = set()
    for pid in watched:
        sample = samples.get(pid)
        if sample is None:
            continue
        live_pgids.add(sample.pgid if sample.pgid is not None else sample.pid)
    if not live_pgids and not watched:
        if sampler_failure is not None:
            raise sampler_failure
        return GuardOrphanCleanupResult()
    report = terminate_watched_processes(
        root_pid,
        samples=samples,
        watched=watched,
        tracker=tracker,
        grace=grace,
        reason="tracked_orphan_cleanup",
        sampler=sampler,
        root_owned=True,
    )
    if sampler_failure is not None:
        raise sampler_failure
    return GuardOrphanCleanupResult(
        process_groups=tuple(sorted(live_pgids)),
        termination_reports=() if report is None else (report,),
    )


def _live_process_group_ids(samples: Mapping[int, ProcessSample]) -> frozenset[int]:
    return frozenset(
        pgid
        for sample in samples.values()
        for pgid in (_sample_pgid(sample),)
        if pgid > 0
    )


def _repo_scoped_post_baseline_orphan_groups(
    samples: Mapping[int, ProcessSample],
    *,
    baseline_pgids: frozenset[int],
    owned_pids: set[int],
) -> tuple[Any, ...]:
    from tools import process_sentinel

    candidates = {
        group.pgid: group
        for group in process_sentinel.process_groups(
            samples,
            root=ROOT,
            self_pid=os.getpid(),
            self_pgid=_safe_getpgrp(),
            owned_pids=set(owned_pids),
        )
        if group.pgid not in baseline_pgids
    }
    eligible_pgids: set[int] = set()
    eligible_pids: set[int] = set()
    changed = True
    while changed:
        changed = False
        for pgid, group in candidates.items():
            if pgid in eligible_pgids:
                continue
            parents = {pid for pid in group.external_parent_pids if pid > 0}
            if not parents:
                continue
            if all(
                parent == 1 or parent in owned_pids or parent in eligible_pids
                for parent in parents
            ):
                eligible_pgids.add(pgid)
                eligible_pids.update(group.pids)
                changed = True
    return tuple(candidates[pgid] for pgid in sorted(eligible_pgids))


def _terminate_pid_if_identity_action(
    pid: int,
    identity: ProcessIdentity,
    *,
    sampler: Callable[[], Mapping[int, ProcessSample]],
    grace: float,
) -> GuardTerminationAction:
    samples = sampler()
    sample = samples.get(pid)
    if sample is None:
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=None,
            result="skipped_missing",
        )
    if process_identity(sample) != identity:
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=None,
            result="skipped_identity_mismatch",
        )
    if is_host_control_plane_process(sample):
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=None,
            result="skipped_host_control_plane",
        )
    if _sample_pgid(sample) in _current_protected_process_group_ids(samples):
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=None,
            result="skipped_protected_group_member",
        )
    action = _send_pid_signal_if_identity_action(
        pid,
        identity,
        signal.SIGTERM,
        sampler=sampler,
    )
    if action.result != "sent":
        return action
    terminated = _pid_exited_or_unobservable(pid, grace=grace)
    return _termination_action(
        target_kind="process",
        target_id=pid,
        signum=signal.SIGTERM,
        result="completed_or_missing" if terminated else "still_live",
    )


def _repo_scoped_orphan_cleanup_report(
    group: Any,
    *,
    fresh_samples: Mapping[int, ProcessSample],
    actions: Sequence[GuardTerminationAction],
    grace: float,
    started_at: str,
) -> GuardTerminationReport:
    remaining_pids = {
        action.target_id
        for action in actions
        if action.target_kind == "process" and action.result == "still_live"
    }
    root_pid = group.samples[0].pid if group.samples else group.pgid
    return GuardTerminationReport(
        reason="repo_scoped_orphan_cleanup",
        started_at=started_at,
        completed_at=_utc_timestamp(),
        root_pid=root_pid,
        root_pgid=group.pgid,
        root_sid=None,
        grace_sec=grace,
        watched_pids=tuple(sorted(sample.pid for sample in group.samples)),
        protected_pgids=tuple(
            sorted(_current_protected_process_group_ids(fresh_samples))
        ),
        escaped_pids=(),
        remaining_pgids=(group.pgid,) if remaining_pids else (),
        remaining_pids=tuple(sorted(remaining_pids)),
        actions=tuple(actions),
    )


def cleanup_repo_scoped_orphans_since_baseline(
    *,
    baseline_pgids: frozenset[int],
    tracker: ProcessTreeTracker,
    sampler: Callable[[], Mapping[int, ProcessSample]] = sample_processes,
    grace: float = 0.25,
) -> GuardOrphanCleanupResult:
    """Terminate newly orphaned groups proven to belong to this guard's tree."""

    if os.name != "posix":
        return GuardOrphanCleanupResult()

    samples = sampler()
    owned_pids = _filter_protected_watched_pids(samples, tracker.update(samples))
    if not owned_pids:
        return GuardOrphanCleanupResult()
    groups = _repo_scoped_post_baseline_orphan_groups(
        samples,
        baseline_pgids=baseline_pgids,
        owned_pids=set(owned_pids),
    )
    terminated: list[int] = []
    reports: list[GuardTerminationReport] = []
    for group in groups:
        fresh_samples = sampler()
        fresh_owned_pids = _filter_protected_watched_pids(
            fresh_samples,
            tracker.update(fresh_samples),
        )
        fresh_groups = {
            fresh_group.pgid: fresh_group
            for fresh_group in _repo_scoped_post_baseline_orphan_groups(
                fresh_samples,
                baseline_pgids=baseline_pgids,
                owned_pids=set(fresh_owned_pids),
            )
        }
        fresh_group = fresh_groups.get(group.pgid)
        if fresh_group is None:
            continue
        actions: list[GuardTerminationAction] = []
        started_at = _utc_timestamp()
        for sample in fresh_group.samples:
            actions.append(
                _terminate_pid_if_identity_action(
                    sample.pid,
                    process_identity(sample),
                    sampler=sampler,
                    grace=grace,
                )
            )
        if any(action.result == "completed_or_missing" for action in actions):
            terminated.append(fresh_group.pgid)
        if actions:
            reports.append(
                _repo_scoped_orphan_cleanup_report(
                    fresh_group,
                    fresh_samples=fresh_samples,
                    actions=actions,
                    grace=grace,
                    started_at=started_at,
                )
            )
    return GuardOrphanCleanupResult(
        process_groups=tuple(sorted(terminated)),
        termination_reports=tuple(reports),
    )


def _terminate_process_group(pid: int) -> None:
    terminate_watched_processes(pid, grace=5.0, root_owned=True)


def _load_json_string_list(environ: Mapping[str, str], name: str) -> list[str]:
    raw = environ.get(name)
    if not raw:
        raise ValueError(f"{name} is required")
    try:
        decoded = json.loads(raw)
    except json.JSONDecodeError as exc:
        raise ValueError(f"{name} is invalid JSON") from exc
    if not isinstance(decoded, list) or not all(
        isinstance(item, str) for item in decoded
    ):
        raise ValueError(f"{name} must be a JSON string list")
    if not decoded:
        raise ValueError(f"{name} command must not be empty")
    return decoded


def _apply_child_resource_limit(limit_kb: int) -> None:
    if limit_kb <= 0:
        return
    try:
        import resource  # type: ignore
    except Exception:
        return
    limit_bytes = int(limit_kb * 1024)
    for name in ("RLIMIT_AS", "RLIMIT_DATA", "RLIMIT_RSS"):
        res = getattr(resource, name, None)
        if res is None:
            continue
        try:
            soft, hard = resource.getrlimit(res)
            bounded_hard = (
                limit_bytes
                if hard == resource.RLIM_INFINITY
                else min(int(hard), limit_bytes)
            )
            bounded_soft = (
                limit_bytes
                if soft == resource.RLIM_INFINITY
                else min(int(soft), limit_bytes)
            )
            resource.setrlimit(res, (min(bounded_soft, bounded_hard), bounded_hard))
        except Exception:
            continue


def _run_child_runner(environ: Mapping[str, str]) -> int:
    try:
        command = _load_json_string_list(environ, INTERNAL_CHILD_COMMAND_ENV)
        raw_limit = environ.get(INTERNAL_CHILD_RLIMIT_KB_ENV, "0")
        try:
            limit_kb = int(raw_limit)
        except ValueError as exc:
            raise ValueError(f"{INTERNAL_CHILD_RLIMIT_KB_ENV} must be an int") from exc
    except ValueError as exc:
        print(f"memory_guard child_runner: {exc}", file=sys.stderr)
        return 2
    _apply_child_resource_limit(limit_kb)
    child_env = _child_env_without_internal_keys(environ)
    _write_child_started_timestamp(environ)
    if _is_windows_process_model():
        try:
            completed = subprocess.run(
                command,
                env=child_env,
                check=False,
                **_guarded_popen_process_isolation_kwargs(),
            )
        except OSError as exc:
            print(f"memory_guard child_runner: spawn failed: {exc}", file=sys.stderr)
            return 127
        return completed.returncode
    try:
        os.execvpe(command[0], command, child_env)
    except OSError as exc:
        print(f"memory_guard child_runner: exec failed: {exc}", file=sys.stderr)
        return 127
    return 127


def _write_child_started_timestamp(environ: Mapping[str, str]) -> None:
    raw_fd = environ.get(INTERNAL_CHILD_STARTED_FD_ENV)
    if not raw_fd:
        return
    try:
        fd = int(raw_fd)
    except ValueError:
        return
    try:
        os.write(fd, f"{time.monotonic_ns()}\n".encode("ascii"))
    except OSError:
        pass
    with contextlib.suppress(OSError):
        os.close(fd)


def _write_child_started_fd(fd: int | None) -> None:
    if fd is None:
        return
    try:
        os.write(fd, f"{time.monotonic_ns()}\n".encode("ascii"))
    except OSError:
        pass
    with contextlib.suppress(OSError):
        os.close(fd)


def _child_runner_env(
    environ: Mapping[str, str],
    command: Sequence[str],
    *,
    child_rlimit_kb: int,
    child_started_fd: int | None = None,
) -> dict[str, str]:
    runner_env = dict(environ)
    runner_env[INTERNAL_CHILD_RUNNER_ENV] = "1"
    runner_env[INTERNAL_CHILD_COMMAND_ENV] = json.dumps(list(command))
    runner_env[INTERNAL_CHILD_RLIMIT_KB_ENV] = str(child_rlimit_kb)
    if child_started_fd is not None:
        runner_env[INTERNAL_CHILD_STARTED_FD_ENV] = str(child_started_fd)
    return runner_env


def _resolve_relative_executable(command: Sequence[str]) -> list[str]:
    """Resolve a relative, path-bearing ``command[0]`` against the PARENT cwd.

    POSIX ``subprocess.Popen``/``os.execvpe`` with ``cwd=`` set exec a relative
    executable that contains a path separator (e.g. ``.venv/bin/python3``)
    relative to the CHILD's working directory, not the parent's. When the guard
    is asked to run such a command with a differing ``cwd=``, the relative
    interpreter is silently mis-resolved and the child fails with
    ``FileNotFoundError``. Resolve it deterministically against the launcher's
    (parent's) cwd before spawn so the guarded subprocess execs the correct
    binary regardless of ``cwd=``.

    Bare program names (no path separator, e.g. ``python3``) are left untouched
    so normal PATH lookup still applies; absolute paths are returned as-is.
    Resolution is skipped when the resolved path does not exist, so a genuinely
    PATH-resolved or intentionally child-relative command is never clobbered.
    """
    if not command:
        return list(command)
    cmd0 = command[0]
    if not cmd0:
        return list(command)
    has_sep = os.sep in cmd0 or (os.altsep is not None and os.altsep in cmd0)
    if not has_sep:
        return list(command)
    candidate = Path(cmd0)
    if candidate.is_absolute():
        return list(command)
    resolved = (Path.cwd() / candidate).resolve(strict=False)
    if not resolved.exists():
        return list(command)
    return [str(resolved), *command[1:]]


def _guarded_launch(
    command: Sequence[str],
    env: Mapping[str, str] | None,
    *,
    child_rlimit_kb: int | None,
) -> GuardedLaunch:
    # Normalize a relative path-bearing executable against the parent cwd before
    # any spawn path (POSIX rlimit, POSIX no-rlimit, or the Windows child-runner
    # env encoding) so none of them mis-resolve it against the child's `cwd=`.
    command = _resolve_relative_executable(command)
    if child_rlimit_kb is None or child_rlimit_kb <= 0:
        return GuardedLaunch(command=list(command), env=env)
    base_env = os.environ if env is None else env
    started_read_fd: int | None = None
    started_write_fd: int | None = None
    pass_fds: tuple[int, ...] = ()
    close_fds: tuple[int, ...] = ()
    if os.name == "posix":
        started_read_fd, started_write_fd = os.pipe()
        pass_fds = (started_write_fd,)
        close_fds = (started_write_fd,)
        limit_kb = child_rlimit_kb

        def apply_posix_limits() -> None:
            _apply_child_resource_limit(limit_kb)
            _write_child_started_fd(started_write_fd)

        return GuardedLaunch(
            command=list(command),
            env=env,
            pass_fds=pass_fds,
            close_fds=close_fds,
            started_read_fd=started_read_fd,
            preexec_fn=apply_posix_limits,
        )
    return GuardedLaunch(
        command=[sys.executable, str(Path(__file__).resolve())],
        env=_child_runner_env(
            base_env,
            command,
            child_rlimit_kb=child_rlimit_kb,
            child_started_fd=started_write_fd,
        ),
        pass_fds=pass_fds,
        close_fds=close_fds,
        started_read_fd=started_read_fd,
    )


def _close_fds(fds: Sequence[int | None]) -> None:
    for fd in fds:
        if fd is None:
            continue
        with contextlib.suppress(OSError):
            os.close(fd)


def _guarded_popen_process_isolation_kwargs() -> dict[str, object]:
    if _is_windows_process_model():
        creationflags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
        return {"creationflags": creationflags} if creationflags else {}
    return {"start_new_session": True}


def _read_child_started_at(fd: int | None) -> float | None:
    if fd is None:
        return None
    try:
        raw = os.read(fd, 64)
    except OSError:
        return None
    finally:
        with contextlib.suppress(OSError):
            os.close(fd)
    try:
        return int(raw.strip()) / 1_000_000_000
    except ValueError:
        return None


def _cargo_interruption_reason(
    *,
    violation: RssViolation | None,
    timed_out: bool,
    termination_wait_expired: bool,
    orphaned_process_groups: tuple[int, ...],
    returncode: int | None,
) -> str | None:
    if violation is not None:
        return "rss_limit_exceeded"
    if termination_wait_expired:
        return "termination_wait_expired"
    if timed_out:
        return "timeout"
    if orphaned_process_groups:
        return "orphaned_processes_cleaned"
    if returncode is not None and _returncode_looks_signal(returncode):
        return "signal_exit"
    return None


WINDOWS_PROCESS_SIGNAL_EXIT_CODES = frozenset(
    code
    for code in (
        int(signal.SIGTERM),
        int(getattr(signal, "SIGBREAK", 0)),
    )
    if code > 0
)


def _returncode_signal_payload(returncode: int) -> dict[str, object] | None:
    conventional_shell_status = False
    if returncode < 0:
        signo = -returncode
    elif 129 <= returncode <= 192:
        signo = returncode - 128
        conventional_shell_status = True
    elif (
        _is_windows_process_model()
        and returncode in WINDOWS_PROCESS_SIGNAL_EXIT_CODES
    ):
        signo = returncode
    else:
        return None
    with contextlib.suppress(ValueError):
        signame = signal.Signals(signo).name
        return {
            "signal": signo,
            "name": signame,
            "conventional_shell_status": conventional_shell_status,
        }
    return {
        "signal": signo,
        "name": None,
        "conventional_shell_status": conventional_shell_status,
    }


def _returncode_looks_signal(returncode: int) -> bool:
    return _returncode_signal_payload(returncode) is not None


def _append_guard_message(
    output: str | bytes,
    message: str,
    *,
    text: bool,
) -> str | bytes:
    if text:
        if isinstance(output, bytes):
            output = output.decode("utf-8", errors="replace")
        return f"{output or ''}{message}"
    if isinstance(output, str):
        output = output.encode("utf-8", errors="replace")
    return bytes(output or b"") + message.encode("utf-8", errors="replace")


def run_guarded(
    command: Sequence[str],
    *,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
    poll_interval: float,
    sampler: Callable[[], Mapping[int, ProcessSample]] = sample_processes,
    capture_output: bool = True,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    samples_jsonl: str | None = None,
    samples_jsonl_max_bytes: int | None = None,
    stream: str = "",
    child_rlimit_kb: int | None = None,
    input: str | bytes | None = None,
    adaptive_budget_provider: Callable[[int], AdaptiveMemoryBudget] | None = None,
    dynamic_process_rss: bool = False,
    dynamic_total_rss: bool = False,
    cleanup_orphans: bool = True,
    progress_label: str | None = None,
    keepalive_interval: float | None = None,
    text: bool = True,
    encoding: str = "utf-8",
    errors: str = "replace",
) -> GuardResult:
    if not command:
        raise ValueError("command is required")
    if sampler is None:
        sampler = sample_processes
    if poll_interval <= 0:
        raise ValueError("poll interval must be greater than 0")
    if timeout is not None and timeout <= 0:
        raise ValueError("timeout must be greater than 0")
    if keepalive_interval is not None and keepalive_interval <= 0:
        keepalive_interval = None
    if text and isinstance(input, bytes):
        raise TypeError("bytes input requires text=False")
    if not text and isinstance(input, str):
        raise TypeError("str input requires text=True")
    child_env = dict(os.environ) if env is None else dict(env)
    child_env[ACTIVE_ENV] = "1"
    child_env[ACTIVE_GUARD_PID_ENV] = str(os.getpid())
    guard_token, guard_marker = _write_active_guard_marker(os.getpid())
    child_env[ACTIVE_GUARD_TOKEN_ENV] = guard_token
    child_env[ACTIVE_GUARD_MARKER_ENV] = str(guard_marker)
    _inject_guard_memory_contract_env(
        child_env,
        max_rss_kb=max_rss_kb,
        child_rlimit_kb=child_rlimit_kb,
    )
    start = time.monotonic()
    baseline_pgids: frozenset[int] = frozenset()
    guard_signal: int | None = None

    def _handle_guard_signal(signum: int, _frame: object) -> None:
        nonlocal guard_signal
        if guard_signal is None:
            guard_signal = signum

    installed_signal_handlers: dict[int, object] = {}
    if threading.current_thread() is threading.main_thread():
        for maybe_signal in (
            getattr(signal, "SIGTERM", None),
            getattr(signal, "SIGINT", None),
            getattr(signal, "SIGHUP", None),
        ):
            if maybe_signal is None:
                continue
            with contextlib.suppress(ValueError, OSError):
                installed_signal_handlers[int(maybe_signal)] = signal.getsignal(
                    maybe_signal
                )
                signal.signal(maybe_signal, _handle_guard_signal)

    def _restore_guard_signal_handlers() -> None:
        for signum, previous_handler in installed_signal_handlers.items():
            with contextlib.suppress(ValueError, OSError):
                signal.signal(signum, previous_handler)

    proc: subprocess.Popen[Any] | None = None
    launch: GuardedLaunch | None = None
    child_process: GuardedChildProcess | None = None
    termination_reports: list[GuardTerminationReport] = []
    stdout_capture: Any = None
    stderr_capture: Any = None
    guard_interrupted = False
    try:
        launch = _guarded_launch(
            command,
            child_env,
            child_rlimit_kb=child_rlimit_kb,
        )
        if capture_output:
            if text:
                stdout_capture = tempfile.TemporaryFile(
                    mode="w+t",
                    encoding=encoding,
                    errors=errors,
                )
                stderr_capture = tempfile.TemporaryFile(
                    mode="w+t",
                    encoding=encoding,
                    errors=errors,
                )
            else:
                stdout_capture = tempfile.TemporaryFile(mode="w+b")
                stderr_capture = tempfile.TemporaryFile(mode="w+b")
        popen_kwargs: dict[str, object] = {
            "cwd": cwd,
            "env": dict(launch.env) if launch.env is not None else None,
            "stdout": stdout_capture if capture_output else None,
            "stderr": stderr_capture if capture_output else None,
            "stdin": subprocess.PIPE if input is not None else None,
            "text": text,
            **_guarded_popen_process_isolation_kwargs(),
        }
        if launch.pass_fds:
            popen_kwargs["pass_fds"] = launch.pass_fds
        if launch.preexec_fn is not None:
            popen_kwargs["preexec_fn"] = launch.preexec_fn
        try:
            proc = subprocess.Popen(launch.command, **popen_kwargs)
        except Exception:
            _close_fds((*launch.close_fds, launch.started_read_fd))
            if stdout_capture is not None:
                stdout_capture.close()
            if stderr_capture is not None:
                stderr_capture.close()
            raise
        _close_fds(launch.close_fds)
        child_process = GuardedChildProcess(
            pid=proc.pid,
            pgid=_safe_getpgid(proc.pid),
            sid=_safe_getsid(proc.pid),
            command=tuple(launch.command),
            started_at=_utc_timestamp(),
        )

        def terminate_owned_tree(
            *,
            reason: str,
            samples: Mapping[int, ProcessSample] | None = None,
            watched: set[int] | None = None,
            grace: float,
        ) -> None:
            termination_reports.append(
                terminate_watched_processes(
                    proc.pid,
                    samples=samples,
                    watched=watched,
                    grace=grace,
                    reason=reason,
                    sampler=sampler,
                )
            )

        stdin_thread: threading.Thread | None = None
        if input is not None and proc.stdin is not None:
            stdin_handle = proc.stdin
            proc.stdin = None

            def _feed_stdin() -> None:
                try:
                    stdin_handle.write(input)
                    stdin_handle.close()
                except (BrokenPipeError, OSError, ValueError):
                    with contextlib.suppress(OSError, ValueError):
                        stdin_handle.close()

            stdin_thread = threading.Thread(
                target=_feed_stdin,
                name="memory-guard-stdin-feeder",
                daemon=True,
            )
            stdin_thread.start()
        violation: RssViolation | None = None
        limit_at_violation: ResolvedMemoryLimits | None = None
        peak: RssViolation | None = None
        peak_total: RssViolation | None = None
        timed_out = False
        tracker = ProcessTreeTracker(proc.pid)
        child_exit_usage: ChildExitResourceUsage | None = None
        last_limits: ResolvedMemoryLimits | None = None
        termination_wait_expired = False
        termination_wait_s = termination_wait_seconds(env)
        remembered_samples: Mapping[int, ProcessSample] | None = None
        remembered_watched: set[int] | None = None
        saw_cargo_build_state = _command_invokes_cargo_build_state(command)
        next_keepalive = (
            start + keepalive_interval
            if progress_label is not None and keepalive_interval is not None
            else None
        )

        def terminate_after_sampling_failure(*, reason: str) -> None:
            if remembered_samples is not None and remembered_watched is not None:
                terminate_owned_tree(
                    reason=reason,
                    samples=remembered_samples,
                    watched=remembered_watched,
                    grace=0.0,
                )
                return
            termination_reports.append(
                terminate_watched_processes(
                    proc.pid,
                    grace=0.0,
                    reason=reason,
                    sampler=sample_processes,
                    root_owned=True,
                )
            )

        def sample_tracked_tree() -> tuple[Mapping[int, ProcessSample], set[int]]:
            nonlocal guard_interrupted, remembered_samples, remembered_watched
            try:
                samples = sampler()
            except KeyboardInterrupt:
                guard_interrupted = True
                terminate_after_sampling_failure(reason="guard_interrupted")
                with contextlib.suppress(subprocess.TimeoutExpired):
                    proc.wait(timeout=termination_wait_s)
                return remembered_samples or {}, set(remembered_watched or ())
            except Exception:
                terminate_after_sampling_failure(reason="sampler_failure")
                raise
            watched = tracker.update(samples)
            remembered_samples = samples
            remembered_watched = set(watched)
            return samples, watched

        if cleanup_orphans:
            baseline_samples, _baseline_watched = sample_tracked_tree()
            if not guard_interrupted:
                baseline_pgids = _live_process_group_ids(baseline_samples)

        while not guard_interrupted:
            now = time.monotonic()
            if guard_signal is not None:
                samples, watched = sample_tracked_tree()
                if guard_interrupted:
                    break
                saw_cargo_build_state = (
                    saw_cargo_build_state
                    or _samples_include_cargo_build_state(samples, watched)
                )
                terminate_owned_tree(
                    reason="guard_signal",
                    samples=samples,
                    watched=watched,
                    grace=0.0,
                )
                try:
                    proc.wait(timeout=termination_wait_s)
                except subprocess.TimeoutExpired:
                    termination_wait_expired = True
                break
            if timeout is not None and now - start >= timeout:
                timed_out = True
                samples, watched = sample_tracked_tree()
                if guard_interrupted:
                    break
                saw_cargo_build_state = (
                    saw_cargo_build_state
                    or _samples_include_cargo_build_state(samples, watched)
                )
                terminate_owned_tree(
                    reason="timeout",
                    samples=samples,
                    watched=watched,
                    grace=0.25,
                )
                break
            if next_keepalive is not None and now >= next_keepalive:
                timeout_text = "unbounded" if timeout is None else f"{timeout:.2f}s"
                print(
                    f"{progress_label}: still running "
                    f"elapsed={now - start:.0f}s timeout={timeout_text} pid={proc.pid}",
                    file=sys.stderr,
                    flush=True,
                )
                assert keepalive_interval is not None
                next_keepalive = now + keepalive_interval
            samples, watched = sample_tracked_tree()
            if guard_interrupted:
                break
            saw_cargo_build_state = (
                saw_cargo_build_state
                or _samples_include_cargo_build_state(samples, watched)
            )
            observed_peak = peak_rss(samples, root_pid=proc.pid, watched=watched)
            if observed_peak is not None and (
                peak is None or observed_peak.rss_kb > peak.rss_kb
            ):
                peak = observed_peak
            observed_total = total_rss(samples, root_pid=proc.pid, watched=watched)
            if observed_total is not None and (
                peak_total is None or observed_total.rss_kb > peak_total.rss_kb
            ):
                peak_total = observed_total
            current_limits = resolve_memory_limits(
                max_process_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                adaptive_budget_provider=adaptive_budget_provider,
                dynamic_process_rss=dynamic_process_rss,
                dynamic_total_rss=dynamic_total_rss,
                accounted_rss_kb=0 if observed_total is None else observed_total.rss_kb,
            )
            last_limits = current_limits
            violation = find_rss_violation(
                samples,
                root_pid=proc.pid,
                max_rss_kb=current_limits.max_process_rss_kb,
                max_total_rss_kb=current_limits.max_total_rss_kb,
                watched=watched,
            )
            if violation is not None:
                limit_at_violation = current_limits
                _record_sample(
                    root_pid=proc.pid,
                    peak=observed_peak,
                    total=observed_total,
                    violation=violation,
                    limits=current_limits,
                    samples_jsonl=samples_jsonl,
                    samples_jsonl_max_bytes=samples_jsonl_max_bytes,
                    stream=stream,
                )
                terminate_owned_tree(
                    reason="rss_limit",
                    samples=samples,
                    watched=watched,
                    grace=0.25,
                )
                break
            if samples_jsonl is not None or stream:
                _record_sample(
                    root_pid=proc.pid,
                    peak=observed_peak,
                    total=observed_total,
                    violation=None,
                    limits=current_limits,
                    samples_jsonl=samples_jsonl,
                    samples_jsonl_max_bytes=samples_jsonl_max_bytes,
                    stream=stream,
                )
            exited_usage = _poll_wait4_child(proc)
            if exited_usage is not None:
                child_exit_usage = exited_usage
                break
            if os.name != "posix" and proc.poll() is not None:
                break
            elapsed = time.monotonic() - start
            wait_timeout = (
                min(poll_interval, DEFAULT_FAST_START_POLL_INTERVAL_SEC)
                if elapsed < DEFAULT_FAST_START_DURATION_SEC
                else poll_interval
            )
            if timeout is not None:
                remaining = timeout - elapsed
                wait_timeout = max(0.0, min(wait_timeout, remaining))
            if os.name == "posix" and hasattr(os, "wait4"):
                time.sleep(wait_timeout)
                exited_usage = _poll_wait4_child(proc)
                if exited_usage is not None:
                    child_exit_usage = exited_usage
                    break
            else:
                try:
                    proc.wait(timeout=wait_timeout)
                    break
                except subprocess.TimeoutExpired:
                    pass
        finished = time.monotonic()
        if violation is None and child_exit_usage is not None:
            current_limits = last_limits or resolve_memory_limits(
                max_process_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                adaptive_budget_provider=adaptive_budget_provider,
                dynamic_process_rss=dynamic_process_rss,
                dynamic_total_rss=dynamic_total_rss,
                accounted_rss_kb=0,
            )
            rusage_peak = RssViolation(
                pid=proc.pid,
                rss_kb=child_exit_usage.max_rss_kb,
                command=" ".join(command),
                scope="process_rusage",
            )
            if rusage_peak.rss_kb > 0 and (
                peak is None or rusage_peak.rss_kb > peak.rss_kb
            ):
                peak = rusage_peak
            if rusage_peak.rss_kb > 0 and (
                peak_total is None or rusage_peak.rss_kb > peak_total.rss_kb
            ):
                peak_total = RssViolation(
                    pid=proc.pid,
                    rss_kb=rusage_peak.rss_kb,
                    command="process tree aggregate from direct child rusage",
                    scope="process_tree_rusage",
                )
            if child_exit_usage.max_rss_kb > current_limits.max_process_rss_kb:
                violation = rusage_peak
                limit_at_violation = current_limits
        stdout: str | bytes = "" if text else b""
        stderr: str | bytes = "" if text else b""
        orphaned_process_groups: tuple[int, ...] = ()
        try:
            if proc.returncode is None and not guard_interrupted:
                try:
                    proc.wait(timeout=max(1.0, poll_interval * 4.0))
                except subprocess.TimeoutExpired:
                    samples = sampler()
                    watched = tracker.update(samples)
                    terminate_owned_tree(
                        reason="post_loop_unreaped_child",
                        samples=samples,
                        watched=watched,
                        grace=0.0,
                    )
                    try:
                        proc.wait(timeout=termination_wait_s)
                    except subprocess.TimeoutExpired:
                        termination_wait_expired = True
            if cleanup_orphans and not guard_interrupted:
                tracked_orphans = cleanup_tracked_orphans(
                    proc.pid,
                    tracker=tracker,
                    sampler=sampler,
                    grace=0.25,
                )
                repo_orphans = cleanup_repo_scoped_orphans_since_baseline(
                    baseline_pgids=baseline_pgids,
                    tracker=tracker,
                    sampler=sampler,
                    grace=0.25,
                )
                termination_reports.extend(tracked_orphans.termination_reports)
                termination_reports.extend(repo_orphans.termination_reports)
                orphaned_process_groups = tuple(
                    sorted(
                        {
                            *tracked_orphans.process_groups,
                            *repo_orphans.process_groups,
                        }
                    )
                )
            if stdin_thread is not None:
                stdin_thread.join(timeout=1.0)
            if stdout_capture is not None:
                stdout_capture.seek(0)
                stdout = stdout_capture.read()
            if stderr_capture is not None:
                stderr_capture.seek(0)
                stderr = stderr_capture.read()
        finally:
            if stdout_capture is not None:
                stdout_capture.close()
            if stderr_capture is not None:
                stderr_capture.close()
        child_started = _read_child_started_at(launch.started_read_fd)
        elapsed_start = child_started if child_started is not None else start
        elapsed_s = max(0.0, finished - elapsed_start)
        returncode = proc.returncode
        if violation is not None:
            returncode = GUARD_RETURN_CODE
        if timed_out:
            returncode = TIMEOUT_RETURN_CODE
            timeout_msg = f"memory_guard: timeout after {timeout:.2f}s\n"
            stderr = _append_guard_message(stderr, timeout_msg, text=text)
        if guard_signal is not None and violation is None and not timed_out:
            returncode = 128 + guard_signal
            signal_payload = _exit_signal_payload(returncode)
            signal_label = (
                signal_payload["name"]
                if signal_payload is not None and signal_payload["name"] is not None
                else f"signal {guard_signal}"
            )
            stderr = _append_guard_message(
                stderr,
                "memory_guard: received "
                f"{signal_label}; terminated tracked process tree before exiting\n",
                text=text,
            )
        if guard_interrupted:
            returncode = GUARD_RETURN_CODE
            stderr = _append_guard_message(
                stderr,
                "memory_guard: interrupted; terminated tracked process tree "
                "before exiting\n",
                text=text,
            )
        if termination_wait_expired:
            if returncode is None:
                returncode = TIMEOUT_RETURN_CODE if timed_out else GUARD_RETURN_CODE
            stderr = _append_guard_message(
                stderr,
                "memory_guard: termination wait expired; tracked process tree did "
                "not fully exit after SIGTERM/SIGKILL: "
                f"observed_at={_utc_timestamp()} "
                f"elapsed={elapsed_s:.2f}s pid={proc.pid} wait={termination_wait_s:.2f}s\n"
                "memory_guard: next action: inspect host process state and child "
                "logs for uninterruptible work; the guard returned without waiting "
                "forever so CI can surface the failure instead of hanging.\n",
                text=text,
            )
        final_returncode = GUARD_RETURN_CODE if returncode is None else returncode
        cargo_incremental_quarantine: CargoIncrementalQuarantine | None = None
        cargo_interruption_reason = _cargo_interruption_reason(
            violation=violation,
            timed_out=timed_out,
            termination_wait_expired=termination_wait_expired,
            orphaned_process_groups=orphaned_process_groups,
            returncode=final_returncode,
        )
        if saw_cargo_build_state and cargo_interruption_reason is not None:
            effective_cwd = _effective_guard_cwd(cwd, child_env)
            cargo_incremental_quarantine = _quarantine_cargo_incremental_state(
                reason=cargo_interruption_reason,
                target_dir=_cargo_target_dir(child_env, effective_cwd),
                command=command,
                cwd=effective_cwd,
            )
            stderr = _append_guard_message(
                stderr,
                f"{_cargo_incremental_quarantine_message(cargo_incremental_quarantine)}\n",
                text=text,
            )
            if cargo_incremental_quarantine.errors:
                stderr = _append_guard_message(
                    stderr,
                    "memory_guard: cargo incremental quarantine errors: "
                    f"{'; '.join(cargo_incremental_quarantine.errors)}\n"
                    "memory_guard: next action: run `molt clean --apply "
                    "--kill-processes` if stale Cargo state still blocks rebuilds.\n",
                    text=text,
                )
        return GuardResult(
            returncode=final_returncode,
            violation=violation,
            peak=peak,
            peak_total=peak_total,
            stdout=stdout,
            stderr=stderr,
            timed_out=timed_out,
            elapsed_s=elapsed_s,
            limit_at_violation=limit_at_violation,
            orphaned_process_groups=orphaned_process_groups,
            cargo_incremental_quarantine=cargo_incremental_quarantine,
            guard_signal=guard_signal,
            child_process=child_process,
            termination_reports=tuple(termination_reports),
        )
    finally:
        if proc is not None and proc.poll() is None:
            with contextlib.suppress(Exception):
                termination_reports.append(
                    terminate_watched_processes(
                        proc.pid,
                        grace=0.0,
                        reason="run_guarded_finalizer",
                        sampler=sample_processes if guard_interrupted else sampler,
                    )
                )
            with contextlib.suppress(Exception):
                proc.wait(timeout=termination_wait_seconds(env))
        if stdout_capture is not None and not getattr(stdout_capture, "closed", False):
            stdout_capture.close()
        if stderr_capture is not None and not getattr(stderr_capture, "closed", False):
            stderr_capture.close()
        if launch is not None:
            _close_fds((launch.started_read_fd,))
        _restore_guard_signal_handlers()


_REPRO_ENV_KEYS = {
    "CARGO_TARGET_DIR",
    "CI",
    "CODEX_SESSION_ID",
    "CODEX_WORKSPACE",
    "MOLT_CACHE",
    "MOLT_DIFF_CARGO_TARGET_DIR",
    "MOLT_DIFF_ROOT",
    "MOLT_EXT_ROOT",
    "MOLT_GUARD_PROFILE",
    "MOLT_GUARD_PROFILE_LOG",
    "MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC",
    "MOLT_PREFER_EXTERNAL_ARTIFACTS",
    "MOLT_SESSION_ID",
    "PYTEST_CURRENT_TEST",
    "PYTEST_XDIST_WORKER",
    "PYTHONHASHSEED",
    "PYTHONPATH",
    "RUSTFLAGS",
    "RUSTC_WRAPPER",
    "TMPDIR",
    "UV_CACHE_DIR",
    "VIRTUAL_ENV",
}
_REPRO_ENV_PREFIXES = ("CODEX_", "GITHUB_", "MOLT_", "PYTEST_")
_SECRET_ENV_TOKENS = (
    "AUTH",
    "COOKIE",
    "CREDENTIAL",
    "KEY",
    "PASS",
    "SECRET",
    "TOKEN",
)
_PYTEST_CURRENT_TEST_FILE_ENV = "MOLT_PYTEST_CURRENT_TEST_FILE"
_PYTEST_CURRENT_TEST_FILE_MAX_BYTES = 16 * 1024
_PYTEST_CURRENT_TEST_WORKER_MAX_FILES = 128
_PYTEST_COMMAND_NAMES = frozenset({"pytest", "py.test", "pytest.exe", "py.test.exe"})


def _safe_repro_env_key(key: str) -> bool:
    upper = key.upper()
    if any(token in upper for token in _SECRET_ENV_TOKENS):
        return False
    return key in _REPRO_ENV_KEYS or any(
        key.startswith(prefix) for prefix in _REPRO_ENV_PREFIXES
    )


def _safe_repro_env_value(value: object) -> str:
    text = str(value)
    return text if len(text) <= 512 else f"{text[:512]}...<truncated>"


def _safe_repro_env(environ: Mapping[str, str]) -> dict[str, str]:
    payload: dict[str, str] = {}
    for key in sorted(environ):
        if not _safe_repro_env_key(key):
            continue
        payload[key] = _safe_repro_env_value(environ.get(key, ""))
    return payload


def _safe_repro_env_delta(
    environ: Mapping[str, str],
    *,
    baseline: Mapping[str, str] | None = None,
) -> dict[str, object]:
    base = os.environ if baseline is None else baseline
    added: dict[str, str] = {}
    changed: dict[str, dict[str, str]] = {}
    removed: list[str] = []
    for key in sorted(set(base) | set(environ)):
        if not _safe_repro_env_key(key):
            continue
        in_base = key in base
        in_env = key in environ
        if in_env and not in_base:
            added[key] = _safe_repro_env_value(environ[key])
        elif in_base and not in_env:
            removed.append(key)
        elif in_base and in_env and base[key] != environ[key]:
            changed[key] = {
                "from": _safe_repro_env_value(base[key]),
                "to": _safe_repro_env_value(environ[key]),
            }
    return {
        "baseline": "guard_parent_environment",
        "added": added,
        "changed": changed,
        "removed": removed,
    }


def _safe_getpgrp() -> int | None:
    if os.name != "posix":
        return None
    try:
        return os.getpgrp()
    except OSError:
        return None


def _safe_getpgid(pid: int) -> int | None:
    if os.name != "posix":
        return None
    try:
        return os.getpgid(pid)
    except OSError:
        return None


def _safe_getsid(pid: int) -> int | None:
    if os.name != "posix":
        return None
    try:
        return os.getsid(pid)
    except OSError:
        return None


def _process_sample_payload(sample: ProcessSample) -> dict[str, object]:
    return {
        "pid": sample.pid,
        "ppid": sample.ppid,
        "pgid": sample.pgid,
        "rss_kb": sample.rss_kb,
        "elapsed_sec": sample.elapsed_sec,
        "command": sample.command,
    }


def process_sample_payload(sample: ProcessSample) -> dict[str, object]:
    return _process_sample_payload(sample)


def _bounded_process_sample_payload(
    sample: ProcessSample,
    *,
    max_command_chars: int = 512,
) -> dict[str, object]:
    payload = _process_sample_payload(sample)
    command = str(payload["command"])
    if len(command) > max_command_chars:
        payload["command"] = f"{command[:max_command_chars]}...<truncated>"
    return payload


def _host_control_plane_payload(
    samples: Mapping[int, ProcessSample],
    *,
    max_samples: int = 32,
) -> dict[str, object] | None:
    host_pgids = {
        _sample_pgid(sample)
        for sample in samples.values()
        if is_host_control_plane_process(sample)
    }
    protected_pgids = _current_protected_process_group_ids(samples)
    if not host_pgids and not protected_pgids:
        return None
    host_samples = [
        sample
        for sample in sorted(samples.values(), key=lambda item: item.pid)
        if _sample_pgid(sample) in host_pgids
    ]
    payload: dict[str, object] = {
        "protected_pgids": sorted(protected_pgids),
        "host_pgids": sorted(host_pgids),
        "samples": [
            _bounded_process_sample_payload(sample)
            for sample in host_samples[:max_samples]
        ],
    }
    if len(host_samples) > max_samples:
        payload["truncated_samples"] = len(host_samples) - max_samples
    return payload


def _process_lineage_payload(
    samples: Mapping[int, ProcessSample],
    *,
    pid: int,
    max_depth: int = 8,
) -> list[dict[str, object]]:
    lineage: list[dict[str, object]] = []
    seen: set[int] = set()
    current = pid
    for _ in range(max_depth):
        if current <= 0 or current in seen:
            break
        seen.add(current)
        sample = samples.get(current)
        if sample is None:
            lineage.append({"pid": current, "sample_missing": True})
            break
        lineage.append(_process_sample_payload(sample))
        if sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return lineage


def _path_is_under(path: Path, root: Path) -> bool:
    try:
        path.resolve(strict=False).relative_to(root.resolve(strict=False))
    except ValueError:
        return False
    return True


def _pytest_custody_artifact_path(
    kind: str,
    suffix: str,
    *,
    pid: int | None = None,
) -> Path:
    safe_kind = "".join(ch if ch.isalnum() else "-" for ch in kind.lower()).strip("-")
    safe_suffix = "".join(ch if ch.isalnum() else "-" for ch in suffix.lower()).strip(
        "-"
    )
    return PYTEST_OUTER_GUARD_SUMMARY_DIR / (
        f"{safe_kind or 'pytest'}-{os.getpid() if pid is None else pid}_"
        f"{safe_suffix}.json"
    )


def _canonical_pytest_current_test_file_path(raw_path: str | None = None) -> Path:
    path = Path(raw_path).expanduser() if raw_path else None
    if path is None:
        return _pytest_custody_artifact_path("test-custody", "current-test")
    if not path.is_absolute():
        path = ROOT / path
    path = path.resolve(strict=False)
    if not _path_is_under(path, PYTEST_OUTER_GUARD_SUMMARY_DIR):
        return _pytest_custody_artifact_path("test-custody", "current-test")
    return path


def _looks_like_repo_test_path(raw: str, cwd: str | Path | None) -> bool:
    if not raw or raw == "-" or raw.startswith("-") or not raw.endswith(".py"):
        return False
    path = Path(raw).expanduser()
    if not path.is_absolute():
        root = Path.cwd() if cwd is None else Path(cwd).expanduser()
        path = root / path
    try:
        path.resolve(strict=False).relative_to((ROOT / "tests").resolve(strict=False))
    except ValueError:
        return False
    return True


def _command_requests_test_custody(
    command: Sequence[str],
    *,
    cwd: str | Path | None = None,
) -> bool:
    args = tuple(str(arg) for arg in command)
    for idx, arg in enumerate(args):
        if Path(arg).name in _PYTEST_COMMAND_NAMES:
            return True
        if _looks_like_repo_test_path(arg, cwd):
            return True
        if arg == "-m" and idx + 1 < len(args):
            module = args[idx + 1]
            if module == "pytest" or module == "tests" or module.startswith("tests."):
                return True
    return False


def test_custody_launch_env(
    command: Sequence[str],
    *,
    environ: Mapping[str, str] | None = None,
    cwd: str | Path | None = None,
) -> dict[str, str]:
    env = dict(os.environ if environ is None else environ)
    if not _command_requests_test_custody(command, cwd=cwd):
        return env
    PYTEST_OUTER_GUARD_SUMMARY_DIR.mkdir(parents=True, exist_ok=True)
    env[_PYTEST_CURRENT_TEST_FILE_ENV] = str(
        _canonical_pytest_current_test_file_path(env.get(_PYTEST_CURRENT_TEST_FILE_ENV))
    )
    return env


def _read_pytest_current_test_json(path: Path) -> dict[str, object]:
    payload: dict[str, object] = {"path": str(path)}
    try:
        data = path.read_bytes()
    except FileNotFoundError:
        payload["missing"] = True
        return payload
    except OSError as exc:
        payload["read_error"] = str(exc)
        return payload
    if len(data) > _PYTEST_CURRENT_TEST_FILE_MAX_BYTES:
        payload["truncated"] = True
        data = data[:_PYTEST_CURRENT_TEST_FILE_MAX_BYTES]
    try:
        text = data.decode("utf-8", errors="replace")
    except Exception as exc:
        payload["decode_error"] = str(exc)
        return payload
    try:
        decoded = json.loads(text)
    except json.JSONDecodeError:
        payload["raw"] = text[:_PYTEST_CURRENT_TEST_FILE_MAX_BYTES]
    else:
        payload["payload"] = decoded
    return payload


def _lineage_pid_set(
    samples: Mapping[int, ProcessSample],
    *,
    pid: int,
    max_depth: int = 16,
) -> set[int]:
    lineage: set[int] = set()
    seen: set[int] = set()
    current = pid
    for _ in range(max_depth):
        if current <= 0 or current in seen:
            break
        seen.add(current)
        lineage.add(current)
        sample = samples.get(current)
        if sample is None or sample.ppid <= 0 or sample.ppid == current:
            break
        current = sample.ppid
    return lineage


def _pytest_worker_record_payloads(
    aggregate_path: Path,
    *,
    samples: Mapping[int, ProcessSample],
    incident_pid: int | None,
) -> list[dict[str, object]]:
    worker_dir = aggregate_path.with_name(f"{aggregate_path.name}.d")
    try:
        paths = sorted(
            (path for path in worker_dir.glob("*.json") if path.is_file()),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
    except OSError:
        return []
    incident_lineage = (
        _lineage_pid_set(samples, pid=incident_pid)
        if incident_pid is not None
        else set()
    )
    records: list[dict[str, object]] = []
    for path in paths[:_PYTEST_CURRENT_TEST_WORKER_MAX_FILES]:
        record = _read_pytest_current_test_json(path)
        decoded = record.get("payload")
        if isinstance(decoded, dict) and incident_lineage:
            try:
                record_pid = int(decoded.get("pid", 0) or 0)
            except (TypeError, ValueError):
                record_pid = 0
            if record_pid in incident_lineage:
                record["incident_match"] = "pid_lineage"
        records.append(record)
    if len(paths) > _PYTEST_CURRENT_TEST_WORKER_MAX_FILES:
        records.append(
            {
                "truncated_worker_records": len(paths)
                - _PYTEST_CURRENT_TEST_WORKER_MAX_FILES
            }
        )
    return records


def _pytest_current_test_file_payload(
    environ: Mapping[str, str],
    *,
    samples: Mapping[int, ProcessSample],
    incident_pid: int | None = None,
) -> dict[str, object] | None:
    raw_path = environ.get(_PYTEST_CURRENT_TEST_FILE_ENV, "").strip()
    if not raw_path:
        return None
    path = Path(raw_path).expanduser()
    if not path.is_absolute():
        path = ROOT / path
    path = path.resolve(strict=False)
    if not _path_is_under(path, PYTEST_OUTER_GUARD_SUMMARY_DIR):
        return {
            "path": str(path),
            "rejected": "noncanonical",
            "canonical_root": str(PYTEST_OUTER_GUARD_SUMMARY_DIR),
        }
    payload = _read_pytest_current_test_json(path)
    worker_records = _pytest_worker_record_payloads(
        path,
        samples=samples,
        incident_pid=incident_pid,
    )
    if worker_records:
        payload["worker_records"] = worker_records
    return payload


def repro_context_payload(
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    environ: Mapping[str, str] | None = None,
    max_process_rss_kb: int | None = None,
    max_total_rss_kb: int | None = None,
    max_global_rss_kb: int | None = None,
    child_rlimit_kb: int | None = None,
    timeout_s: float | None = None,
    poll_interval_s: float | None = None,
    summary_json: str | None = None,
    incident_pid: int | None = None,
) -> dict[str, object]:
    source = os.environ if environ is None else environ
    cwd_path = Path.cwd() if cwd is None else Path(cwd).expanduser()
    samples = sample_processes()
    pid = os.getpid()
    parent_pid = os.getppid()
    pytest_payload: dict[str, object] = {
        "current_test": source.get("PYTEST_CURRENT_TEST", ""),
        "xdist_worker": source.get("PYTEST_XDIST_WORKER", ""),
    }
    current_test_file = _pytest_current_test_file_payload(
        source,
        samples=samples,
        incident_pid=incident_pid,
    )
    if current_test_file is not None:
        pytest_payload["current_test_file"] = current_test_file
    payload: dict[str, object] = {
        "command": list(command),
        "cwd": str(cwd_path.resolve(strict=False)),
        "env": _safe_repro_env(source),
        "env_delta": _safe_repro_env_delta(source),
        "guard_process": {
            "pid": pid,
            "ppid": parent_pid,
            "pgid": _safe_getpgrp(),
            "sid": _safe_getsid(0),
            "argv": list(sys.argv),
        },
        "host": {
            "python_executable": sys.executable,
            "python_version": sys.version.split()[0],
            "platform": sys.platform,
            "platform_detail": platform.platform(),
            "machine": platform.machine(),
        },
        "limits": {
            "max_process_rss_kb": max_process_rss_kb,
            "max_process_rss_gb": (
                None
                if max_process_rss_kb is None
                else max_process_rss_kb / (1024 * 1024)
            ),
            "max_total_rss_kb": max_total_rss_kb,
            "max_total_rss_gb": (
                None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
            ),
            "max_global_rss_kb": max_global_rss_kb,
            "max_global_rss_gb": (
                None if max_global_rss_kb is None else max_global_rss_kb / (1024 * 1024)
            ),
            "child_rlimit_kb": child_rlimit_kb,
            "child_rlimit_gb": (
                None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
            ),
            "timeout_s": timeout_s,
            "poll_interval_s": poll_interval_s,
        },
        "parent_lineage": _process_lineage_payload(samples, pid=pid),
        "pytest": pytest_payload,
    }
    host_control_plane = _host_control_plane_payload(samples)
    if host_control_plane is not None:
        payload["host_control_plane"] = host_control_plane
    if summary_json:
        payload["summary_json"] = str(Path(summary_json).expanduser())
    parent_sample = samples.get(parent_pid)
    if parent_sample is not None:
        payload["parent_process"] = _process_sample_payload(parent_sample)
    else:
        payload["parent_process"] = {
            "pid": parent_pid,
            "pgid": _safe_getpgid(parent_pid),
            "sample_missing": True,
        }
    return payload


def repro_context_line(payload: Mapping[str, object]) -> str:
    return json.dumps(payload, sort_keys=True, separators=(",", ":"))


def exit_signal_payload(returncode: int) -> dict[str, object] | None:
    return _returncode_signal_payload(returncode)


_exit_signal_payload = exit_signal_payload


def _elapsed_text(elapsed_s: float | None) -> str:
    return "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"


def _limit_text(limit_gb: float | None) -> str:
    return "unknown" if limit_gb is None else f"{limit_gb:.2f}GB"


def _child_identity_text(child: GuardedChildProcess | None) -> str:
    if child is None:
        return "child_pid=unknown child_pgid=unknown child_sid=unknown"
    return f"child_pid={child.pid} child_pgid={child.pgid} child_sid={child.sid}"


def _incident_payload(result: GuardResult) -> dict[str, object] | None:
    def attach_guard_custody(payload: dict[str, object]) -> dict[str, object]:
        child_payload = guarded_child_process_payload(result.child_process)
        if child_payload is not None:
            payload["child_process"] = child_payload
        if result.termination_reports:
            payload["termination_reports"] = termination_reports_payload(
                result.termination_reports
            )
        return payload

    guard_signal_payload = (
        None
        if result.guard_signal is None
        else _exit_signal_payload(128 + result.guard_signal)
    )
    if (
        result.guard_signal is not None
        and result.violation is None
        and not result.timed_out
    ):
        payload: dict[str, object] = {
            "reason": "guard_interrupted",
            "cleanup": (
                "terminated tracked process tree and post-baseline Molt process groups"
                if result.orphaned_process_groups
                else "terminated tracked process tree"
            ),
            "recorded_at": _utc_timestamp(),
            "elapsed_s": result.elapsed_s,
            "signal": guard_signal_payload,
            "next_action": (
                "Inspect the parent host/control-plane signal source and child "
                "logs; the guard parent received the signal and wrote this "
                "summary before exiting."
            ),
        }
        if result.orphaned_process_groups:
            payload["process_groups"] = list(result.orphaned_process_groups)
        return attach_guard_custody(payload)
    if result.violation is not None:
        cleanup = (
            "classified command as failed from child exit resource usage"
            if result.violation.scope == "process_rusage"
            else "terminated tracked process tree"
        )
        payload: dict[str, object] = {
            "reason": "rss_limit_exceeded",
            "cleanup": cleanup,
            "recorded_at": _utc_timestamp(),
            "elapsed_s": result.elapsed_s,
            "next_action": (
                "Inspect child logs and allocations, lower parallelism/input size, "
                "or raise the relevant memory guard RSS limit if the workload is "
                "expected."
            ),
        }
        if guard_signal_payload is not None:
            payload["guard_signal"] = guard_signal_payload
        return attach_guard_custody(payload)
    if result.timed_out:
        payload: dict[str, object] = {
            "reason": "timeout",
            "cleanup": (
                "terminated tracked process tree and post-baseline Molt process groups"
                if result.orphaned_process_groups
                else "terminated tracked process tree"
            ),
            "recorded_at": _utc_timestamp(),
            "elapsed_s": result.elapsed_s,
            "next_action": (
                "Inspect child logs for a hang or oversized workload; raise the "
                "guard timeout only for intentional long-running work."
            ),
        }
        if result.orphaned_process_groups:
            payload["process_groups"] = list(result.orphaned_process_groups)
        if guard_signal_payload is not None:
            payload["guard_signal"] = guard_signal_payload
        return attach_guard_custody(payload)
    if result.orphaned_process_groups:
        return attach_guard_custody(
            {
                "reason": "orphaned_processes_cleaned",
                "cleanup": "terminated tracked orphan descendants; group ids recorded",
                "recorded_at": _utc_timestamp(),
                "elapsed_s": result.elapsed_s,
                "process_groups": list(result.orphaned_process_groups),
                "next_action": (
                    "Inspect child process lifecycle and logs; make helpers shut down "
                    "explicitly, or run intentional warm daemons inside a suite-level "
                    "sentinel that drains at scope exit."
                ),
            }
        )
    exit_signal = _exit_signal_payload(result.returncode)
    if exit_signal is not None:
        cleanup = (
            "quarantined Cargo incremental state"
            if result.cargo_incremental_quarantine is not None
            and result.cargo_incremental_quarantine.moved_paths
            else "none_by_guard"
        )
        return attach_guard_custody(
            {
                "reason": "signal_exit",
                "cleanup": cleanup,
                "recorded_at": _utc_timestamp(),
                "elapsed_s": result.elapsed_s,
                "signal": exit_signal,
                "next_action": (
                    "Inspect child stderr/logs or the host signal source; the memory "
                    "guard did not classify this as an RSS limit trip."
                ),
            }
        )
    return None


def _write_summary_json(
    path: str,
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    environ: Mapping[str, str],
    max_rss_kb: int,
    max_total_rss_kb: int | None,
    max_global_rss_kb: int | None,
    child_rlimit_kb: int | None,
    timeout_s: float | None,
    poll_interval_s: float,
    result: GuardResult,
) -> None:
    summary_path = Path(path)
    if summary_path.parent:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
    incident = _incident_payload(result)
    payload = {
        "command": list(command),
        "returncode": result.returncode,
        "elapsed_s": result.elapsed_s,
        "max_rss_kb": max_rss_kb,
        "max_rss_gb": max_rss_kb / (1024 * 1024),
        "max_total_rss_kb": max_total_rss_kb,
        "max_total_rss_gb": (
            None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
        ),
        "child_rlimit_kb": child_rlimit_kb,
        "child_rlimit_gb": (
            None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
        ),
        "violation": _rss_record_payload(result.violation),
        "peak": _rss_record_payload(result.peak),
        "peak_total": _rss_record_payload(result.peak_total),
        "timed_out": result.timed_out,
        "orphaned_process_groups": list(result.orphaned_process_groups),
        "child_process": guarded_child_process_payload(result.child_process),
        "termination_reports": termination_reports_payload(result.termination_reports),
        "cargo_incremental_quarantine": _cargo_incremental_quarantine_payload(
            result.cargo_incremental_quarantine
        ),
        "limit_at_violation": (
            None
            if result.limit_at_violation is None
            else memory_limits_payload(result.limit_at_violation)
        ),
        "exit_signal": (
            None
            if (
                result.violation is not None
                or result.timed_out
                or result.guard_signal is not None
            )
            else _exit_signal_payload(result.returncode)
        ),
        "guard_signal": (
            None
            if result.guard_signal is None
            else _exit_signal_payload(128 + result.guard_signal)
        ),
        "incident": incident,
    }
    if incident is not None:
        payload["repro"] = repro_context_payload(
            command=command,
            cwd=cwd,
            environ=environ,
            max_process_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
            max_global_rss_kb=max_global_rss_kb,
            child_rlimit_kb=child_rlimit_kb,
            timeout_s=timeout_s,
            poll_interval_s=poll_interval_s,
            summary_json=path,
            incident_pid=result.violation.pid if result.violation is not None else None,
        )
    summary_path.write_text(
        json.dumps(payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def _default_incident_summary_path() -> Path:
    stamp = _utc_compact_timestamp()
    return (
        ROOT / "tmp" / "memory_guard" / "incidents" / (f"{stamp}-pid{os.getpid()}.json")
    )


def _prune_default_incident_summaries(
    directory: Path,
    *,
    keep: int = DEFAULT_INCIDENT_SUMMARY_KEEP,
) -> None:
    if keep <= 0:
        return
    try:
        paths = sorted(
            (path for path in directory.glob("*.json") if path.is_file()),
            key=lambda path: path.stat().st_mtime,
            reverse=True,
        )
    except OSError:
        return
    for path in paths[keep:]:
        with contextlib.suppress(OSError):
            path.unlink()


def _write_running_summary_json(
    path: str,
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    environ: Mapping[str, str],
    max_rss_kb: int,
    max_total_rss_kb: int | None,
    max_global_rss_kb: int | None,
    child_rlimit_kb: int | None,
    timeout_s: float | None,
    poll_interval_s: float,
) -> None:
    summary_path = Path(path)
    if summary_path.parent:
        summary_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "command": list(command),
        "returncode": None,
        "recorded_at": _utc_timestamp(),
        "status": "running",
        "max_rss_kb": max_rss_kb,
        "max_rss_gb": max_rss_kb / (1024 * 1024),
        "max_total_rss_kb": max_total_rss_kb,
        "max_total_rss_gb": (
            None if max_total_rss_kb is None else max_total_rss_kb / (1024 * 1024)
        ),
        "child_rlimit_kb": child_rlimit_kb,
        "child_rlimit_gb": (
            None if child_rlimit_kb is None else child_rlimit_kb / (1024 * 1024)
        ),
        "violation": None,
        "peak": None,
        "peak_total": None,
        "timed_out": False,
        "orphaned_process_groups": [],
        "child_process": None,
        "termination_reports": [],
        "cargo_incremental_quarantine": None,
        "limit_at_violation": None,
        "exit_signal": None,
        "guard_signal": None,
        "incident": {
            "reason": "guard_started",
            "cleanup": "pending",
            "recorded_at": _utc_timestamp(),
            "next_action": (
                "If this file remains in running status, the guard parent was "
                "terminated before it could write the final summary; use the "
                "repro block and host/control-plane samples below."
            ),
        },
        "repro": repro_context_payload(
            command=command,
            cwd=cwd,
            environ=environ,
            max_process_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
            max_global_rss_kb=max_global_rss_kb,
            child_rlimit_kb=child_rlimit_kb,
            timeout_s=timeout_s,
            poll_interval_s=poll_interval_s,
            summary_json=path,
            incident_pid=None,
        ),
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
        "--max-process-rss-gb",
        dest="max_rss_gb",
        type=float,
        default=None,
        help=(
            "Abort if any child process exceeds this RSS; must be "
            f"<{DEFAULT_HARD_MAX_RSS_GB:g}GB "
            "(default: adaptive from live available memory)."
        ),
    )
    parser.add_argument(
        "--max-total-rss-gb",
        "--max-tree-rss-gb",
        "--max-group-rss-gb",
        dest="max_total_rss_gb",
        type=float,
        default=None,
        help=(
            "Abort if the watched process tree exceeds this aggregate RSS; "
            f"must be <{DEFAULT_HARD_MAX_RSS_GB:g}GB "
            "(default: adaptive from live available memory)."
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
    parser.add_argument(
        "--samples-jsonl",
        help="Append per-poll peak and process-tree RSS samples as JSONL.",
    )
    parser.add_argument(
        "--samples-max-mb",
        type=float,
        default=DEFAULT_SAMPLES_MAX_MB,
        help=(
            "Rotate --samples-jsonl after this many MB; set <=0 to disable "
            f"rotation (default: {DEFAULT_SAMPLES_MAX_MB})."
        ),
    )
    parser.add_argument(
        "--stream",
        choices=("stderr", "stdout", "json-stderr", "json-stdout"),
        default="",
        help="Emit per-poll guard samples to this stream without writing artifacts.",
    )
    parser.add_argument(
        "--child-rlimit-gb",
        type=float,
        default=None,
        help=(
            "Apply an OS resource limit to the direct guarded child before exec; "
            "defaults to an adaptive virtual-memory clamp distinct from RSS. "
            "Set <=0 to disable this layer."
        ),
    )
    parser.add_argument(
        "--timeout",
        type=float,
        help="Abort the command if wall-clock runtime exceeds this many seconds.",
    )
    parser.add_argument("command", nargs=argparse.REMAINDER)
    return parser


def _load_internal_command(environ: Mapping[str, str]) -> list[str] | None:
    if environ.get(INTERNAL_WORKER_ENV) != "1":
        return None
    return _load_json_string_list(environ, INTERNAL_COMMAND_ENV)


def _child_env_without_internal_keys(environ: Mapping[str, str]) -> dict[str, str]:
    child_env = dict(environ)
    for key in _INTERNAL_ENV_KEYS:
        child_env.pop(key, None)
    return child_env


def _worker_env(environ: Mapping[str, str], command: Sequence[str]) -> dict[str, str]:
    worker_env = dict(environ)
    worker_env[INTERNAL_COMMAND_ENV] = json.dumps(list(command))
    worker_env[INTERNAL_WORKER_ENV] = "1"
    return worker_env


def _worker_argv(args: argparse.Namespace) -> list[str]:
    worker_args = [
        sys.executable,
        str(Path(__file__).resolve()),
        "--poll-interval",
        str(args.poll_interval),
    ]
    if args.max_rss_gb is not None:
        worker_args.extend(["--max-rss-gb", str(args.max_rss_gb)])
    if args.max_total_rss_gb is not None:
        worker_args.extend(["--max-total-rss-gb", str(args.max_total_rss_gb)])
    if args.summary_json:
        worker_args.extend(["--summary-json", args.summary_json])
    if args.samples_jsonl:
        worker_args.extend(["--samples-jsonl", args.samples_jsonl])
        worker_args.extend(["--samples-max-mb", str(args.samples_max_mb)])
    if args.stream:
        worker_args.extend(["--stream", args.stream])
    if args.child_rlimit_gb is not None:
        worker_args.extend(["--child-rlimit-gb", str(args.child_rlimit_gb)])
    if args.timeout is not None:
        worker_args.extend(["--timeout", str(args.timeout)])
    return worker_args


def main(
    argv: Sequence[str] | None = None,
    *,
    hide_command_argv: bool = False,
    execve: Callable[[str, Sequence[str], Mapping[str, str]], object] = os.execve,
    environ: Mapping[str, str] | None = None,
) -> int:
    current_env = os.environ if environ is None else environ
    if current_env.get(INTERNAL_CHILD_RUNNER_ENV) == "1":
        return _run_child_runner(current_env)
    args = _parser().parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        try:
            internal_command = _load_internal_command(current_env)
        except ValueError as exc:
            print(f"memory_guard: {exc}", file=sys.stderr)
            return 2
        if internal_command is None:
            print("memory_guard: command is required", file=sys.stderr)
            return 2
        command = internal_command
    current_env = test_custody_launch_env(command, environ=current_env)
    try:
        budget = adaptive_memory_budget(environ=current_env)
        max_rss_gb = (
            budget.max_process_rss_gb
            if args.max_rss_gb is None
            else float(args.max_rss_gb)
        )
        max_total_rss_gb = (
            budget.max_total_rss_gb
            if args.max_total_rss_gb is None
            else float(args.max_total_rss_gb)
        )
        max_rss_kb = max_rss_kb_from_gb(max_rss_gb)
        max_total_rss_kb = max_rss_kb_from_gb(max_total_rss_gb)
        max_global_rss_kb = max_global_rss_kb_from_gb(budget.max_global_rss_gb)
        poll_interval = float(args.poll_interval)
        if poll_interval <= 0:
            raise ValueError("poll interval must be greater than 0")
        if args.timeout is not None and args.timeout <= 0:
            raise ValueError("timeout must be greater than 0")
        samples_jsonl_max_bytes = _samples_max_bytes_from_mb(args.samples_max_mb)
        child_rlimit_gb = (
            default_child_rlimit_gb(
                max_process_rss_gb=max_rss_gb,
                max_total_rss_gb=max_total_rss_gb,
                max_global_rss_gb=budget.max_global_rss_gb,
            )
            if args.child_rlimit_gb is None
            else float(args.child_rlimit_gb)
        )
        child_rlimit_kb = (
            None if child_rlimit_gb <= 0 else child_rlimit_kb_from_gb(child_rlimit_gb)
        )
        dynamic_process_rss = args.max_rss_gb is None
        dynamic_total_rss = args.max_total_rss_gb is None

        def adaptive_budget_provider(accounted_rss_kb: int) -> AdaptiveMemoryBudget:
            return adaptive_memory_budget(
                environ=current_env,
                accounted_rss_kb=accounted_rss_kb,
            )
    except ValueError as exc:
        print(f"memory_guard: {exc}", file=sys.stderr)
        return 2
    if hide_command_argv and current_env.get(INTERNAL_WORKER_ENV) != "1":
        worker_argv = _worker_argv(args)
        if _is_windows_process_model():
            completed = subprocess.run(
                worker_argv,
                env=_worker_env(current_env, command),
                check=False,
                **_guarded_popen_process_isolation_kwargs(),
            )
            return completed.returncode
        execve(
            sys.executable,
            worker_argv,
            _worker_env(current_env, command),
        )
        print("memory_guard: failed to exec internal worker", file=sys.stderr)
        return 2
    if args.summary_json:
        try:
            _write_running_summary_json(
                args.summary_json,
                command=command,
                cwd=None,
                environ=current_env,
                max_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                max_global_rss_kb=max_global_rss_kb,
                child_rlimit_kb=child_rlimit_kb,
                timeout_s=args.timeout,
                poll_interval_s=poll_interval,
            )
        except OSError as exc:
            print(
                f"memory_guard: failed to write running summary JSON: {exc}",
                file=sys.stderr,
            )
            return 2
    result = run_guarded(
        command,
        max_rss_kb=max_rss_kb,
        max_total_rss_kb=max_total_rss_kb,
        poll_interval=poll_interval,
        capture_output=False,
        timeout=args.timeout,
        env=_child_env_without_internal_keys(current_env),
        samples_jsonl=args.samples_jsonl,
        samples_jsonl_max_bytes=samples_jsonl_max_bytes,
        stream=args.stream,
        child_rlimit_kb=child_rlimit_kb,
        adaptive_budget_provider=adaptive_budget_provider,
        dynamic_process_rss=dynamic_process_rss,
        dynamic_total_rss=dynamic_total_rss,
    )
    incident = _incident_payload(result)
    repro_payload: dict[str, object] | None = None
    if incident is not None:
        repro_payload = repro_context_payload(
            command=command,
            cwd=None,
            environ=current_env,
            max_process_rss_kb=max_rss_kb,
            max_total_rss_kb=max_total_rss_kb,
            max_global_rss_kb=max_global_rss_kb,
            child_rlimit_kb=child_rlimit_kb,
            timeout_s=args.timeout,
            poll_interval_s=poll_interval,
            summary_json=args.summary_json,
            incident_pid=result.violation.pid if result.violation is not None else None,
        )
    if args.summary_json:
        try:
            _write_summary_json(
                args.summary_json,
                command=command,
                cwd=None,
                environ=current_env,
                max_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                max_global_rss_kb=max_global_rss_kb,
                child_rlimit_kb=child_rlimit_kb,
                timeout_s=args.timeout,
                poll_interval_s=poll_interval,
                result=result,
            )
        except OSError as exc:
            print(f"memory_guard: failed to write summary JSON: {exc}", file=sys.stderr)
            return 2 if result.returncode == 0 else result.returncode
    elif incident is not None:
        incident_summary_path = _default_incident_summary_path()
        try:
            _write_summary_json(
                str(incident_summary_path),
                command=command,
                cwd=None,
                environ=current_env,
                max_rss_kb=max_rss_kb,
                max_total_rss_kb=max_total_rss_kb,
                max_global_rss_kb=max_global_rss_kb,
                child_rlimit_kb=child_rlimit_kb,
                timeout_s=args.timeout,
                poll_interval_s=poll_interval,
                result=result,
            )
            _prune_default_incident_summaries(incident_summary_path.parent)
            if repro_payload is not None:
                repro_payload["summary_json"] = str(incident_summary_path)
            print(
                f"memory_guard: incident summary: path={incident_summary_path}",
                file=sys.stderr,
            )
        except OSError as exc:
            print(
                f"memory_guard: failed to write incident summary JSON: {exc}",
                file=sys.stderr,
            )
    if result.violation is not None:
        violation_limits = result.limit_at_violation
        limit_gb = (
            (
                violation_limits.max_total_rss_gb
                if violation_limits is not None
                else max_total_rss_gb
            )
            if result.violation.scope == "process_tree"
            else (
                violation_limits.max_process_rss_gb
                if violation_limits is not None
                else max_rss_gb
            )
        )
        incident_at = _utc_timestamp()
        cleanup = (
            "classified command as failed from child exit resource usage"
            if result.violation.scope == "process_rusage"
            else "terminated tracked process tree to prevent orphaned Molt subprocesses"
        )
        time_label = (
            "observed_at" if result.violation.scope == "process_rusage" else "killed_at"
        )
        print(
            "memory_guard: RSS limit exceeded; "
            f"{cleanup}: {time_label}={incident_at} "
            f"elapsed={_elapsed_text(result.elapsed_s)} "
            f"{_child_identity_text(result.child_process)} "
            f"pid={result.violation.pid} "
            f"rss={result.violation.rss_gb:.2f}GB "
            f"limit={_limit_text(limit_gb)} "
            f"scope={result.violation.scope} "
            f"command={result.violation.command}",
            file=sys.stderr,
        )
        print(
            "memory_guard: next action: inspect child logs and allocations for "
            "runaway work; lower parallelism/input size, or if expected raise the "
            "relevant *_MAX_PROCESS_RSS_GB/*_MAX_TOTAL_RSS_GB limit.",
            file=sys.stderr,
        )
    if result.timed_out:
        incident_at = _utc_timestamp()
        print(
            "memory_guard: timeout after "
            f"{0.0 if args.timeout is None else args.timeout:.2f}s; "
            "terminated tracked process tree to prevent orphaned Molt "
            f"subprocesses: killed_at={incident_at} "
            f"elapsed={_elapsed_text(result.elapsed_s)} "
            f"{_child_identity_text(result.child_process)}",
            file=sys.stderr,
        )
        print(
            "memory_guard: next action: inspect child logs for a hang or oversized "
            "workload; raise --timeout only for intentional long-running work.",
            file=sys.stderr,
        )
    if result.orphaned_process_groups:
        incident_at = _utc_timestamp()
        pgids = ",".join(str(pgid) for pgid in result.orphaned_process_groups)
        print(
            "memory_guard: orphaned child processes detected after command exit; "
            "terminated tracked process groups to prevent accumulation: "
            f"killed_at={incident_at} elapsed={_elapsed_text(result.elapsed_s)} "
            f"pgids={pgids} reason=direct child exited while descendants were "
            "still live",
            file=sys.stderr,
        )
        print(
            "memory_guard: next action: inspect child process lifecycle and logs; "
            "make helpers shut down explicitly, or run intentional warm daemons "
            "inside a suite-level sentinel that drains at scope exit.",
            file=sys.stderr,
        )
    exit_signal = _exit_signal_payload(result.returncode)
    if result.guard_signal is not None:
        guard_signal_payload = _exit_signal_payload(128 + result.guard_signal)
        signame = (
            guard_signal_payload["name"]
            if guard_signal_payload is not None
            and guard_signal_payload["name"] is not None
            else f"signal {result.guard_signal}"
        )
        print(
            "memory_guard: guard parent received "
            f"{signame}; summary written after terminating the tracked child tree: "
            f"observed_at={_utc_timestamp()} "
            f"elapsed={_elapsed_text(result.elapsed_s)} "
            f"{_child_identity_text(result.child_process)}",
            file=sys.stderr,
        )
        print(
            (
                "memory_guard: next action: inspect the parent host/control-plane "
                "signal source and child logs; the RSS limit incident remains "
                "the primary classification."
                if result.violation is not None
                else (
                    "memory_guard: next action: inspect the parent host/control-plane "
                    "signal source and child logs; the timeout incident remains "
                    "the primary classification."
                    if result.timed_out
                    else "memory_guard: next action: inspect the parent "
                    "host/control-plane signal source and child logs; this was "
                    "not classified as an RSS limit trip."
                )
            ),
            file=sys.stderr,
        )
        exit_signal = None
    if exit_signal is not None and result.violation is None and not result.timed_out:
        signame = exit_signal["name"] or f"signal {exit_signal['signal']}"
        print(
            "memory_guard: command exited with "
            f"{signame} status ({result.returncode}); no RSS violation observed: "
            f"observed_at={_utc_timestamp()} "
            f"elapsed={_elapsed_text(result.elapsed_s)}",
            file=sys.stderr,
        )
        print(
            "memory_guard: next action: inspect child stderr/logs or host signal "
            "source, including direct-child resource limits such as RLIMIT_AS; "
            "the guard did not classify this as an RSS limit trip.",
            file=sys.stderr,
        )
    if result.cargo_incremental_quarantine is not None:
        print(
            _cargo_incremental_quarantine_message(result.cargo_incremental_quarantine),
            file=sys.stderr,
        )
        if result.cargo_incremental_quarantine.errors:
            print(
                "memory_guard: cargo incremental quarantine errors: "
                f"{'; '.join(result.cargo_incremental_quarantine.errors)}",
                file=sys.stderr,
            )
            print(
                "memory_guard: next action: run `molt clean --apply "
                "--kill-processes` if stale Cargo state still blocks rebuilds.",
                file=sys.stderr,
            )
    if repro_payload is not None:
        print(
            f"memory_guard: repro context: {repro_context_line(repro_payload)}",
            file=sys.stderr,
        )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(hide_command_argv=True))

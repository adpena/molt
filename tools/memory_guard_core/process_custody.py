from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
import contextlib
from dataclasses import dataclass
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from typing import Any

from tools.memory_guard_core import process_model as _process_model
from tools.memory_guard_core.cargo_quarantine import CargoIncrementalQuarantine
from tools.memory_guard_core.common import utc_timestamp as _utc_timestamp
from tools.memory_guard_core.memory_limits import ResolvedMemoryLimits
from tools.memory_guard_core.windows_snapshot import (
    _windows_process_snapshot_rows,
    _windows_process_snapshot_rows_hard_timeout,
)

ROOT = Path(__file__).resolve().parents[2]


def _is_windows_process_model() -> bool:
    return os.name == "nt"


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


ProcessSample = _process_model.ProcessSample
ProcessIdentity = _process_model.ProcessIdentity
ProcessTreeTracker = _process_model.ProcessTreeTracker
RssViolation = _process_model.RssViolation
process_identity = _process_model.process_identity


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


ChildExitResourceUsage = _process_model.ChildExitResourceUsage


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
    return _process_model.elapsed_seconds_from_ps(value)


def parse_process_table(text: str) -> dict[int, ProcessSample]:
    return _process_model.parse_process_table(text)


def parse_windows_process_snapshot_rows(
    rows: Sequence[
        tuple[int, int, int, str, int | None]
        | tuple[int, int, int, str, int | None, int | None]
    ],
) -> dict[int, ProcessSample]:
    return _process_model.parse_windows_process_snapshot_rows(rows)


def sample_processes_posix() -> dict[int, ProcessSample]:
    return _process_model.sample_processes_posix()


def sample_processes_windows() -> dict[int, ProcessSample]:
    return _process_model.sample_processes_windows(_windows_process_snapshot_rows)


def sample_processes_windows_hard_timeout() -> dict[int, ProcessSample]:
    return _process_model.sample_processes_windows(
        _windows_process_snapshot_rows_hard_timeout
    )


def sample_processes() -> dict[int, ProcessSample]:
    if _is_windows_process_model():
        return sample_processes_windows()
    return sample_processes_posix()


def _sample_pgid(sample: ProcessSample) -> int:
    return _process_model.sample_pgid_or_pid(sample)


def _command_executable_name(command: str) -> str:
    return _process_model.command_executable_name(command)


def is_host_control_plane_process(sample: ProcessSample) -> bool:
    return _process_model.is_host_control_plane_process(sample)


def _ancestor_pids(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
) -> set[int]:
    return _process_model.ancestor_pids(samples, pid)


def host_control_plane_ancestor_pids(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
    *,
    include_self: bool = False,
) -> set[int]:
    return _process_model.host_control_plane_ancestor_pids(
        samples,
        pid,
        include_self=include_self,
    )


def has_host_control_plane_ancestor(
    samples: Mapping[int, ProcessSample],
    pid: int | None,
    *,
    include_self: bool = False,
) -> bool:
    return _process_model.has_host_control_plane_ancestor(
        samples,
        pid,
        include_self=include_self,
    )


def protected_process_group_ids(
    samples: Mapping[int, ProcessSample],
    *,
    self_pid: int | None = None,
    self_pgid: int | None = None,
) -> set[int]:
    return _process_model.protected_process_group_ids(
        samples,
        self_pid=self_pid,
        self_pgid=self_pgid,
    )


def _root_pid_is_kill_eligible(
    samples: Mapping[int, ProcessSample],
    root_pid: int,
    *,
    protected_pgids: set[int],
    root_owned: bool,
) -> bool:
    return _process_model.root_pid_is_kill_eligible(
        samples,
        root_pid,
        protected_pgids=protected_pgids,
        root_owned=root_owned,
        current_pid=os.getpid(),
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
    return _process_model.filter_protected_watched_pids(
        samples,
        watched,
        protected_pgids=_current_protected_process_group_ids(samples),
        current_pid=os.getpid(),
    )


def descendant_pids(samples: Mapping[int, ProcessSample], root_pid: int) -> set[int]:
    return _process_model.descendant_pids(samples, root_pid)


def watched_pids(
    samples: Mapping[int, ProcessSample],
    root_pid: int,
    *,
    tracker: ProcessTreeTracker | None = None,
) -> set[int]:
    return _process_model.watched_pids(
        samples,
        root_pid,
        tracker=tracker,
        protected_pgids=_current_protected_process_group_ids(samples),
    )


def peak_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
) -> RssViolation | None:
    return _process_model.peak_rss(
        samples,
        root_pid=root_pid,
        watched=watched,
        tracker=tracker,
        protected_pgids=_current_protected_process_group_ids(samples),
    )


def total_rss(
    samples: Mapping[int, ProcessSample],
    *,
    root_pid: int,
    watched: set[int] | None = None,
    tracker: ProcessTreeTracker | None = None,
) -> RssViolation | None:
    return _process_model.total_rss(
        samples,
        root_pid=root_pid,
        watched=watched,
        tracker=tracker,
        protected_pgids=_current_protected_process_group_ids(samples),
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
    return _process_model.find_rss_violation(
        samples,
        root_pid=root_pid,
        max_rss_kb=max_rss_kb,
        max_total_rss_kb=max_total_rss_kb,
        watched=watched,
        tracker=tracker,
        protected_pgids=_current_protected_process_group_ids(samples),
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


def _terminate_single_process_group(pgid: int, *, grace: float) -> bool:
    if pgid <= 0:
        return True
    if _is_windows_process_model():
        return _terminate_single_pid(pgid, grace=grace)
    if os.name != "posix" or pgid == os.getpgrp():
        return True
    samples = sample_processes()
    identities = {
        sample.pid: process_identity(sample)
        for sample in _process_group_members(samples, pgid)
    }
    if not identities:
        return True
    action = _terminate_process_group_if_identities_match_action(
        pgid,
        identities,
        sampler=sample_processes,
        grace=grace,
    )
    return action.result in {
        "completed_or_missing",
        "missing",
        "skipped_protected_group",
        "skipped_host_control_lineage",
        "skipped_host_control_plane",
        "skipped_identity_mismatch",
    }


def _terminate_single_pid(pid: int, *, grace: float) -> bool:
    if pid <= 0 or pid == os.getpid():
        return True
    samples = sample_processes()
    sample = samples.get(pid)
    if sample is None:
        return True
    if _process_model.has_external_host_control_plane_lineage(
        samples,
        pid,
        current_pid=os.getpid(),
    ):
        return True
    if is_host_control_plane_process(sample):
        return True
    if _sample_pgid(sample) in _current_protected_process_group_ids(samples):
        return True
    action = _terminate_pid_if_identity_action(
        pid,
        process_identity(sample),
        sampler=sample_processes,
        grace=grace,
    )
    return action.result in {
        "completed_or_missing",
        "skipped_missing",
        "skipped_identity_mismatch",
        "skipped_host_control_lineage",
        "skipped_host_control_plane",
        "skipped_protected_group_member",
    }


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


def process_group_exited_or_unobservable(pgid: int, *, grace: float) -> bool:
    """Shared non-terminating process-group liveness probe after a scoped signal."""
    return _process_group_exited_or_unobservable(pgid, grace=grace)


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
    if _process_model.has_external_host_control_plane_lineage(
        samples,
        pid,
        current_pid=os.getpid(),
        owned_pids={pid} if identity is not None else (),
    ):
        return _termination_action(
            target_kind="process",
            target_id=pid,
            signum=signum,
            result="skipped_host_control_lineage",
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
        if _process_model.has_external_host_control_plane_lineage(
            samples,
            sample.pid,
            current_pid=os.getpid(),
            owned_pids=identities.keys(),
        ):
            return _termination_action(
                target_kind="process_group",
                target_id=pgid,
                signum=signum,
                result="skipped_host_control_lineage",
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
            elif root_sample is None:
                result = "skipped_missing_identity"
            elif root_sample is not None and is_host_control_plane_process(root_sample):
                result = "skipped_host_control_plane"
            elif (
                root_sample is not None
                and _process_model.has_external_host_control_plane_lineage(
                    observed_samples,
                    root_pid,
                    current_pid=os.getpid(),
                    owned_pids={root_pid} if root_owned else (),
                )
            ):
                result = "skipped_host_control_lineage"
            elif (
                root_sample is not None and _sample_pgid(root_sample) in protected_pgids
            ):
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
        identity_sampler = (
            (lambda: observed_samples) if samples is not None else sampler
        )
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


def terminate_verified_pid(
    pid: int,
    identity: ProcessIdentity,
    *,
    sampler: Callable[[], Mapping[int, ProcessSample]] = sample_processes,
    grace: float = 0.25,
) -> tuple[GuardTerminationAction, ...]:
    """Terminate one PID only through the shared identity/custody gate.

    Callers may capture ``identity`` from a previous trusted snapshot, but every
    signal is sent only after a fresh sampler pass proves that the PID still has
    the same identity and is outside host-control-plane/protected groups.
    """

    actions: list[GuardTerminationAction] = [
        _terminate_pid_if_identity_action(
            pid,
            identity,
            sampler=sampler,
            grace=grace,
        )
    ]
    if actions[0].result == "still_live":
        actions.append(
            _send_pid_signal_if_identity_action(
                pid,
                identity,
                fallback_kill_signal(),
                sampler=sampler,
            )
        )
    return tuple(actions)


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

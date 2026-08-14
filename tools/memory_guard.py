#!/usr/bin/env python3
from __future__ import annotations

import argparse
from collections.abc import Callable, Mapping, Sequence
import contextlib
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
DEFAULT_TERMINATION_WAIT_SEC = 2.0
DEFAULT_INCIDENT_SUMMARY_KEEP = 32
# When one full process-table sample costs more than the configured poll
# interval (Windows full-table snapshots under build load), polling
# back-to-back pins a core on guard bookkeeping and steals wall time from the
# guarded workload. Pacing bounds the sampling duty cycle at
# 1 / (1 + factor) of loop wall time while never waiting less than the
# configured poll interval, so cheap samplers (POSIX ps, test fakes) keep the
# exact configured cadence.
SAMPLE_COST_PACING_FACTOR = 2.0


def _platform_detail_no_subprocess() -> str:
    if sys.platform == "win32":
        try:
            version = sys.getwindowsversion()
        except AttributeError:
            return "Windows"
        detail = f"Windows-{version.major}.{version.minor}.{version.build}"
        service_pack = getattr(version, "service_pack", "")
        if service_pack:
            detail += f"-{str(service_pack).replace(' ', '-')}"
        return detail
    uname = platform.uname()
    return "-".join(
        str(part).replace(" ", "_")
        for part in (uname.system, uname.release, uname.version)
        if part
    )


def paced_poll_interval(poll_interval: float, last_sample_cost_s: float) -> float:
    """Return the next guard poll wait, paced by the last sampling cost.

    Single authority for every guard/sentinel polling loop. The configured
    ``poll_interval`` is always the floor, so behavior is identical whenever
    sampling is at least as fast as the configured cadence; only genuinely
    expensive samplers stretch the wait, bounding guard sampling overhead to
    at most ``1 / (1 + SAMPLE_COST_PACING_FACTOR)`` of loop wall time.
    """
    if last_sample_cost_s <= 0.0:
        return poll_interval
    return max(poll_interval, SAMPLE_COST_PACING_FACTOR * last_sample_cost_s)


ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))
from tools.memory_guard_core.common import utc_timestamp as _utc_timestamp  # noqa: E402
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
    windows_job_cleanup_payload as windows_job_cleanup_payload,
)
from tools.memory_guard_core.sample_records import (  # noqa: E402
    DEFAULT_SAMPLES_MAX_MB as DEFAULT_SAMPLES_MAX_MB,
    _append_sample_jsonl as _append_sample_jsonl,
    _format_sample_payload as _format_sample_payload,
    _record_gb as _record_gb,
    _record_sample as _record_sample,
    _rotate_jsonl_if_needed as _rotate_jsonl_if_needed,
    _samples_max_bytes_from_mb as _samples_max_bytes_from_mb,
    _stream_sample_payload as _stream_sample_payload,
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
    ProcessSnapshotError as ProcessSnapshotError,
    WindowsProcessSnapshotTimeout as WindowsProcessSnapshotTimeout,
    WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES as WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES,
    _filetime_to_unix_seconds as _filetime_to_unix_seconds,
    _windows_process_needs_full_command_line as _windows_process_needs_full_command_line,
    _windows_process_snapshot_rows_hard_timeout as _windows_process_snapshot_rows_hard_timeout,
    _windows_process_snapshot_rows as _windows_process_snapshot_rows,
    windows_process_handle_rss_kb as windows_process_handle_rss_kb,
    windows_process_handle_started_at_ns as windows_process_handle_started_at_ns,
)
from tools.process_spawn import (  # noqa: E402
    detached_process_group_kwargs,
    inherit_stdio_kwargs,
)
from tools import win_job as _win_job  # noqa: E402

WindowsJobCleanup = _win_job.WindowsJobCleanup
from tools.memory_guard_core import process_model as _process_model  # noqa: E402
from tools.memory_guard_core import process_custody as _process_custody  # noqa: E402
from tools.memory_guard_core import cli_contract as _cli_contract  # noqa: E402
from tools.memory_guard_core import repro_context as _repro_context  # noqa: E402
from tools.memory_guard_core import reporting as _reporting  # noqa: E402
from tools.memory_guard_core.paths import active_guard_marker_dir  # noqa: E402
from tools.memory_guard_core.process_custody import (  # noqa: E402
    ChildExitResourceUsage as ChildExitResourceUsage,
    GuardOrphanCleanupResult as GuardOrphanCleanupResult,
    GuardResult as GuardResult,
    GuardSamplingTelemetry as GuardSamplingTelemetry,
    GuardTerminationAction as GuardTerminationAction,
    GuardTerminationReport as GuardTerminationReport,
    GuardedChildProcess as GuardedChildProcess,
    GuardedLaunch as GuardedLaunch,
    MAX_TERMINATION_PID_FANOUT as MAX_TERMINATION_PID_FANOUT,
    ProcessIdentity as ProcessIdentity,
    ProcessSample as ProcessSample,
    ProcessTreeTracker as ProcessTreeTracker,
    RssViolation as RssViolation,
    _ancestor_pids as _ancestor_pids,
    _command_executable_name as _command_executable_name,
    _current_protected_process_group_ids as _current_protected_process_group_ids,
    _elapsed_seconds_from_ps as _elapsed_seconds_from_ps,
    _filter_protected_watched_pids as _filter_protected_watched_pids,
    _fully_completed_process_groups as _fully_completed_process_groups,
    _inject_guard_memory_contract_env as _inject_guard_memory_contract_env,
    _is_windows_process_model as _is_windows_process_model,
    _live_process_group_ids as _live_process_group_ids,
    _pid_exited_or_unobservable as _pid_exited_or_unobservable,
    _poll_wait4_child as _poll_wait4_child,
    _process_group_exited_or_unobservable as _process_group_exited_or_unobservable,
    _process_group_is_fully_owned as _process_group_is_fully_owned,
    _process_group_members as _process_group_members,
    _repo_scoped_orphan_cleanup_report as _repo_scoped_orphan_cleanup_report,
    _repo_scoped_post_baseline_orphan_groups as _repo_scoped_post_baseline_orphan_groups,
    _root_pid_is_kill_eligible as _root_pid_is_kill_eligible,
    _rusage_maxrss_kb as _rusage_maxrss_kb,
    _safe_getpgid as _safe_getpgid,
    _safe_getpgrp as _safe_getpgrp,
    _safe_getsid as _safe_getsid,
    _sample_pgid as _sample_pgid,
    _send_pid_signal_action as _send_pid_signal_action,
    _send_pid_signal_if_identity_action as _send_pid_signal_if_identity_action,
    _send_process_group_signal_action as _send_process_group_signal_action,
    _send_process_group_signal_if_identities_match_action as _send_process_group_signal_if_identities_match_action,
    _set_env_gb_ceiling as _set_env_gb_ceiling,
    _signal_name as _signal_name,
    _terminate_pid_if_identity_action as _terminate_pid_if_identity_action,
    _terminate_process_group as _terminate_process_group,
    _terminate_process_group_if_identities_match_action as _terminate_process_group_if_identities_match_action,
    _terminate_single_process_group as _terminate_single_process_group,
    _termination_action as _termination_action,
    cleanup_repo_scoped_orphans_since_baseline as cleanup_repo_scoped_orphans_since_baseline,
    descendant_pids as descendant_pids,
    fallback_kill_signal as fallback_kill_signal,
    fallback_kill_signal_payload as fallback_kill_signal_payload,
    find_rss_violation as find_rss_violation,
    has_host_control_plane_ancestor as has_host_control_plane_ancestor,
    host_control_plane_ancestor_pids as host_control_plane_ancestor_pids,
    is_host_control_plane_process as is_host_control_plane_process,
    parse_process_table as parse_process_table,
    parse_windows_process_snapshot_rows as parse_windows_process_snapshot_rows,
    peak_rss as peak_rss,
    process_group_exited_or_unobservable as process_group_exited_or_unobservable,
    process_identity as process_identity,
    protected_process_group_ids as protected_process_group_ids,
    signal_payload as signal_payload,
    term_signal_payload as term_signal_payload,
    terminate_verified_pid as terminate_verified_pid,
    total_rss as total_rss,
    watched_pids as watched_pids,
)

PYTEST_OUTER_GUARD_SUMMARY_DIR = ROOT / "tmp" / "pytest-memory-guard"
GUARD_RETURN_CODE = 137
TIMEOUT_RETURN_CODE = 124
INTERNAL_COMMAND_ENV = "MOLT_MEMORY_GUARD_COMMAND_JSON"
INTERNAL_WORKER_ENV = "MOLT_MEMORY_GUARD_INTERNAL"
ACTIVE_ENV = "MOLT_MEMORY_GUARD_ACTIVE"
ACTIVE_GUARD_PID_ENV = "MOLT_MEMORY_GUARD_PID"
ACTIVE_GUARD_TOKEN_ENV = "MOLT_MEMORY_GUARD_TOKEN"
ACTIVE_GUARD_MARKER_ENV = "MOLT_MEMORY_GUARD_MARKER"
ACTIVE_GUARD_MARKER_DIR = active_guard_marker_dir(ROOT)
ACTIVE_GUARD_MARKER_KEEP = 128
_INTERNAL_ENV_KEYS = (
    INTERNAL_COMMAND_ENV,
    INTERNAL_WORKER_ENV,
)
HOST_CONTROL_PLANE_TOKENS = _process_model.HOST_CONTROL_PLANE_TOKENS
HOST_CONTROL_PLANE_EXECUTABLE_NAMES = _process_model.HOST_CONTROL_PLANE_EXECUTABLE_NAMES
sample_processes_linux_proc = _process_model.sample_processes_linux_proc


def sample_processes_posix() -> dict[int, ProcessSample]:
    return _process_model.sample_processes_posix()


def parse_process_table_with_start(text: str) -> dict[int, ProcessSample]:
    return _process_model.parse_process_table_with_start(text)


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


def _timeout_sampler(
    sampler: Callable[[], Mapping[int, ProcessSample]],
) -> Callable[[], Mapping[int, ProcessSample]]:
    if _is_windows_process_model() and sampler is sample_processes:
        return sample_processes_windows_hard_timeout
    return sampler


def _sync_process_custody_facade() -> None:
    _process_custody._is_windows_process_model = _is_windows_process_model
    _process_custody.sample_processes = sample_processes
    _process_custody.sample_processes_posix = sample_processes_posix
    _process_custody.sample_processes_windows = sample_processes_windows
    _process_custody.sample_processes_windows_hard_timeout = (
        sample_processes_windows_hard_timeout
    )
    _process_custody._current_protected_process_group_ids = (
        _current_protected_process_group_ids
    )
    _process_custody._filter_protected_watched_pids = _filter_protected_watched_pids


_custody_terminate_watched_processes = _process_custody.terminate_watched_processes
_custody_cleanup_tracked_orphans = _process_custody.cleanup_tracked_orphans
_custody_terminate_single_pid = _process_custody._terminate_single_pid


def terminate_watched_processes(
    *args: object, **kwargs: object
) -> GuardTerminationReport:
    _sync_process_custody_facade()
    return _custody_terminate_watched_processes(*args, **kwargs)


def _validated_termination_report(
    report: object,
    *,
    caller: str,
) -> GuardTerminationReport:
    if not isinstance(report, GuardTerminationReport):
        raise TypeError(
            f"{caller} must return GuardTerminationReport, got {type(report).__name__}"
        )
    return report


def _validated_termination_reports(
    reports: Sequence[object],
    *,
    caller: str,
) -> tuple[GuardTerminationReport, ...]:
    return tuple(
        _validated_termination_report(report, caller=caller) for report in reports
    )


_terminate_watched_processes_facade = terminate_watched_processes


def cleanup_tracked_orphans(
    *args: object, **kwargs: object
) -> GuardOrphanCleanupResult:
    _sync_process_custody_facade()
    delegate = terminate_watched_processes
    if delegate is _terminate_watched_processes_facade:
        delegate = _custody_terminate_watched_processes
    previous = _process_custody.terminate_watched_processes
    _process_custody.terminate_watched_processes = delegate
    try:
        return _custody_cleanup_tracked_orphans(*args, **kwargs)
    finally:
        _process_custody.terminate_watched_processes = previous


def _terminate_single_pid(pid: int, *, grace: float) -> bool:
    _sync_process_custody_facade()
    return _custody_terminate_single_pid(pid, grace=grace)


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


def _write_json_atomic(path: Path, payload: Mapping[str, object]) -> None:
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp_path.write_text(json.dumps(payload, sort_keys=True) + "\n", encoding="utf-8")
    os.replace(tmp_path, path)


def _write_active_guard_marker(
    pid: int,
    *,
    command: Sequence[str],
    cwd: str | Path | None,
) -> tuple[str, Path]:
    if pid <= 0:
        raise ValueError("active guard marker requires a live pid")
    token = os.urandom(16).hex()
    ACTIVE_GUARD_MARKER_DIR.mkdir(parents=True, exist_ok=True)
    marker_path = ACTIVE_GUARD_MARKER_DIR / f"guard-{pid}-{token}.json"
    cwd_path = Path.cwd() if cwd is None else Path(cwd).expanduser()
    payload = {
        "schema_version": 1,
        "pid": pid,
        "token": token,
        "path": str(Path(__file__).resolve()),
        "command": list(command),
        "cwd": str(cwd_path.resolve(strict=False)),
        "status": "guard_starting",
        "created_at": _utc_timestamp(),
        "updated_at": _utc_timestamp(),
    }
    _write_json_atomic(marker_path, payload)
    _prune_active_guard_markers()
    return token, marker_path


def _update_active_guard_marker(
    marker_path: Path,
    token: str,
    *,
    status: str,
    **fields: object,
) -> None:
    try:
        payload = json.loads(marker_path.read_text(encoding="utf-8"))
    except (FileNotFoundError, OSError, json.JSONDecodeError):
        return
    if payload.get("token") != token:
        return
    payload.update(fields)
    payload["status"] = status
    payload["updated_at"] = _utc_timestamp()
    with contextlib.suppress(OSError):
        _write_json_atomic(marker_path, payload)


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


def _apply_child_resource_limit(limit_kb: int) -> None:
    if limit_kb <= 0:
        return
    try:
        import resource  # type: ignore
    except Exception:
        return
    limit_bytes = int(limit_kb * 1024)
    # The guard budget is a committed-memory/RSS policy, not a virtual-address
    # policy.  Treating the same number as RLIMIT_AS breaks healthy children
    # that reserve sparse address space: V8's trap-based WebAssembly bounds
    # checks, sanitizers, and compiler allocators all reserve substantially more
    # virtual space than they commit. RLIMIT_DATA also constrains mmap on modern
    # Linux, so it has the same false-OOM failure mode for sparse reservations.
    # The parent guard already measures every owned descendant's RSS; apply only
    # the matching kernel RSS resource where the host implements it.
    for name in ("RLIMIT_RSS",):
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


def _write_child_started_fd(fd: int | None) -> None:
    if fd is None:
        return
    try:
        os.write(fd, f"{time.monotonic_ns()}\n".encode("ascii"))
    except OSError:
        pass
    with contextlib.suppress(OSError):
        os.close(fd)


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
    # direct spawn path (POSIX rlimit, POSIX no-rlimit, or Windows Job custody)
    # so none of them mis-resolve it against the child's `cwd=`.
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
    # Windows has no POSIX rlimit implementation. The outer guard assigns the
    # real command to a suspended KILL_ON_JOB_CLOSE Job before it can execute,
    # then enforces the process/tree memory contract from that same kernel
    # object. The former Python child-runner added startup and a second process
    # without enforcing an additional limit, so direct launch is the sole
    # Windows authority.
    return GuardedLaunch(command=list(command), env=base_env)


def _close_fds(fds: Sequence[int | None]) -> None:
    for fd in fds:
        if fd is None:
            continue
        with contextlib.suppress(OSError):
            os.close(fd)


def _guarded_popen_process_isolation_kwargs() -> dict[str, object]:
    return detached_process_group_kwargs(
        windows=_is_windows_process_model(),
        subprocess_module=subprocess,
    )


def _read_child_started_at(fd: int | None) -> float | None:
    """Read the child-start timestamp without consuming descriptor ownership.

    ``run_guarded`` owns ``GuardedLaunch.started_read_fd`` from launch through
    its single finalizer.  Closing it here as well creates an ABA race: once
    the kernel reuses the descriptor number for an unrelated file, the
    finalizer's second close can corrupt that concurrent read.  The launch
    finalizer therefore remains the only close authority for this descriptor.
    """

    if fd is None:
        return None
    try:
        raw = os.read(fd, 64)
    except OSError:
        return None
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
    if orphaned_process_groups and returncode != 0:
        return "orphaned_processes_cleaned"
    if returncode is not None and _returncode_looks_signal(returncode):
        return "signal_exit"
    return None


def _returncode_signal_payload(returncode: int) -> dict[str, object] | None:
    return _reporting.exit_signal_payload(
        returncode,
        windows_process_model=_is_windows_process_model(),
    )


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


def _sampling_telemetry_payload(
    telemetry: GuardSamplingTelemetry | None,
) -> dict[str, object] | None:
    return _reporting.sampling_telemetry_payload(telemetry)


def run_guarded(
    command: Sequence[str],
    *,
    max_rss_kb: int,
    max_total_rss_kb: int | None = None,
    poll_interval: float,
    sampler: Callable[[], Mapping[int, ProcessSample]] = sample_processes,
    capture_output: bool = True,
    stdout_capture_path: str | Path | None = None,
    stderr_capture_path: str | Path | None = None,
    capture_tail_bytes: int | None = None,
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
    running_summary_json: str | None = None,
    running_summary_environ: Mapping[str, str] | None = None,
    running_summary_max_global_rss_kb: int | None = None,
    on_spawn: Callable[[int], None] | None = None,
) -> GuardResult:
    if not command:
        raise ValueError("command is required")
    if sampler is None:
        sampler = sample_processes
    if poll_interval <= 0:
        raise ValueError("poll interval must be greater than 0")
    if timeout is not None and timeout <= 0:
        raise ValueError("timeout must be greater than 0")
    if (stdout_capture_path is None) != (stderr_capture_path is None):
        raise ValueError("stdout/stderr capture paths must be provided together")
    if stdout_capture_path is not None and not capture_output:
        raise ValueError("capture paths require capture_output")
    if capture_tail_bytes is not None and capture_tail_bytes <= 0:
        raise ValueError("capture_tail_bytes must be positive")
    if capture_tail_bytes is not None and stdout_capture_path is None:
        raise ValueError("capture_tail_bytes requires external capture paths")
    if stdout_capture_path is not None and Path(stdout_capture_path).resolve() == Path(
        stderr_capture_path  # type: ignore[arg-type]
    ).resolve():
        raise ValueError("stdout/stderr capture paths must be distinct")
    if keepalive_interval is not None and keepalive_interval <= 0:
        keepalive_interval = None
    if text and isinstance(input, bytes):
        raise TypeError("bytes input requires text=False")
    if not text and isinstance(input, str):
        raise TypeError("str input requires text=True")
    child_env = dict(os.environ) if env is None else dict(env)
    child_env[ACTIVE_ENV] = "1"
    child_env[ACTIVE_GUARD_PID_ENV] = str(os.getpid())
    guard_token, guard_marker = _write_active_guard_marker(
        os.getpid(),
        command=command,
        cwd=cwd,
    )
    child_env[ACTIVE_GUARD_TOKEN_ENV] = guard_token
    child_env[ACTIVE_GUARD_MARKER_ENV] = str(guard_marker)
    _inject_guard_memory_contract_env(
        child_env,
        max_rss_kb=max_rss_kb,
        child_rlimit_kb=child_rlimit_kb,
    )
    start = time.monotonic()
    observer_cpu_start = time.process_time()
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
    guard_job: int | None = None
    windows_job_cleanup: _win_job.WindowsJobCleanup | None = None
    launch: GuardedLaunch | None = None
    child_process: GuardedChildProcess | None = None
    tracker: ProcessTreeTracker | None = None
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
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="launch_prepared",
            launch_command=list(launch.command),
        )
        if capture_output:
            if stdout_capture_path is not None:
                stdout_path = Path(stdout_capture_path)
                stderr_path = Path(stderr_capture_path)  # type: ignore[arg-type]
                stdout_path.parent.mkdir(parents=True, exist_ok=True)
                stderr_path.parent.mkdir(parents=True, exist_ok=True)
                if text:
                    stdout_capture = stdout_path.open(
                        mode="x+t", encoding=encoding, errors=errors
                    )
                    stderr_capture = stderr_path.open(
                        mode="x+t", encoding=encoding, errors=errors
                    )
                else:
                    stdout_capture = stdout_path.open(mode="x+b")
                    stderr_capture = stderr_path.open(mode="x+b")
            elif text:
                stdout_capture = tempfile.TemporaryFile(
                    mode="w+t", encoding=encoding, errors=errors
                )
                stderr_capture = tempfile.TemporaryFile(
                    mode="w+t", encoding=encoding, errors=errors
                )
            else:
                stdout_capture = tempfile.TemporaryFile(mode="w+b")
                stderr_capture = tempfile.TemporaryFile(mode="w+b")
        popen_kwargs: dict[str, object] = {
            "cwd": cwd,
            "env": dict(launch.env) if launch.env is not None else None,
            "text": text,
            **_guarded_popen_process_isolation_kwargs(),
        }
        if capture_output:
            popen_kwargs["stdout"] = stdout_capture
            popen_kwargs["stderr"] = stderr_capture
        else:
            popen_kwargs.update(inherit_stdio_kwargs())
        if input is not None:
            popen_kwargs["stdin"] = subprocess.PIPE
        if launch.pass_fds:
            popen_kwargs["pass_fds"] = launch.pass_fds
        if launch.preexec_fn is not None:
            popen_kwargs["preexec_fn"] = launch.preexec_fn
        # Windows Job Object custody: create a KILL_ON_JOB_CLOSE job and spawn
        # the child SUSPENDED so it is placed in the job before it can spawn any
        # descendant (race-free capture). The guard holds the sole handle, so if
        # it dies for ANY reason the OS reaps the whole build subtree instead of
        # leaking orphaned cargo/rustc/link/tail that reserve GB. Creation,
        # assignment, and resume are fail-closed: Windows never launches an
        # unassigned child.
        guard_job = _win_job.create_kill_on_close_job()
        if guard_job is not None:
            popen_kwargs["creationflags"] = (
                int(popen_kwargs.get("creationflags", 0) or 0)
                | _win_job.suspended_creationflag()
            )
        try:
            proc = subprocess.Popen(launch.command, **popen_kwargs)
        except Exception as exc:
            if guard_job is not None:
                _win_job.close_job(guard_job)
            guard_job = None
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="spawn_failed",
                launch_command=list(launch.command),
                spawn_error_type=type(exc).__name__,
                spawn_error=str(exc),
            )
            # The outer finalizer is the sole owner of started_read_fd.  Do not
            # close it here as well: a reused numeric descriptor would make the
            # finalizer's second close corrupt an unrelated concurrent file.
            _close_fds(launch.close_fds)
            if stdout_capture is not None:
                stdout_capture.close()
            if stderr_capture is not None:
                stderr_capture.close()
            raise
        if guard_job is not None:
            # Child was spawned SUSPENDED; assignment completes before resume.
            # A custody failure terminates the still-suspended child or its job
            # and raises with exact Win32 evidence.
            _win_job.assign_and_resume(guard_job, proc)
        _close_fds(launch.close_fds)
        child_process = GuardedChildProcess(
            pid=proc.pid,
            pgid=_safe_getpgid(proc.pid),
            sid=_safe_getsid(proc.pid),
            command=tuple(launch.command),
            started_at=_utc_timestamp(),
        )
        tracker = ProcessTreeTracker(proc.pid)
        sampling_source = (
            "windows_full_process_table"
            if _is_windows_process_model()
            else "posix_process_table"
        )
        if sampler is not sample_processes:
            sampling_source = "custom"
        if guard_job is not None:
            cleanup_orphans = False
        if guard_job is not None:
            # A Windows Job already provides race-free ownership for the entire
            # descendant tree. Query only those kernel-owned members for every
            # guarded command; full host snapshots are fallback/diagnostic
            # authority, never the steady-state sampler.
            job_command = " ".join(command)
            job_member_commands: dict[tuple[int, int | None], str] = {}
            previous_job_members: frozenset[int] = frozenset()

            def _sample_owned_job() -> Mapping[int, ProcessSample]:
                nonlocal job_member_commands, previous_job_members
                members = _win_job.process_memory(guard_job)
                member_ids = frozenset(member.pid for member in members)
                if member_ids != previous_job_members:
                    members = _win_job.process_memory(
                        guard_job,
                        include_image_names=True,
                    )
                    previous_job_members = frozenset(member.pid for member in members)
                    live_keys = {
                        (member.pid, member.started_at_ns) for member in members
                    }
                    job_member_commands = {
                        key: value
                        for key, value in job_member_commands.items()
                        if key in live_keys
                    }
                    for member in members:
                        if member.image_name:
                            job_member_commands[(member.pid, member.started_at_ns)] = (
                                member.image_name
                            )
                samples: dict[int, ProcessSample] = {}
                for member in members:
                    command_text = (
                        job_command
                        if member.pid == proc.pid
                        else job_member_commands.get(
                            (member.pid, member.started_at_ns),
                            f"windows-job-member pid={member.pid}",
                        )
                    )
                    samples[member.pid] = ProcessSample(
                        pid=member.pid,
                        ppid=os.getpid() if member.pid == proc.pid else proc.pid,
                        rss_kb=(member.rss_bytes + 1023) // 1024,
                        command=command_text,
                        pgid=child_process.pgid,
                        started_at_ns=member.started_at_ns,
                    )
                return samples

            sampler = _sample_owned_job
            sampling_source = "windows_job_members"
        root_started_at_ns = (
            windows_process_handle_started_at_ns(getattr(proc, "_handle", None))
            if _is_windows_process_model()
            else _process_model.process_started_at_ns(proc.pid)
        )
        if root_started_at_ns is not None:
            assert tracker.known_identities is not None
            tracker.known_identities[proc.pid] = ProcessIdentity(root_started_at_ns)
        if running_summary_json is not None:
            try:
                _write_running_summary_json(
                    running_summary_json,
                    command=command,
                    cwd=cwd,
                    environ=(
                        running_summary_environ
                        if running_summary_environ is not None
                        else child_env
                    ),
                    max_rss_kb=max_rss_kb,
                    max_total_rss_kb=max_total_rss_kb,
                    max_global_rss_kb=running_summary_max_global_rss_kb,
                    child_rlimit_kb=child_rlimit_kb,
                    timeout_s=timeout,
                    poll_interval_s=poll_interval,
                    child_process=child_process,
                )
            except OSError as exc:
                print(
                    f"memory_guard: failed to refresh running summary JSON: {exc}",
                    file=sys.stderr,
                    flush=True,
                )
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="child_running",
            child_process=guarded_child_process_payload(child_process),
        )

        def terminate_owned_tree(
            *,
            reason: str,
            samples: Mapping[int, ProcessSample] | None = None,
            watched: set[int] | None = None,
            grace: float,
        ) -> None:
            if guard_job is not None:
                # The Job is the exact Windows ownership boundary established
                # before the child was resumed.  Do not duplicate that authority
                # with PID-table termination, whose identities can race process
                # exit and wrapper descendants.
                _win_job.terminate_job(guard_job)
                _win_job.wait_until_empty(guard_job, timeout=termination_wait_s)
                return
            termination_reports.append(
                _validated_termination_report(
                    terminate_watched_processes(
                        proc.pid,
                        samples=samples,
                        watched=watched,
                        grace=grace,
                        reason=reason,
                        sampler=sampler,
                        tracker=tracker,
                        root_owned=True,
                    ),
                    caller="terminate_watched_processes",
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
        launch_rss_kb = windows_process_handle_rss_kb(getattr(proc, "_handle", None))
        if launch_rss_kb is not None and launch_rss_kb > 0:
            peak = RssViolation(
                pid=proc.pid,
                rss_kb=launch_rss_kb,
                command=" ".join(command),
                scope="process_handle",
            )
            peak_total = RssViolation(
                pid=proc.pid,
                rss_kb=launch_rss_kb,
                command="process tree aggregate from direct child process handle",
                scope="process_tree_handle",
            )
        timed_out = False
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
        sampling_attempts = 0
        sampling_successes = 0
        transient_sampling_failures = 0
        first_transient_sampling_failure_at: str | None = None
        last_transient_sampling_failure_at: str | None = None
        last_transient_sampling_error: str | None = None
        sampling_wall_time_s = 0.0
        sampling_cpu_time_s = 0.0
        max_sampling_wall_time_s = 0.0
        max_sampling_cpu_time_s = 0.0
        sampling_process_rows = 0
        max_sampling_process_rows = 0

        def sampling_telemetry() -> GuardSamplingTelemetry:
            observer_wall_time_s = max(0.0, time.monotonic() - start)
            observer_cpu_time_s = max(0.0, time.process_time() - observer_cpu_start)
            return GuardSamplingTelemetry(
                attempts=sampling_attempts,
                successes=sampling_successes,
                transient_failures=transient_sampling_failures,
                first_transient_failure_at=first_transient_sampling_failure_at,
                last_transient_failure_at=last_transient_sampling_failure_at,
                last_transient_error=last_transient_sampling_error,
                source=sampling_source,
                wall_time_s=sampling_wall_time_s,
                cpu_time_s=sampling_cpu_time_s,
                max_wall_time_s=max_sampling_wall_time_s,
                max_cpu_time_s=max_sampling_cpu_time_s,
                process_rows=sampling_process_rows,
                max_process_rows=max_sampling_process_rows,
                observer_wall_time_s=observer_wall_time_s,
                observer_cpu_time_s=observer_cpu_time_s,
                observer_cpu_duty_cycle=(
                    observer_cpu_time_s / observer_wall_time_s
                    if observer_wall_time_s > 0.0
                    else 0.0
                ),
            )

        def record_transient_sampling_failure(
            exc: WindowsProcessSnapshotTimeout,
            *,
            attempt_already_counted: bool = True,
        ) -> None:
            nonlocal sampling_attempts
            nonlocal transient_sampling_failures
            nonlocal first_transient_sampling_failure_at
            nonlocal last_transient_sampling_failure_at
            nonlocal last_transient_sampling_error
            observed_at = _utc_timestamp()
            if not attempt_already_counted:
                sampling_attempts += 1
            transient_sampling_failures += 1
            if first_transient_sampling_failure_at is None:
                first_transient_sampling_failure_at = observed_at
            last_transient_sampling_failure_at = observed_at
            last_transient_sampling_error = str(exc)
            telemetry = sampling_telemetry()
            print(
                "memory_guard: Windows process snapshot timed out; preserving "
                "the healthy guarded child and marking this RSS enforcement "
                f"observation unavailable: {exc}",
                file=sys.stderr,
                flush=True,
            )
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="child_running_telemetry_degraded",
                child_process=guarded_child_process_payload(child_process),
                elapsed_s=time.monotonic() - start,
                sampling_telemetry=_sampling_telemetry_payload(telemetry),
            )

        def terminate_direct_child_handle(*, reason: str) -> None:
            if proc.poll() is not None:
                return
            started_at = _utc_timestamp()
            actions: list[GuardTerminationAction] = []
            try:
                proc.terminate()
            except ProcessLookupError:
                actions.append(
                    GuardTerminationAction(
                        target_kind="owned_child_handle",
                        target_id=proc.pid,
                        signal=signal.SIGTERM,
                        signal_name="SIGTERM",
                        result="completed_or_missing",
                    )
                )
            except OSError as exc:
                actions.append(
                    GuardTerminationAction(
                        target_kind="owned_child_handle",
                        target_id=proc.pid,
                        signal=signal.SIGTERM,
                        signal_name="SIGTERM",
                        result="failed",
                        error=str(exc),
                    )
                )
            else:
                try:
                    proc.wait(timeout=max(0.25, termination_wait_s))
                except subprocess.TimeoutExpired:
                    actions.append(
                        GuardTerminationAction(
                            target_kind="owned_child_handle",
                            target_id=proc.pid,
                            signal=signal.SIGTERM,
                            signal_name="SIGTERM",
                            result="still_live",
                        )
                    )
                    try:
                        proc.kill()
                        proc.wait(timeout=max(0.25, termination_wait_s))
                    except ProcessLookupError:
                        actions.append(
                            GuardTerminationAction(
                                target_kind="owned_child_handle",
                                target_id=proc.pid,
                                signal=fallback_kill_signal(),
                                signal_name=_signal_name(fallback_kill_signal()),
                                result="completed_or_missing",
                            )
                        )
                    except (OSError, subprocess.TimeoutExpired) as exc:
                        actions.append(
                            GuardTerminationAction(
                                target_kind="owned_child_handle",
                                target_id=proc.pid,
                                signal=fallback_kill_signal(),
                                signal_name=_signal_name(fallback_kill_signal()),
                                result="failed",
                                error=str(exc),
                            )
                        )
                    else:
                        actions.append(
                            GuardTerminationAction(
                                target_kind="owned_child_handle",
                                target_id=proc.pid,
                                signal=fallback_kill_signal(),
                                signal_name=_signal_name(fallback_kill_signal()),
                                result="completed_or_missing",
                            )
                        )
                else:
                    actions.append(
                        GuardTerminationAction(
                            target_kind="owned_child_handle",
                            target_id=proc.pid,
                            signal=signal.SIGTERM,
                            signal_name="SIGTERM",
                            result="completed_or_missing",
                        )
                    )
            termination_reports.append(
                GuardTerminationReport(
                    reason=reason,
                    started_at=started_at,
                    completed_at=_utc_timestamp(),
                    root_pid=proc.pid,
                    root_pgid=child_process.pgid,
                    root_sid=child_process.sid,
                    grace_sec=termination_wait_s,
                    watched_pids=(proc.pid,),
                    protected_pgids=(),
                    escaped_pids=(),
                    remaining_pgids=(),
                    remaining_pids=(
                        (proc.pid,)
                        if actions and actions[-1].result in {"failed", "still_live"}
                        else ()
                    ),
                    actions=tuple(actions),
                )
            )

        def terminate_after_sampling_failure(*, reason: str) -> None:
            if remembered_samples is not None and remembered_watched is not None:
                terminate_owned_tree(
                    reason=reason,
                    samples=remembered_samples,
                    watched=remembered_watched,
                    grace=0.0,
                )
                terminate_direct_child_handle(reason=f"{reason}_direct_child_handle")
                return
            termination_reports.append(
                _validated_termination_report(
                    terminate_watched_processes(
                        proc.pid,
                        grace=0.0,
                        reason=reason,
                        sampler=sample_processes,
                        tracker=tracker,
                        root_owned=True,
                    ),
                    caller="terminate_watched_processes",
                )
            )
            terminate_direct_child_handle(reason=f"{reason}_direct_child_handle")

        if on_spawn is not None:
            try:
                on_spawn(proc.pid)
            except BaseException as callback_error:
                try:
                    if guard_job is not None:
                        _win_job.terminate_job(guard_job)
                        _win_job.wait_until_empty(guard_job, timeout=termination_wait_s)
                    else:
                        termination_reports.append(
                            _validated_termination_report(
                                terminate_watched_processes(
                                    proc.pid,
                                    grace=0.0,
                                    reason="on_spawn_callback_failure",
                                    sampler=sampler,
                                    tracker=tracker,
                                    root_owned=True,
                                ),
                                caller="terminate_watched_processes",
                            )
                        )
                        terminate_direct_child_handle(
                            reason="on_spawn_callback_failure_direct_child_handle"
                        )
                except BaseException as cleanup_error:
                    callback_error.add_note(
                        "owned-tree cleanup after on_spawn failure: "
                        f"{type(cleanup_error).__name__}: {cleanup_error}"
                    )
                raise

        last_sample_cost_s = 0.0

        def sample_tracked_tree(
            *,
            timeout_deadline: bool = False,
            allow_transient_timeout: bool = False,
        ) -> tuple[Mapping[int, ProcessSample], set[int]] | None:
            nonlocal guard_interrupted, last_sample_cost_s
            nonlocal remembered_samples, remembered_watched
            nonlocal sampling_attempts, sampling_successes
            nonlocal sampling_wall_time_s, sampling_cpu_time_s
            nonlocal max_sampling_wall_time_s, max_sampling_cpu_time_s
            nonlocal sampling_process_rows, max_sampling_process_rows
            active_sampler = _timeout_sampler(sampler) if timeout_deadline else sampler
            sample_started_ns = time.perf_counter_ns()
            sample_cpu_started_ns = time.process_time_ns()
            sampling_attempts += 1

            def record_sampling_cost(process_rows: int = 0) -> None:
                nonlocal last_sample_cost_s
                nonlocal sampling_wall_time_s, sampling_cpu_time_s
                nonlocal max_sampling_wall_time_s, max_sampling_cpu_time_s
                nonlocal sampling_process_rows, max_sampling_process_rows
                wall_cost = (time.perf_counter_ns() - sample_started_ns) / 1_000_000_000
                cpu_cost = (time.process_time_ns() - sample_cpu_started_ns) / 1_000_000_000
                last_sample_cost_s = wall_cost
                sampling_wall_time_s += wall_cost
                sampling_cpu_time_s += cpu_cost
                max_sampling_wall_time_s = max(max_sampling_wall_time_s, wall_cost)
                max_sampling_cpu_time_s = max(max_sampling_cpu_time_s, cpu_cost)
                sampling_process_rows += process_rows
                max_sampling_process_rows = max(
                    max_sampling_process_rows,
                    process_rows,
                )

            try:
                samples = active_sampler()
            except WindowsProcessSnapshotTimeout as exc:
                record_sampling_cost()
                if allow_transient_timeout:
                    record_transient_sampling_failure(exc)
                    return None
                terminate_after_sampling_failure(reason="sampler_timeout")
                raise
            except KeyboardInterrupt:
                record_sampling_cost()
                guard_interrupted = True
                terminate_after_sampling_failure(reason="guard_interrupted")
                with contextlib.suppress(subprocess.TimeoutExpired):
                    proc.wait(timeout=termination_wait_s)
                return remembered_samples or {}, set(remembered_watched or ())
            except Exception:
                record_sampling_cost()
                terminate_after_sampling_failure(reason="sampler_failure")
                raise
            record_sampling_cost(len(samples))
            sampling_successes += 1
            watched = tracker.update(samples)
            remembered_samples = samples
            remembered_watched = set(watched)
            return samples, watched

        baseline_authoritative = not cleanup_orphans
        if cleanup_orphans:
            baseline_snapshot = sample_tracked_tree(allow_transient_timeout=True)
            if baseline_snapshot is not None and not guard_interrupted:
                baseline_samples, _baseline_watched = baseline_snapshot
                baseline_pgids = _live_process_group_ids(baseline_samples)
                baseline_authoritative = True

        while not guard_interrupted:
            if os.name == "posix" and hasattr(os, "wait4"):
                exited_usage = _poll_wait4_child(proc)
                if exited_usage is not None:
                    child_exit_usage = exited_usage
                    break
            elif proc.poll() is not None:
                break
            now = time.monotonic()
            if guard_signal is not None:
                signal_snapshot = sample_tracked_tree()
                assert signal_snapshot is not None
                samples, watched = signal_snapshot
                if guard_interrupted:
                    break
                _update_active_guard_marker(
                    guard_marker,
                    guard_token,
                    status="guard_signal_terminating",
                    child_process=guarded_child_process_payload(child_process),
                    guard_signal=guard_signal,
                    elapsed_s=now - start,
                )
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
                timeout_snapshot = sample_tracked_tree(timeout_deadline=True)
                assert timeout_snapshot is not None
                samples, watched = timeout_snapshot
                if guard_interrupted:
                    break
                _update_active_guard_marker(
                    guard_marker,
                    guard_token,
                    status="timeout_terminating",
                    child_process=guarded_child_process_payload(child_process),
                    elapsed_s=now - start,
                    timeout_s=timeout,
                )
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
                _update_active_guard_marker(
                    guard_marker,
                    guard_token,
                    status="child_running",
                    child_process=guarded_child_process_payload(child_process),
                    elapsed_s=now - start,
                    last_keepalive_at=_utc_timestamp(),
                )
                assert keepalive_interval is not None
                next_keepalive = now + keepalive_interval
            snapshot = sample_tracked_tree(allow_transient_timeout=True)
            if snapshot is None:
                exited_usage = _poll_wait4_child(proc)
                if exited_usage is not None:
                    child_exit_usage = exited_usage
                    break
                if os.name != "posix" and proc.poll() is not None:
                    break
                elapsed = time.monotonic() - start
                wait_timeout = paced_poll_interval(
                    poll_interval,
                    last_sample_cost_s,
                )
                if timeout is not None:
                    wait_timeout = max(
                        0.0,
                        min(wait_timeout, timeout - elapsed),
                    )
                if os.name == "posix" and hasattr(os, "wait4"):
                    time.sleep(wait_timeout)
                else:
                    try:
                        proc.wait(timeout=wait_timeout)
                        break
                    except subprocess.TimeoutExpired:
                        pass
                continue
            samples, watched = snapshot
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
                _update_active_guard_marker(
                    guard_marker,
                    guard_token,
                    status="rss_limit_terminating",
                    child_process=guarded_child_process_payload(child_process),
                    violation=_rss_record_payload(violation),
                    peak=_rss_record_payload(observed_peak),
                    peak_total=_rss_record_payload(observed_total),
                    limit_at_violation=memory_limits_payload(current_limits),
                    elapsed_s=now - start,
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
            paced_interval = paced_poll_interval(poll_interval, last_sample_cost_s)
            wait_timeout = (
                min(paced_interval, DEFAULT_FAST_START_POLL_INTERVAL_SEC)
                if elapsed < DEFAULT_FAST_START_DURATION_SEC
                else paced_interval
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
                    post_loop_snapshot = sample_tracked_tree()
                    assert post_loop_snapshot is not None
                    samples, watched = post_loop_snapshot
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
                        # The Popen handle is direct-child custody even when a
                        # platform sampler cannot prove a stable PID identity.
                        # Reap that one child without extending authority to
                        # any unverified descendant.
                        terminate_direct_child_handle(
                            reason=("post_loop_unreaped_child_direct_child_handle")
                        )
            if cleanup_orphans and not guard_interrupted:
                try:
                    tracked_orphans = cleanup_tracked_orphans(
                        proc.pid,
                        tracker=tracker,
                        sampler=sampler,
                        grace=0.25,
                    )
                    termination_reports.extend(
                        _validated_termination_reports(
                            tracked_orphans.termination_reports,
                            caller="cleanup_tracked_orphans",
                        )
                    )
                    orphaned_groups = set(tracked_orphans.process_groups)
                    if baseline_authoritative:
                        repo_orphans = cleanup_repo_scoped_orphans_since_baseline(
                            baseline_pgids=baseline_pgids,
                            tracker=tracker,
                            sampler=sampler,
                            grace=0.25,
                        )
                        termination_reports.extend(
                            _validated_termination_reports(
                                repo_orphans.termination_reports,
                                caller="cleanup_repo_scoped_orphans_since_baseline",
                            )
                        )
                        orphaned_groups.update(repo_orphans.process_groups)
                    orphaned_process_groups = tuple(sorted(orphaned_groups))
                except WindowsProcessSnapshotTimeout as exc:
                    # The child has already exited.  A telemetry timeout cannot
                    # authorize PID/group cleanup, and it must not rewrite the
                    # healthy child's return code.  Record the custody gap and
                    # leave the Job Object/orphan reaper as the safety net.
                    record_transient_sampling_failure(
                        exc,
                        attempt_already_counted=False,
                    )
            if guard_job is not None:
                _update_active_guard_marker(
                    guard_marker,
                    guard_token,
                    status="windows_job_draining",
                    child_process=guarded_child_process_payload(child_process),
                    child_returncode=proc.returncode,
                )
                windows_job_cleanup = _win_job.complete_job_custody(
                    guard_job,
                    timeout=termination_wait_s,
                )
            if stdin_thread is not None:
                stdin_thread.join(timeout=1.0)
            if stdout_capture is not None:
                stdout_capture.flush()
                if capture_tail_bytes is not None and stdout_capture_path is not None:
                    with Path(stdout_capture_path).open("rb") as tail_handle:
                        tail_handle.seek(0, os.SEEK_END)
                        tail_handle.seek(max(0, tail_handle.tell() - capture_tail_bytes))
                        tail_data = tail_handle.read()
                    stdout = tail_data.decode(encoding, errors=errors) if text else tail_data
                else:
                    stdout_capture.seek(0)
                    stdout = stdout_capture.read()
            if stderr_capture is not None:
                stderr_capture.flush()
                if capture_tail_bytes is not None and stderr_capture_path is not None:
                    with Path(stderr_capture_path).open("rb") as tail_handle:
                        tail_handle.seek(0, os.SEEK_END)
                        tail_handle.seek(max(0, tail_handle.tell() - capture_tail_bytes))
                        tail_data = tail_handle.read()
                    stderr = tail_data.decode(encoding, errors=errors) if text else tail_data
                else:
                    stderr_capture.seek(0)
                    stderr = stderr_capture.read()
        finally:
            if stdout_capture is not None:
                stdout_capture.close()
            if stderr_capture is not None:
                stderr_capture.close()
        finished = time.monotonic()
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
        final_sampling_telemetry = sampling_telemetry()
        if final_sampling_telemetry.transient_failures:
            stderr = _append_guard_message(
                stderr,
                "memory_guard: telemetry degraded: "
                f"{final_sampling_telemetry.transient_failures} of "
                f"{final_sampling_telemetry.attempts} process snapshots timed "
                "out; the child result is preserved, but RSS enforcement was "
                "unobserved during those intervals. See sampling_telemetry in "
                "the summary JSON.\n",
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
        peak_job_commit_bytes = (
            None
            if windows_job_cleanup is None
            else windows_job_cleanup.after.peak_job_commit_bytes
        )
        result = GuardResult(
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
            sampling_telemetry=final_sampling_telemetry,
            peak_job_commit_bytes=peak_job_commit_bytes,
            windows_job_cleanup=windows_job_cleanup,
        )
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="completed",
            returncode=result.returncode,
            timed_out=result.timed_out,
            elapsed_s=result.elapsed_s,
            violation=_rss_record_payload(result.violation),
            peak=_rss_record_payload(result.peak),
            peak_total=_rss_record_payload(result.peak_total),
            orphaned_process_groups=list(result.orphaned_process_groups),
            child_process=guarded_child_process_payload(result.child_process),
            termination_reports=termination_reports_payload(result.termination_reports),
            cargo_incremental_quarantine=_cargo_incremental_quarantine_payload(
                result.cargo_incremental_quarantine
            ),
            sampling_telemetry=_sampling_telemetry_payload(result.sampling_telemetry),
            windows_job_cleanup=windows_job_cleanup_payload(result.windows_job_cleanup),
            limit_at_violation=(
                None
                if result.limit_at_violation is None
                else memory_limits_payload(result.limit_at_violation)
            ),
            guard_signal=(
                None
                if result.guard_signal is None
                else _exit_signal_payload(128 + result.guard_signal)
            ),
        )
        return result
    except BaseException as exc:
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="guard_exception",
            exception_type=type(exc).__name__,
            exception=str(exc),
            child_process=guarded_child_process_payload(child_process),
            child_returncode=None if proc is None else proc.returncode,
            termination_reports=termination_reports_payload(tuple(termination_reports)),
        )
        raise
    finally:
        if proc is not None and proc.poll() is None:
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="finalizer_cleanup",
                child_process=guarded_child_process_payload(child_process),
                child_returncode=proc.returncode,
                termination_reports=termination_reports_payload(
                    tuple(termination_reports)
                ),
            )
            with contextlib.suppress(Exception):
                termination_reports.append(
                    _validated_termination_report(
                        terminate_watched_processes(
                            proc.pid,
                            grace=0.0,
                            reason="run_guarded_finalizer",
                            sampler=sample_processes if guard_interrupted else sampler,
                            tracker=tracker,
                            root_owned=True,
                        ),
                        caller="terminate_watched_processes",
                    )
                )
            with contextlib.suppress(Exception):
                proc.wait(timeout=termination_wait_seconds(env))
            with contextlib.suppress(Exception):
                if proc.poll() is None:
                    terminate_direct_child_handle(
                        reason="run_guarded_finalizer_direct_child_handle"
                    )
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="finalizer_completed",
                child_process=guarded_child_process_payload(child_process),
                child_returncode=proc.returncode,
                termination_reports=termination_reports_payload(
                    tuple(termination_reports)
                ),
            )
        if stdout_capture is not None and not getattr(stdout_capture, "closed", False):
            stdout_capture.close()
        if stderr_capture is not None and not getattr(stderr_capture, "closed", False):
            stderr_capture.close()
        if launch is not None:
            _close_fds((launch.started_read_fd,))
        _restore_guard_signal_handlers()
        # Drop the job handle last. Successful execution has already proven the
        # exact Job empty; KILL_ON_JOB_CLOSE remains the crash-only safety net.
        _win_job.close_job(guard_job)


_REPRO_ENV_KEYS = _repro_context.REPRO_ENV_KEYS
_REPRO_ENV_PREFIXES = _repro_context.REPRO_ENV_PREFIXES
_SECRET_ENV_TOKENS = _repro_context.SECRET_ENV_TOKENS
_PYTEST_CURRENT_TEST_FILE_ENV = "MOLT_PYTEST_CURRENT_TEST_FILE"
_PYTEST_CURRENT_TEST_FILE_MAX_BYTES = _repro_context.PYTEST_CURRENT_TEST_FILE_MAX_BYTES
_PYTEST_CURRENT_TEST_WORKER_MAX_FILES = (
    _repro_context.PYTEST_CURRENT_TEST_WORKER_MAX_FILES
)
_PYTEST_COMMAND_NAMES = _repro_context.PYTEST_COMMAND_NAMES
_safe_repro_env_key = _repro_context._safe_repro_env_key
_safe_repro_env_value = _repro_context._safe_repro_env_value
_safe_repro_env = _repro_context._safe_repro_env


def _safe_repro_env_delta(
    environ: Mapping[str, str],
    *,
    baseline: Mapping[str, str] | None = None,
) -> dict[str, object]:
    return _repro_context._safe_repro_env_delta(
        environ,
        baseline=os.environ if baseline is None else baseline,
    )


def _process_sample_payload(sample: ProcessSample) -> dict[str, object]:
    return _repro_context._process_sample_payload(sample)


def process_sample_payload(sample: ProcessSample) -> dict[str, object]:
    return _process_sample_payload(sample)


def _bounded_process_sample_payload(
    sample: ProcessSample,
    *,
    max_command_chars: int = 512,
) -> dict[str, object]:
    return _repro_context._bounded_process_sample_payload(
        sample,
        max_command_chars=max_command_chars,
    )


def _host_control_plane_payload(
    samples: Mapping[int, ProcessSample],
    *,
    max_samples: int = 32,
) -> dict[str, object] | None:
    return _repro_context._host_control_plane_payload(
        samples,
        sample_pgid=_sample_pgid,
        is_host_control_plane_process=is_host_control_plane_process,
        protected_process_group_ids=_current_protected_process_group_ids,
        max_samples=max_samples,
    )


def _process_lineage_payload(
    samples: Mapping[int, ProcessSample],
    *,
    pid: int,
    max_depth: int = 8,
) -> list[dict[str, object]]:
    return _repro_context._process_lineage_payload(
        samples,
        pid=pid,
        max_depth=max_depth,
    )


def _path_is_under(path: Path, root: Path) -> bool:
    return _repro_context._path_is_under(path, root)


def _pytest_custody_artifact_path(
    kind: str,
    suffix: str,
    *,
    pid: int | None = None,
) -> Path:
    return _repro_context._pytest_custody_artifact_path(
        kind,
        suffix,
        summary_dir=PYTEST_OUTER_GUARD_SUMMARY_DIR,
        pid=os.getpid() if pid is None else pid,
    )


def _canonical_pytest_current_test_file_path(raw_path: str | None = None) -> Path:
    return _repro_context._canonical_pytest_current_test_file_path(
        raw_path,
        root=ROOT,
        summary_dir=PYTEST_OUTER_GUARD_SUMMARY_DIR,
        fallback_pid=os.getpid(),
    )


def _looks_like_repo_test_path(raw: str, cwd: str | Path | None) -> bool:
    return _repro_context._looks_like_repo_test_path(raw, cwd, root=ROOT)


def _command_requests_test_custody(
    command: Sequence[str],
    *,
    cwd: str | Path | None = None,
) -> bool:
    return _repro_context._command_requests_test_custody(
        command,
        cwd=cwd,
        root=ROOT,
    )


def test_custody_launch_env(
    command: Sequence[str],
    *,
    environ: Mapping[str, str] | None = None,
    cwd: str | Path | None = None,
) -> dict[str, str]:
    return _repro_context.test_custody_launch_env(
        command,
        environ=os.environ if environ is None else environ,
        cwd=cwd,
        root=ROOT,
        summary_dir=PYTEST_OUTER_GUARD_SUMMARY_DIR,
        fallback_pid=os.getpid(),
        current_test_file_env=_PYTEST_CURRENT_TEST_FILE_ENV,
    )


def _read_pytest_current_test_json(path: Path) -> dict[str, object]:
    return _repro_context._read_pytest_current_test_json(path)


def _lineage_pid_set(
    samples: Mapping[int, ProcessSample],
    *,
    pid: int,
    max_depth: int = 16,
) -> set[int]:
    return _repro_context._lineage_pid_set(
        samples,
        pid=pid,
        max_depth=max_depth,
    )


def _pytest_worker_record_payloads(
    aggregate_path: Path,
    *,
    samples: Mapping[int, ProcessSample],
    incident_pid: int | None,
) -> list[dict[str, object]]:
    return _repro_context._pytest_worker_record_payloads(
        aggregate_path,
        samples=samples,
        incident_pid=incident_pid,
    )


def _pytest_current_test_file_payload(
    environ: Mapping[str, str],
    *,
    samples: Mapping[int, ProcessSample],
    incident_pid: int | None = None,
) -> dict[str, object] | None:
    return _repro_context._pytest_current_test_file_payload(
        environ,
        samples=samples,
        incident_pid=incident_pid,
        root=ROOT,
        summary_dir=PYTEST_OUTER_GUARD_SUMMARY_DIR,
        current_test_file_env=_PYTEST_CURRENT_TEST_FILE_ENV,
    )


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
    samples = sample_processes()
    pid = os.getpid()
    parent_pid = os.getppid()
    return _repro_context.repro_context_payload(
        command=command,
        cwd=cwd,
        source_environ=source,
        baseline_environ=os.environ,
        root=ROOT,
        summary_dir=PYTEST_OUTER_GUARD_SUMMARY_DIR,
        current_test_file_env=_PYTEST_CURRENT_TEST_FILE_ENV,
        samples=samples,
        pid=pid,
        parent_pid=parent_pid,
        current_process_group_id=_safe_getpgrp(),
        current_session_id=_safe_getsid(0),
        parent_process_group_id=_safe_getpgid(parent_pid),
        argv=sys.argv,
        python_executable=sys.executable,
        python_version=sys.version.split()[0],
        platform_name=sys.platform,
        platform_detail=_platform_detail_no_subprocess(),
        machine=platform.machine(),
        sample_pgid=_sample_pgid,
        is_host_control_plane_process=is_host_control_plane_process,
        protected_process_group_ids=_current_protected_process_group_ids,
        max_process_rss_kb=max_process_rss_kb,
        max_total_rss_kb=max_total_rss_kb,
        max_global_rss_kb=max_global_rss_kb,
        child_rlimit_kb=child_rlimit_kb,
        timeout_s=timeout_s,
        poll_interval_s=poll_interval_s,
        summary_json=summary_json,
        incident_pid=incident_pid,
    )


def repro_context_line(payload: Mapping[str, object]) -> str:
    return _repro_context.repro_context_line(payload)


def exit_signal_payload(returncode: int) -> dict[str, object] | None:
    return _returncode_signal_payload(returncode)


_exit_signal_payload = exit_signal_payload


def _incident_payload(result: GuardResult) -> dict[str, object] | None:
    return _reporting.incident_payload(result, signal_payload=_exit_signal_payload)


def _write_summary_json(path: str, **kwargs: object) -> None:
    _reporting.write_summary_json(
        path,
        **kwargs,
        signal_payload=_exit_signal_payload,
        repro_context_provider=repro_context_payload,
    )


def _write_worker_exit_summary_json(
    path: str,
    *,
    worker_returncode: int,
) -> bool:
    return _reporting.write_worker_exit_summary_json(
        path,
        worker_returncode=worker_returncode,
        signal_payload=_exit_signal_payload,
    )


def _default_incident_summary_path() -> Path:
    return _reporting.default_incident_summary_path(ROOT)


def _prune_default_incident_summaries(
    directory: Path,
    *,
    keep: int = DEFAULT_INCIDENT_SUMMARY_KEEP,
) -> None:
    _reporting.prune_default_incident_summaries(directory, keep=keep)


def _write_running_summary_json(path: str, **kwargs: object) -> None:
    _reporting.write_running_summary_json(
        path,
        **kwargs,
        repro_context_provider=repro_context_payload,
    )


def _parser() -> argparse.ArgumentParser:
    return _cli_contract.parser(
        default_poll_interval_sec=DEFAULT_POLL_INTERVAL_SEC,
        default_samples_max_mb=DEFAULT_SAMPLES_MAX_MB,
        hard_max_rss_gb=DEFAULT_HARD_MAX_RSS_GB,
        hard_max_global_rss_gb=DEFAULT_HARD_MAX_GLOBAL_RSS_GB,
    )


def _load_internal_command(environ: Mapping[str, str]) -> list[str] | None:
    return _cli_contract.load_internal_command(
        environ,
        worker_env_name=INTERNAL_WORKER_ENV,
        command_env_name=INTERNAL_COMMAND_ENV,
    )


def _child_env_without_internal_keys(environ: Mapping[str, str]) -> dict[str, str]:
    return _cli_contract.child_env_without_internal_keys(
        environ,
        internal_env_keys=_INTERNAL_ENV_KEYS,
    )


def _worker_env(environ: Mapping[str, str], command: Sequence[str]) -> dict[str, str]:
    return _cli_contract.worker_env(
        environ,
        command,
        worker_env_name=INTERNAL_WORKER_ENV,
        command_env_name=INTERNAL_COMMAND_ENV,
    )


def _worker_argv(args: argparse.Namespace) -> list[str]:
    return _cli_contract.worker_argv(
        args,
        python_executable=sys.executable,
        script_path=Path(__file__).resolve(),
    )


def main(
    argv: Sequence[str] | None = None,
    *,
    hide_command_argv: bool = False,
    execve: Callable[[str, Sequence[str], Mapping[str, str]], object] = os.execve,
    environ: Mapping[str, str] | None = None,
) -> int:
    current_env = os.environ if environ is None else environ
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
        max_global_rss_gb = (
            budget.max_global_rss_gb
            if args.max_global_rss_gb is None
            else float(args.max_global_rss_gb)
        )
        max_global_rss_kb = max_global_rss_kb_from_gb(max_global_rss_gb)
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
                max_global_rss_gb=max_global_rss_gb,
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
                **inherit_stdio_kwargs(),
                **_guarded_popen_process_isolation_kwargs(),
            )
            if args.summary_json:
                try:
                    _write_worker_exit_summary_json(
                        args.summary_json,
                        worker_returncode=int(completed.returncode),
                    )
                except OSError as exc:
                    print(
                        "memory_guard: failed to terminalize worker-exit "
                        f"summary JSON: {exc}",
                        file=sys.stderr,
                    )
                    if completed.returncode == 0:
                        return 2
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
        running_summary_json=args.summary_json,
        running_summary_environ=current_env,
        running_summary_max_global_rss_kb=max_global_rss_kb,
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
    _reporting.emit_terminal_report(
        result,
        timeout_s=args.timeout,
        max_rss_gb=max_rss_gb,
        max_total_rss_gb=max_total_rss_gb,
        repro_payload=repro_payload,
        signal_payload=_exit_signal_payload,
        stderr=sys.stderr,
    )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(hide_command_argv=True))

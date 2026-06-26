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
    WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES as WINDOWS_FULL_COMMAND_LINE_EXECUTABLE_NAMES,
    _filetime_to_unix_seconds as _filetime_to_unix_seconds,
    _windows_process_needs_full_command_line as _windows_process_needs_full_command_line,
    _windows_process_snapshot_rows_hard_timeout as _windows_process_snapshot_rows_hard_timeout,
    _windows_process_snapshot_rows as _windows_process_snapshot_rows,
)
from tools.memory_guard_core import process_model as _process_model  # noqa: E402
from tools.memory_guard_core import process_custody as _process_custody  # noqa: E402
from tools.memory_guard_core import repro_context as _repro_context  # noqa: E402
from tools.memory_guard_core.process_custody import (  # noqa: E402
    ChildExitResourceUsage as ChildExitResourceUsage,
    GuardOrphanCleanupResult as GuardOrphanCleanupResult,
    GuardResult as GuardResult,
    GuardTerminationAction as GuardTerminationAction,
    GuardTerminationReport as GuardTerminationReport,
    GuardedChildProcess as GuardedChildProcess,
    GuardedLaunch as GuardedLaunch,
    ProcessIdentity as ProcessIdentity,
    ProcessSample as ProcessSample,
    ProcessTreeTracker as ProcessTreeTracker,
    RssViolation as RssViolation,
    _ancestor_pids as _ancestor_pids,
    _command_executable_name as _command_executable_name,
    _current_protected_process_group_ids as _current_protected_process_group_ids,
    _elapsed_seconds_from_ps as _elapsed_seconds_from_ps,
    _filter_protected_watched_pids as _filter_protected_watched_pids,
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
    _terminate_single_pid as _terminate_single_pid,
    _terminate_single_process_group as _terminate_single_process_group,
    _termination_action as _termination_action,
    cleanup_repo_scoped_orphans_since_baseline as cleanup_repo_scoped_orphans_since_baseline,
    cleanup_tracked_orphans as cleanup_tracked_orphans,
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
    sample_processes as sample_processes,
    sample_processes_posix as sample_processes_posix,
    sample_processes_windows as sample_processes_windows,
    sample_processes_windows_hard_timeout as sample_processes_windows_hard_timeout,
    signal_payload as signal_payload,
    term_signal_payload as term_signal_payload,
    terminate_verified_pid as terminate_verified_pid,
    terminate_watched_processes as terminate_watched_processes,
    total_rss as total_rss,
    watched_pids as watched_pids,
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
HOST_CONTROL_PLANE_TOKENS = _process_model.HOST_CONTROL_PLANE_TOKENS
HOST_CONTROL_PLANE_EXECUTABLE_NAMES = _process_model.HOST_CONTROL_PLANE_EXECUTABLE_NAMES


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
        _is_windows_process_model() and returncode in WINDOWS_PROCESS_SIGNAL_EXIT_CODES
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
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="launch_prepared",
            launch_command=list(launch.command),
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
        except Exception as exc:
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="spawn_failed",
                launch_command=list(launch.command),
                spawn_error_type=type(exc).__name__,
                spawn_error=str(exc),
            )
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
                samples, watched = sample_tracked_tree()
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
                    terminate_watched_processes(
                        proc.pid,
                        grace=0.0,
                        reason="run_guarded_finalizer",
                        sampler=sample_processes if guard_interrupted else sampler,
                    )
                )
            with contextlib.suppress(Exception):
                proc.wait(timeout=termination_wait_seconds(env))
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
        platform_detail=platform.platform(),
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

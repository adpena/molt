#!/usr/bin/env python3
from __future__ import annotations

import argparse
import atexit
from collections.abc import Callable, Mapping, Sequence
import contextlib
from dataclasses import replace
from functools import partial
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
from types import FrameType
from typing import Any, Unpack, cast


_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)
from molt.process_environment import (  # noqa: E402
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)

DEFAULT_POLL_INTERVAL_SEC = 0.10
DEFAULT_FAST_START_POLL_INTERVAL_SEC = 0.02
DEFAULT_FAST_START_DURATION_SEC = 2.0
DEFAULT_TERMINATION_WAIT_SEC = 2.0
# How long the enforcer thread lets the run loop deliver a deadline, memory
# trip or cancellation before it signals the child's own process group itself
# (HF-417). A healthy loop delivered a timeout 0.10-0.41 s after its deadline
# at host load 120 (2026-10-21), so the enforcer stays idle unless the loop
# is stalled; the slack bounds the guard's own lateness, not the child.
ENFORCEMENT_SLACK_SEC = 2.0
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


def closure_progress_notifier(path: str | Path) -> Callable[[str], None]:
    """Return the notifier that tells a cancelling owner this guard progresses.

    Each notice appends one line to the launch's progress file. The owner
    renews its cancellation observation window only when a notice arrived
    within it (systemd's ``EXTEND_TIMEOUT_USEC`` rule), so a guard that is
    slow under CPU load is still observed, and a window without a notice is a
    stall. A notice that cannot be written is dropped: the owner then reports
    an unresolved observation, never a false closure.
    """
    target = Path(path)

    def notify(step: str) -> None:
        try:
            with target.open("a", encoding="utf-8") as handle:
                handle.write(f"{time.monotonic_ns()} {step}\n")
        except OSError:
            pass

    return notify


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

SRC_ROOT = ROOT / "src"

from tools.memory_guard_core.common import utc_timestamp as _utc_timestamp  # noqa: E402
from tools.memory_guard_core.active_custody import (  # noqa: E402
    ACTIVE_GUARD_MARKER_SCHEMA_VERSION,
    command_artifact_scope,
    guard_marker_records,
    observe_custody,
    retire_active_guard_marker,
    sweep_active_guard_markers,
    update_active_guard_marker,
    write_active_guard_marker,
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
    _float_env as _float_env,
    _gb_from_bytes as _gb_from_bytes,
    _prefixed_names as _prefixed_names,
    adaptive_memory_budget as adaptive_memory_budget,
    available_memory_bytes as available_memory_bytes,
    child_rlimit_kb_from_gb as child_rlimit_kb_from_gb,
    default_child_rlimit_gb as default_child_rlimit_gb,
    normalize_env_prefix as normalize_env_prefix,
    max_global_rss_kb_from_gb as max_global_rss_kb_from_gb,
    max_rss_kb_from_gb as max_rss_kb_from_gb,
    physical_memory_bytes as physical_memory_bytes,
    resolve_memory_limits as resolve_memory_limits,
)
from tools.memory_guard_core.payloads import (  # noqa: E402
    _rss_record_payload as _rss_record_payload,
    guarded_child_process_payload as guarded_child_process_payload,
    process_identities_payload as process_identities_payload,
    memory_limits_payload as memory_limits_payload,
    termination_action_payload as termination_action_payload,
    termination_report_payload as termination_report_payload,
    termination_reports_payload as termination_reports_payload,
    windows_job_cleanup_payload as windows_job_cleanup_payload,
)
from tools.memory_guard_core.sample_records import (  # noqa: E402
    DEFAULT_SAMPLES_MAX_MB as DEFAULT_SAMPLES_MAX_MB,
    _format_sample_payload as _format_sample_payload,
    _record_gb as _record_gb,
    _record_sample as _record_sample,
    _rotate_jsonl_if_needed as _rotate_jsonl_if_needed,
    _samples_max_bytes_from_mb as _samples_max_bytes_from_mb,
    _stream_sample_payload as _stream_sample_payload,
)
from tools.memory_guard_core.cargo_quarantine import (  # noqa: E402
    CargoIncrementalQuarantine as CargoIncrementalQuarantine,
    CargoIncrementalObservation as CargoIncrementalObservation,
    observe_owned_incremental_state as observe_owned_incremental_state,
    CargoInterruptionInventory,
    CARGO_COMPILER_EXECUTABLES,
    observe_cargo_interruption_inventory,
    CargoIncrementalQuarantineMove as CargoIncrementalQuarantineMove,
    _cargo_incremental_dirs as _cargo_incremental_dirs,
    _cargo_incremental_quarantine_message as _cargo_incremental_quarantine_message,
    _cargo_recovery_next_action as _cargo_recovery_next_action,
    _cargo_incremental_quarantine_payload as _cargo_incremental_quarantine_payload,
    _cargo_quarantine_id as _cargo_quarantine_id,
    _cargo_quarantine_parent as _cargo_quarantine_parent,
    _cargo_quarantine_payload_required as _cargo_quarantine_payload_required,
    _cargo_target_dir as _cargo_target_dir,
    _command_invokes_cargo_build_state as _command_invokes_cargo_build_state,
    _command_tokens as _command_tokens,
    _effective_guard_cwd as _effective_guard_cwd,
    _quarantine_cargo_incremental_state as _quarantine_cargo_incremental_state,
    _samples_include_cargo_build_state as _samples_include_cargo_build_state,
    _token_executable_name as _token_executable_name,
    _write_cargo_quarantine_receipt as _write_cargo_quarantine_receipt,
)
from molt.windows_process_snapshot import (  # noqa: E402
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
from molt.process_spawn import (  # noqa: E402
    ProcessGroupKwargs,
    ProcessSpawnKwargs,
    detached_process_group_kwargs,
    inherit_stdio_kwargs,
    native_popen_command,
)
from molt import temporary_artifacts as _temporary_artifacts  # noqa: E402
from molt.llvm_linker_roles import lexical_executable_path  # noqa: E402
from molt.metric_ratios import RatioDirection, signed_ratio_value  # noqa: E402
from tools import win_job as _win_job  # noqa: E402

WindowsJobCleanup = _win_job.WindowsJobCleanup
from molt import process_model as _process_model  # noqa: E402
from tools.memory_guard_core import cli_contract as _cli_contract  # noqa: E402
from tools.memory_guard_core import repro_context as _repro_context  # noqa: E402
from tools.memory_guard_core.worker_peaks import WorkerPeakTracker  # noqa: E402
from tools.memory_guard_core import reporting as _reporting  # noqa: E402
from molt.memory_guard_paths import (  # noqa: E402
    active_guard_marker_dir,
    memory_guard_state_root,
)
from molt.process_model import (  # noqa: E402
    sample_processes as sample_processes,
    sample_processes_custody as sample_processes_custody,
)
from tools.memory_guard_core.process_custody import (  # noqa: E402
    terminate_watched_processes as terminate_watched_processes,
    cleanup_tracked_orphans as cleanup_tracked_orphans,
    _terminate_single_pid as _terminate_single_pid,
    ChildExitResourceUsage as ChildExitResourceUsage,
    GuardOrphanCleanupResult as GuardOrphanCleanupResult,
    GuardInfrastructureFailure as GuardInfrastructureFailure,
    GuardResult as GuardResult,
    GuardSamplingTelemetry as GuardSamplingTelemetry,
    GuardTerminationAction as GuardTerminationAction,
    GuardTerminationReport as GuardTerminationReport,
    GuardedChildProcess as GuardedChildProcess,
    GuardedLaunch as GuardedLaunch,
    MAX_TERMINATION_PID_FANOUT as MAX_TERMINATION_PID_FANOUT,
    OwnedTreeEnforcer as OwnedTreeEnforcer,
    StopRequestWatch as StopRequestWatch,
    completed_enforcement_report as completed_enforcement_report,
    owned_root_group_signal as owned_root_group_signal,
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
    _live_process_group_ids as _live_process_group_ids,
    _pid_exited_or_unobservable as _pid_exited_or_unobservable,
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
    _take_child_exit_usage as _take_child_exit_usage,
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
    reserved_group_exit_census as reserved_group_exit_census,
    RootGroupAbsence as RootGroupAbsence,
    TerminalProcessObservation as TerminalProcessObservation,
    signal_payload as signal_payload,
    take_child_exit_census as take_child_exit_census,
    term_signal_payload as term_signal_payload,
    terminate_verified_pid as terminate_verified_pid,
    termination_report_dispositions as termination_report_dispositions,
    total_rss as total_rss,
    watched_pids as watched_pids,
)

GUARD_RETURN_CODE = 137
TIMEOUT_RETURN_CODE = 124
INFRASTRUCTURE_RETURN_CODE = 125
# An operator signal to the guard asks its tree to stop: the tree gets SIGTERM
# and up to this long to write its own receipts before SIGKILL, the default of
# `docker stop`. A molt_diff suite with two running cases measured 1.4 s when
# signalled itself and 4.2 s through this guard, whose SIGTERM also reaches the
# workers' own guards. Resource trips, timeouts and an owner's cancellation
# still kill at once.
OPERATOR_SIGNAL_GRACE_S = 10.0
INTERNAL_COMMAND_ENV = "MOLT_MEMORY_GUARD_COMMAND_JSON"
INTERNAL_WORKER_ENV = "MOLT_MEMORY_GUARD_INTERNAL"
ACTIVE_ENV = "MOLT_MEMORY_GUARD_ACTIVE"
ACTIVE_GUARD_PID_ENV = "MOLT_MEMORY_GUARD_PID"
ACTIVE_GUARD_TOKEN_ENV = "MOLT_MEMORY_GUARD_TOKEN"
ACTIVE_GUARD_MARKER_ENV = "MOLT_MEMORY_GUARD_MARKER"
_INTERNAL_ENV_KEYS = (
    INTERNAL_COMMAND_ENV,
    INTERNAL_WORKER_ENV,
    _cli_contract.INTERNAL_LAUNCH_ID_ENV,
    _cli_contract.INTERNAL_STARTUP_PATH_ENV,
)
HOST_CONTROL_PLANE_TOKENS = _process_model.HOST_CONTROL_PLANE_TOKENS
HOST_CONTROL_PLANE_EXECUTABLE_NAMES = _process_model.HOST_CONTROL_PLANE_EXECUTABLE_NAMES


def parse_process_table_with_start(text: str) -> dict[int, ProcessSample]:
    return _process_model.parse_process_table_with_start(text)


def _suite_lease_process_ids(environ: Mapping[str, str]) -> frozenset[int]:
    """The owner and guardian pids of the suite lease this run inherits."""
    from molt import backend_daemon_suite_custody as suite_custody

    raw = environ.get(suite_custody.LEASE_ENV)
    if not raw or _process_model.is_windows_process_model():
        return frozenset()
    record = suite_custody.read_lease(Path(raw))
    if record is None:
        return frozenset()
    return frozenset(
        pid
        for pid in (record["owner_pid"], record["guardian_pid"])
        if type(pid) is int and pid > 0
    )


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


def _temporary_artifact_descendant_closure(
    *,
    proc: subprocess.Popen[Any] | None,
    child_process: GuardedChildProcess | None,
    tracker: ProcessTreeTracker | None,
    sampler: Callable[[], Mapping[int, ProcessSample]],
    windows_job_cleanup: WindowsJobCleanup | None,
    windows_process_model: bool,
    posix_process_model: bool,
    cleanup_orphans: bool,
    guard_interrupted: bool,
    termination_wait_expired: bool,
    sampling_telemetry: GuardSamplingTelemetry | None,
    termination_reports: Sequence[GuardTerminationReport],
    probe_grace: float,
    active_dir: Path,
    final_observation: TerminalProcessObservation | None = None,
) -> tuple[bool, dict[str, object]]:
    """Return non-actuating evidence that no guarded child can still use scratch.

    Windows Job accounting is exact.  The POSIX authority is intentionally
    named as sampled process-group custody: it combines the guard's complete
    observation history with a final process-group liveness probe and a
    process-table observation after the existing cleanup boundary.
    A complete terminal observation may be reused until a signal. A scoped
    root-group absence is checked again and never supplies a host-table
    verdict for nested guard records; those materialize a host snapshot lazily.
    """

    if proc is None:
        return True, {
            "schema": "molt.guard-scratch-closure.v1",
            "authority": "no-child-launched",
            "closed": True,
            "direct_child_reaped": True,
        }

    direct_child_reaped = proc.returncode is not None
    if windows_process_model:
        closed = bool(
            direct_child_reaped
            and windows_job_cleanup is not None
            and windows_job_cleanup.completed
        )
        return closed, {
            "schema": "molt.guard-scratch-closure.v1",
            "authority": "windows-job-accounting",
            "closed": closed,
            "direct_child_reaped": direct_child_reaped,
            "windows_job_cleanup": windows_job_cleanup_payload(windows_job_cleanup),
        }

    action_gaps = [
        {
            "report": report.reason,
            "target_kind": action.target_kind,
            "target_id": action.target_id,
            "result": action.result,
        }
        for report, disposition in zip(
            termination_reports,
            termination_report_dispositions(termination_reports),
            strict=True,
        )
        for action in disposition.incomplete_actions
    ]
    sampling_complete = bool(
        sampling_telemetry is not None
        and sampling_telemetry.attempts > 0
        and sampling_telemetry.enforcement_complete
    )
    evidence: dict[str, object] = {
        "schema": "molt.guard-scratch-closure.v1",
        "authority": (
            "posix-sampled-process-group" if posix_process_model else "unsupported"
        ),
        "closed": False,
        "direct_child_reaped": direct_child_reaped,
        "guard_interrupted": guard_interrupted,
        "termination_wait_expired": termination_wait_expired,
        "cleanup_orphans_enabled": cleanup_orphans,
        "sampling_enforcement_complete": sampling_complete,
        "termination_action_gaps": action_gaps,
        "termination_report_count": len(termination_reports),
    }
    if (
        not posix_process_model
        or not direct_child_reaped
        or not cleanup_orphans
        or guard_interrupted
        or termination_wait_expired
        or not sampling_complete
        or action_gaps
        or child_process is None
        or child_process.pgid is None
        or tracker is None
    ):
        return False, evidence

    # Include births retired by earlier polls, before the final update can
    # revoke another reused pid. Historical evidence never grants live custody.
    producer_births: dict[int, set[int | None]] = {}
    for pid, identity in tracker.closure_identities():
        producer_births.setdefault(pid, set()).add(identity.started_at_ns)
    current_births = tracker.custody_identities(tracker.known_pids or ())
    for pid in tracker.known_pids or ():
        if pid not in current_births:
            producer_births.setdefault(pid, set()).add(None)
    root_births = producer_births.setdefault(child_process.pid, set())
    if child_process.started_at_ns is not None:
        # The launch record supplies the root identity even before a tracker
        # sample acquired it. An unknown placeholder is not another lifetime.
        root_births.discard(None)
    root_births.add(child_process.started_at_ns)
    observed_at = time.time()
    root_pgid = child_process.pgid
    scoped_absence = isinstance(final_observation, RootGroupAbsence)
    if scoped_absence:
        assert isinstance(final_observation, RootGroupAbsence)
        group_closed = bool(
            final_observation.pgid == root_pgid
            and not (tracker.known_pids or set()) - {child_process.pid}
            and RootGroupAbsence.observe(root_pgid) is not None
        )
    else:
        group_closed = _process_group_exited_or_unobservable(
            root_pgid,
            grace=max(0.02, probe_grace),
        )
    evidence["root_pgid"] = root_pgid
    evidence["root_process_group_closed"] = group_closed
    evidence["final_observation_scope"] = "root-group" if scoped_absence else "host"
    host_samples: Mapping[int, ProcessSample] | None = None
    try:
        if isinstance(final_observation, RootGroupAbsence):
            root_samples: Mapping[int, ProcessSample] = {}
        else:
            host_samples = sampler() if final_observation is None else final_observation
            root_samples = host_samples
        remaining_tracked = tuple(sorted(tracker.update(root_samples)))
        root_group_members = tuple(
            sorted(
                sample.pid
                for sample in root_samples.values()
                if _sample_pgid(sample) == root_pgid
            )
        )
    except (KeyboardInterrupt, Exception) as exc:
        evidence["final_sample_error"] = f"{type(exc).__name__}: {exc}"
        return False, evidence
    evidence["final_sample_process_count"] = len(root_samples)
    evidence["remaining_tracked_pids"] = list(remaining_tracked)
    evidence["root_process_group_members"] = list(root_group_members)
    closed = group_closed and not remaining_tracked and not root_group_members
    if not closed:
        return False, evidence

    # A stopped inline guard can have published another generation during its
    # signal grace. Its old process group and old token do not close that child.
    # Walk only birth-bound custody edges; this observes records, never signals.
    generations: list[dict[str, object]] = []
    pending = producer_births
    examined: set[tuple[int, int | None]] = set()
    try:
        while pending:
            records = guard_marker_records(active_dir, tuple(pending))
            examined.update(
                (pid, birth) for pid, births in pending.items() for birth in births
            )
            following: dict[int, set[int | None]] = {}
            for record in records:
                row: dict[str, object] = {"path": str(record.path)}
                guard = record.guard
                if guard is None:
                    row["error"] = record.error or "guard identity unavailable"
                    closed = False
                elif None in pending[guard.pid] or guard.started_at_ns is None:
                    # A missing birth on either side leaves the relationship
                    # unknown. The marker cannot supply our historical birth.
                    row["error"] = "guard birth unavailable"
                    closed = False
                elif guard.started_at_ns not in pending[guard.pid]:
                    # A validated publisher envelope proves this is a different
                    # lifetime, even when the custody body is unreadable. Keep
                    # that record protective without importing its uncertainty.
                    row["disposition"] = "foreign"
                    row["reason"] = "publisher_birth_differs_from_owned_tree"
                    if record.error is not None:
                        row["error"] = record.error
                elif record.error is not None:
                    row["error"] = record.error
                    closed = False
                else:
                    if host_samples is None:
                        observed_at = time.time()
                        host_samples = sampler()
                        evidence["nested_sample_process_count"] = len(host_samples)
                    observation = observe_custody(
                        record, host_samples, now_s=observed_at
                    )
                    row["disposition"] = observation.state
                    row["reason"] = observation.reason
                    if observation.state != "closed":
                        closed = False
                    elif record.child is not None:
                        child = record.child
                        identity = (child.pid, child.started_at_ns)
                        if identity not in examined:
                            following.setdefault(child.pid, set()).add(
                                child.started_at_ns
                            )
                generations.append(row)
            if not closed:
                break
            pending = following
    except (KeyboardInterrupt, Exception) as exc:
        evidence["guard_generation_error"] = f"{type(exc).__name__}: {exc}"
        closed = False
    evidence["guard_generations"] = generations
    evidence["closed"] = closed
    return closed, evidence


def termination_wait_seconds(env: Mapping[str, str] | None = None) -> float:
    source = os.environ if env is None else env
    for name in ("MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC",):
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


def _write_active_guard_marker(
    pid: int,
    *,
    command: Sequence[str],
    cwd: str | Path | None,
    environ: Mapping[str, str],
    outputs: Sequence[Path] = (),
) -> tuple[str, Path]:
    if type(pid) is not int or pid <= 0:
        raise ValueError("active guard marker requires a live pid")
    token = os.urandom(16).hex()
    marker_dir = active_guard_marker_dir(ROOT, environ)
    marker_dir.mkdir(parents=True, exist_ok=True)
    marker_path = marker_dir / f"guard-{pid}-{token}.json"
    cwd_path = Path.cwd() if cwd is None else Path(cwd).expanduser()
    payload = {
        "schema_version": ACTIVE_GUARD_MARKER_SCHEMA_VERSION,
        "pid": pid,
        "token": token,
        "guard_process": {
            "pid": pid,
            "started_at_ns": _process_model.process_started_at_ns(pid),
        },
        "child_process": None,
        "child_launch_state": "not_started",
        "path": str(Path(__file__).resolve()),
        "command": list(command),
        "cwd": str(cwd_path.resolve(strict=False)),
        "artifact_scope": command_artifact_scope(
            ROOT, command, environ, cwd=cwd_path, outputs=outputs
        ).to_dict(),
        "status": "guard_starting",
        "created_at": _utc_timestamp(),
        "updated_at": _utc_timestamp(),
    }
    write_active_guard_marker(marker_path, payload)
    # A launch never touches another execution's record. Each marker leaves
    # active/ through its own retirement (_retire_guard_custody) or through
    # evidence-based reconciliation, never by age or count.
    return token, marker_path


# Captured at import: the custody sweep must read the native process table,
# never a sampler that a caller or test substituted later.
_NATIVE_PROCESS_SNAPSHOT = _process_model.sample_processes


def _retire_guard_custody(marker_path: Path, token: str) -> None:
    """Retire this guard's resolved marker, then sweep stale custody records.

    Both steps run after the guarded result is final. A failure changes no
    record's protection: the marker stays in active/, and the next sweep or
    ``tools/memory_guard_custody.py --apply`` repeats the step.
    """
    try:
        retire_active_guard_marker(marker_path, token)
        sweep_active_guard_markers(marker_path.parent, _NATIVE_PROCESS_SNAPSHOT)
    except (OSError, ValueError, RuntimeError):
        return


def _update_active_guard_marker(
    marker_path: Path,
    token: str,
    *,
    status: str,
    **fields: object,
) -> bool:
    try:
        return update_active_guard_marker(marker_path, token, status=status, **fields)
    except (OSError, ValueError, RuntimeError):
        return False


def _apply_child_resource_limit(limit_kb: int) -> None:
    if limit_kb <= 0 or sys.platform == "win32":
        return
    try:
        import resource
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
        os.write(fd, f"{time.perf_counter_ns()}\n".encode("ascii"))
    except OSError:
        pass
    with contextlib.suppress(OSError):
        os.close(fd)


def _resolve_relative_executable(
    command: Sequence[str], *, parent_cwd: str | Path | None = None
) -> list[str]:
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
    base = Path.cwd() if parent_cwd is None else Path(parent_cwd)
    # The selected entrypoint is semantic: resolving a venv Python symlink
    # selects the base environment, and resolving a driver alias changes its role.
    resolved = lexical_executable_path(base / candidate)
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


def _guarded_popen_process_isolation_kwargs() -> ProcessGroupKwargs:
    return detached_process_group_kwargs(
        windows=_process_model.is_windows_process_model(),
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
        windows_process_model=_process_model.is_windows_process_model(),
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
    logical_cwd: str | Path | None = None,
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
    cancellation_requested: Callable[[], bool] | None = None,
    closure_progress: Callable[[str], None] | None = None,
    no_output_timeout: float | None = None,
) -> GuardResult:
    """Run ``command`` under the guard's memory, deadline and closure custody.

    ``no_output_timeout`` ends a command whose captured stdout and stderr
    stop growing for that long, as CI runners end a silent job (CircleCI's
    no_output_timeout). It bounds a command by its progress, not by its
    total wall time, and it counts only time the guard watched: a host
    freeze that stops guard and child alike is not silence. The result
    reports it as a timeout, with ``output_stall_s`` set.
    """
    if not command:
        raise ValueError("command is required")

    def progressed(step: str) -> None:
        if closure_progress is not None:
            closure_progress(step)

    if sampler is None:
        sampler = sample_processes
    if poll_interval <= 0:
        raise ValueError("poll interval must be greater than 0")
    if timeout is not None and timeout <= 0:
        raise ValueError("timeout must be greater than 0")
    if no_output_timeout is not None and (no_output_timeout <= 0 or not capture_output):
        raise ValueError("no_output_timeout must be positive and capture output")
    if (stdout_capture_path is None) != (stderr_capture_path is None):
        raise ValueError("stdout/stderr capture paths must be provided together")
    if stdout_capture_path is not None and not capture_output:
        raise ValueError("capture paths require capture_output")
    if capture_tail_bytes is not None and capture_tail_bytes <= 0:
        raise ValueError("capture_tail_bytes must be positive")
    if capture_tail_bytes is not None and stdout_capture_path is None:
        raise ValueError("capture_tail_bytes requires external capture paths")
    if (
        stdout_capture_path is not None
        and stderr_capture_path is not None
        and Path(stdout_capture_path).resolve() == Path(stderr_capture_path).resolve()
    ):
        raise ValueError("stdout/stderr capture paths must be distinct")
    if keepalive_interval is not None and keepalive_interval <= 0:
        keepalive_interval = None
    if text and isinstance(input, bytes):
        raise TypeError("bytes input requires text=False")
    if not text and isinstance(input, str):
        raise TypeError("str input requires text=True")
    child_env = ProcessEnvironmentBuilder(os.environ if env is None else env)
    child_env[ACTIVE_ENV] = "1"
    child_env[ACTIVE_GUARD_PID_ENV] = str(os.getpid())
    guard_token, guard_marker = _write_active_guard_marker(
        os.getpid(),
        command=command,
        cwd=cwd,
        environ=child_env,
        outputs=tuple(
            Path(path).resolve(strict=False)
            for path in (
                stdout_capture_path,
                stderr_capture_path,
                samples_jsonl,
                running_summary_json,
            )
            if path is not None
        ),
    )
    child_env[ACTIVE_GUARD_TOKEN_ENV] = guard_token
    child_env[ACTIVE_GUARD_MARKER_ENV] = str(guard_marker)
    # Peaks per pytest worker subtree and per test size the next run's workers.
    worker_peaks = WorkerPeakTracker.for_child(
        child_env, current_test_file_env=_PYTEST_CURRENT_TEST_FILE_ENV, root=ROOT
    )
    _inject_guard_memory_contract_env(
        child_env,
        max_rss_kb=max_rss_kb,
        child_rlimit_kb=child_rlimit_kb,
        max_total_rss_kb=max_total_rss_kb,
    )
    start = time.perf_counter()
    observer_cpu_start = time.process_time()
    baseline_pgids: frozenset[int] = frozenset()
    guard_signal: int | None = None
    cancelled = False

    def _handle_guard_signal(signum: int, _frame: object) -> None:
        nonlocal guard_signal
        if guard_signal is None:
            guard_signal = signum

    installed_signal_handlers: dict[
        int, Callable[[int, FrameType | None], Any] | int | None
    ] = {}
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
    enforcer: OwnedTreeEnforcer | None = None
    cancellation_watch: StopRequestWatch | None = None
    signal_watch: StopRequestWatch | None = None
    summary_publisher: _reporting.RunningSummaryPublisher | None = None
    termination_reports: list[GuardTerminationReport] = []
    scratch_lease: _temporary_artifacts.GuardScratchLease | None = None
    temporary_artifacts: dict[str, object] | None = None
    scratch_finalized = False
    scratch_cleanup_error: str | None = None
    caught_exception: BaseException | None = None
    stdout_capture: Any = None
    stderr_capture: Any = None
    guard_interrupted = False

    def finish_temporary_artifacts(
        *,
        closed: bool,
        success: bool,
        evidence: Mapping[str, object],
    ) -> str | None:
        nonlocal scratch_finalized, scratch_cleanup_error, temporary_artifacts
        if scratch_lease is None or scratch_finalized:
            return scratch_cleanup_error
        scratch_finalized = True
        timings = _temporary_artifacts.ScratchFinalizationTimings()
        finalize_started = time.perf_counter()
        try:
            outcome = _temporary_artifacts.finish_guard_scratch(
                scratch_lease,
                closed=closed,
                success=success,
                evidence=evidence,
                timings=timings,
            )
            temporary_artifacts = {
                **dict(outcome),
                "closure": dict(evidence),
            }
        except BaseException as exc:
            scratch_cleanup_error = f"{type(exc).__name__}: {exc}"
            temporary_artifacts = {
                "state": "cleanup-error",
                "error": scratch_cleanup_error,
                "closure": dict(evidence),
            }
            generation = getattr(scratch_lease, "generation", None)
            if generation is not None:
                generation_path = Path(generation)
                finish_error_receipt = generation_path / "finish-error.json"
                temporary_artifacts["receipt"] = str(
                    finish_error_receipt
                    if finish_error_receipt.exists()
                    else generation_path / "owner.json"
                )
        finally:
            # finish_guard_scratch owns the normal release. Keep this idempotent
            # fallback for every partial/exceptional terminalization path.
            try:
                with timings.measure(
                    _temporary_artifacts.ScratchFinalizationPhase.LEASE_RELEASE
                ):
                    scratch_lease.release()
            except BaseException as exc:
                release_error = f"{type(exc).__name__}: {exc}"
                scratch_cleanup_error = (
                    release_error
                    if scratch_cleanup_error is None
                    else f"{scratch_cleanup_error}; lease release failed: {release_error}"
                )
                temporary_artifacts = {
                    **({} if temporary_artifacts is None else temporary_artifacts),
                    "state": "cleanup-error",
                    "error": scratch_cleanup_error,
                    "closure": dict(evidence),
                }
        assert temporary_artifacts is not None
        temporary_artifacts["finalize_phase_elapsed_s"] = timings.json_payload()
        cast(dict[str, object], temporary_artifacts)["finalize_elapsed_s"] = max(
            0.0, time.perf_counter() - finalize_started
        )
        return scratch_cleanup_error

    from molt.checkout_state import checkout_state_command_roots, checkout_state_use

    # A pinned startup worker restores its caller's cwd after its redirector
    # starts elsewhere. Retain the original selections through the same full
    # descendant lifetime; physical launch location must not rebase them.
    state_use = contextlib.ExitStack()
    state_closed = False
    try:
        command = _resolve_relative_executable(command)
        state_use.enter_context(
            checkout_state_use(
                checkout_state_command_roots(
                    ROOT,
                    child_env,
                    command,
                    cwd=cwd if logical_cwd is None else logical_cwd,
                )
            )
        )
        # Register before child birth: an unknown terminal result or raised
        # cleanup failure may outlive this frame and every result reference.
        atexit.register(state_use.close)
        # Selection is pure: RunContext may publish a new scratch root without
        # creating it. Native tools use these paths directly, unlike Python's
        # tempfile fallback. Materialize the actual selections before birth;
        # they remain caller-owned, outside this guard's scratch reclamation.
        # A pinned startup worker restores logical_cwd before running tools,
        # so relative selections follow that consumer directory, as above.
        working: Path | None = None
        temporary_roots: dict[Path, str] = {}
        for name in ("TMPDIR", "TMP", "TEMP"):
            raw = child_env.get(name)
            if raw:
                path = Path(raw)
                if not path.is_absolute():
                    if working is None:
                        selected_cwd = cwd if logical_cwd is None else logical_cwd
                        working = (
                            Path.cwd() if selected_cwd is None else Path(selected_cwd)
                        )
                        if not working.is_absolute():
                            working = Path.cwd() / working
                    path = working / path
                temporary_roots.setdefault(path, name)
        for path, name in temporary_roots.items():
            try:
                if not path.is_dir():
                    path.mkdir(parents=True, exist_ok=True)
            except OSError as error:
                error.add_note(f"preparing selected child {name} directory: {path}")
                raise
        scratch_lease = _temporary_artifacts.acquire_guard_scratch(ROOT, child_env)
        inherited_scratch = child_env.get(_temporary_artifacts.SCRATCH_ENV)
        if (
            inherited_scratch
            and child_env.get("PYTEST_DEBUG_TEMPROOT") == inherited_scratch
        ):
            # Rebind only the inherited guard default, never a caller's root.
            child_env["PYTEST_DEBUG_TEMPROOT"] = str(scratch_lease.target)
        child_env[_temporary_artifacts.SCRATCH_ENV] = str(scratch_lease.target)
        launch = _guarded_launch(
            command,
            ProcessEnvironment(child_env),
            child_rlimit_kb=child_rlimit_kb,
        )
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="launch_prepared",
            launch_command=list(launch.command),
            temporary_artifacts={
                "state": "leased",
                "target": str(scratch_lease.target),
            },
        )
        if capture_output:
            if stdout_capture_path is not None:
                assert stderr_capture_path is not None
                stdout_path = Path(stdout_capture_path)
                stderr_path = Path(stderr_capture_path)
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
                # Capture belongs to this guard's admitted scratch lease.
                # The caller's cached tempfile directory may be the checkout
                # the child removes; Windows keeps these names until close.
                stdout_capture = tempfile.TemporaryFile(
                    mode="w+t",
                    encoding=encoding,
                    errors=errors,
                    dir=scratch_lease.target,
                )
                stderr_capture = tempfile.TemporaryFile(
                    mode="w+t",
                    encoding=encoding,
                    errors=errors,
                    dir=scratch_lease.target,
                )
            else:
                stdout_capture = tempfile.TemporaryFile(
                    mode="w+b", dir=scratch_lease.target
                )
                stderr_capture = tempfile.TemporaryFile(
                    mode="w+b", dir=scratch_lease.target
                )
        popen_kwargs: ProcessSpawnKwargs = {
            "cwd": cwd,
            "env": (
                ProcessEnvironment(launch.env).child_block()
                if launch.env is not None
                else None
            ),
            "text": text,
            **_guarded_popen_process_isolation_kwargs(),
        }
        if text:
            # The stdin pipe is the only text stream Popen opens here; it must
            # use the same codec as the captured output, never the Windows
            # locale code page (cp1252 corrupted non-ASCII generator input).
            popen_kwargs["encoding"] = encoding
            popen_kwargs["errors"] = errors
        if capture_output:
            popen_kwargs["stdout"] = stdout_capture.fileno()
            popen_kwargs["stderr"] = stderr_capture.fileno()
        else:
            popen_kwargs = {**popen_kwargs, **inherit_stdio_kwargs()}
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
        # Persist the launch boundary before a child can exist. If publication
        # fails, do not leave an apparently pre-launch record over a live child.
        if not _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="spawn_pending",
            child_launch_state="pending",
        ):
            raise RuntimeError("cannot publish active guard child launch boundary")
        child_launch_started = time.perf_counter()
        try:
            proc = subprocess.Popen(
                native_popen_command(launch.command), **popen_kwargs
            )
        except Exception as exc:
            if guard_job is not None:
                _win_job.close_job(guard_job)
            guard_job = None
            # Popen raised, so no child exists: CPython reaps a child whose
            # exec failed before it raises. Record that launch outcome.
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="spawn_failed",
                child_launch_state="failed",
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
        _close_fds(launch.close_fds)
        # Freeze the owned launch identity from the launch contract, never a
        # kernel query. A POSIX child started with start_new_session is a
        # session leader once Popen returns (CPython raises if setsid fails),
        # and a leader can change neither its group nor its session, so both
        # equal its PID for its whole life. A fast child can already have
        # exited here, and macOS answers getpgid/getsid for an unreaped zombie
        # with ESRCH; closure would then lose its root group and report a
        # successful child as an infrastructure failure.
        launched_session_leader = popen_kwargs.get("start_new_session") is True
        child_process = GuardedChildProcess(
            pid=proc.pid,
            pgid=proc.pid if launched_session_leader else None,
            sid=proc.pid if launched_session_leader else None,
            command=tuple(launch.command),
            started_at=_utc_timestamp(),
        )
        tracker = ProcessTreeTracker(proc.pid)
        child_clock = None
        if type(proc).__module__ == "subprocess":
            from tools.memory_guard_core.process_custody import ChildExecutionClock

            # The census before the reap is exact only while the root leads
            # its own group; tracked-orphan cleanup is its sole consumer.
            root_pgid = child_process.pgid
            child_clock = ChildExecutionClock(
                proc,
                child_launch_started,
                exit_census=(
                    partial(reserved_group_exit_census, root_pgid, sampler)
                    if cleanup_orphans and root_pgid is not None
                    else None
                ),
            )
        if guard_job is not None:
            # Start execution at the actual resume syscall, after suspended
            # assignment and thread discovery. Kernel custody remains intact.
            resumed_at = _win_job.assign_and_resume(guard_job, proc)
            if child_clock is not None:
                if resumed_at is None:
                    raise RuntimeError("Windows child resume clock is unavailable")
                child_clock.started = resumed_at
        # Enforcement that observes nothing starts before any observation: a
        # stalled sample, marker write or host-wide repro read below can no
        # longer delay the deadline or a cancellation (HF-417).
        enforcer = OwnedTreeEnforcer(
            owned_root_group_signal(
                proc,
                child_clock,
                child_process,
                terminate_job=(
                    None
                    if guard_job is None
                    else partial(_win_job.terminate_job, guard_job)
                ),
            ),
            slack=ENFORCEMENT_SLACK_SEC,
            deadline=None if timeout is None else start + timeout,
            deadline_grace=0.25,
            # The deadline's own clock, which tests may replace.
            clock=time.perf_counter,
        )
        if cancellation_requested is not None:
            cancellation_watch = StopRequestWatch(
                cancellation_requested,
                interval=poll_interval,
                on_request=partial(enforcer.request, "owner_cancellation", grace=0.0),
                name="molt-guard-cancellation",
            )
        if installed_signal_handlers:
            # The handler only records the signal; the loop signals the tree
            # after its export, which may wait for the adoption lock. The
            # enforcer bounds that wait for the root group.
            signal_watch = StopRequestWatch(
                lambda: guard_signal is not None,
                interval=poll_interval,
                on_request=partial(
                    enforcer.request, "guard_signal", grace=OPERATOR_SIGNAL_GRACE_S
                ),
                name="molt-guard-signal",
            )
        sampling_source = (
            "windows_full_process_table"
            if _process_model.is_windows_process_model()
            else _process_model.posix_custody_source()
        )
        if sampler is not sample_processes:
            sampling_source = "custom"
        # Custody rows stop at the tree, while termination protection reads the
        # guard's ancestry and host control-plane processes: when polls read
        # partial rows, a termination decides on the host table.
        custody_rows_partial = (
            sampler is sample_processes
            and not _process_model.is_windows_process_model()
            and _process_model.posix_custody_reads_partial_rows()
        )
        if guard_job is not None:
            cleanup_orphans = False
        if guard_job is not None:
            # A Windows Job already provides race-free ownership for the entire
            # descendant tree. Query only those kernel-owned members for every
            # guarded command; full host snapshots are fallback/diagnostic
            # authority, never the steady-state sampler.
            job_command = " ".join(command)
            job_member_commands: dict[tuple[int, int | None], str] = {}
            job_member_contexts: dict[tuple[int, int | None], tuple[int, str]] = {}
            previous_job_members: frozenset[tuple[int, int | None]] = frozenset()
            latest_job_process_generation: int | None = None

            def _sample_owned_job() -> Mapping[int, ProcessSample]:
                nonlocal job_member_commands, job_member_contexts, previous_job_members
                nonlocal latest_job_process_generation
                # Lifetime accounting fences births, including instances that
                # start and exit between process snapshots. Capture it before
                # inventory reads and compare after atomic Job termination.
                latest_job_process_generation = _win_job.job_accounting(
                    guard_job
                ).total_processes
                members = _win_job.process_memory(guard_job)
                member_ids = frozenset(
                    (member.pid, member.started_at_ns) for member in members
                )
                if member_ids != previous_job_members:
                    members = _win_job.process_memory(
                        guard_job,
                        include_image_names=True,
                    )
                    previous_job_members = frozenset(
                        (member.pid, member.started_at_ns) for member in members
                    )
                    live_keys = {
                        (member.pid, member.started_at_ns) for member in members
                    }
                    job_member_commands = {
                        key: value
                        for key, value in job_member_commands.items()
                        if key in live_keys
                    }
                    job_member_contexts = {
                        key: value
                        for key, value in job_member_contexts.items()
                        if key in live_keys
                    }
                    from molt.windows_process_snapshot import (
                        windows_job_command_context,
                    )

                    for member in members:
                        image_role = Path(member.image_name or "").stem.casefold()
                        if (
                            image_role == "cargo"
                            or image_role in CARGO_COMPILER_EXECUTABLES
                        ) and member.started_at_ns is not None:
                            context = windows_job_command_context(
                                member.pid, member.started_at_ns
                            )
                            if context is not None:
                                job_member_contexts[
                                    (member.pid, member.started_at_ns)
                                ] = context
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
                    context = job_member_contexts.get(
                        (member.pid, member.started_at_ns)
                    )
                    native_argv: tuple[str, ...] = ()
                    real_parent = os.getpid() if member.pid == proc.pid else 0
                    if context is not None:
                        from molt.backend_daemon_custody import _split_command

                        real_parent, command_text = context
                        native_argv = tuple(_split_command(command_text))
                    samples[member.pid] = ProcessSample(
                        pid=member.pid,
                        ppid=real_parent,
                        rss_kb=(member.rss_bytes + 1023) // 1024,
                        command=command_text,
                        pgid=child_process.pgid,
                        started_at_ns=member.started_at_ns,
                        argv=native_argv,
                        command_kind=(
                            "full"
                            if context is not None
                            else "image"
                            if (member.pid, member.started_at_ns) in job_member_commands
                            else "unavailable"
                        ),
                    )
                    # Job membership grants custody independently of parentage.
                    # Never invent Cargo ancestry to prime the resource tracker.
                    if type(member.started_at_ns) is int and member.started_at_ns > 0:
                        assert tracker.known_pids is not None
                        assert tracker.known_identities is not None
                        tracker.known_pids.add(member.pid)
                        tracker.known_identities[member.pid] = ProcessIdentity(
                            member.started_at_ns
                        )
                return samples

            sampler = _sample_owned_job
            sampling_source = "windows_job_members"
        root_started_at_ns = (
            windows_process_handle_started_at_ns(getattr(proc, "_handle", None))
            if _process_model.is_windows_process_model()
            else _process_model.process_started_at_ns(proc.pid)
        )
        if root_started_at_ns is not None:
            assert tracker.known_identities is not None
            tracker.known_identities[proc.pid] = ProcessIdentity(root_started_at_ns)
        child_process = replace(child_process, started_at_ns=root_started_at_ns)
        if running_summary_json is not None:
            # The startup report publishes here; the summary's host-wide repro
            # block is read and written off this thread (HF-417).
            summary_publisher = _reporting.RunningSummaryPublisher(
                running_summary_json,
                child_process=child_process,
                repro_context_provider=repro_context_payload,
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
            )
            try:
                summary_publisher.start()
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
            child_launch_state="recorded",
            child_process=guarded_child_process_payload(child_process),
        )

        def terminate_owned_tree(
            *,
            reason: str,
            samples: Mapping[int, ProcessSample] | None = None,
            watched: set[int] | None = None,
            grace: float,
            decide_on_host_table: bool = True,
        ) -> None:
            nonlocal interruption_inventory
            if (
                interruption_inventory is None
                and samples is not None
                and watched is not None
            ):
                interrupted_cargo_observations.update(latest_cargo_observations)
                if reason == "timeout":
                    interruption_inventory = observe_cargo_interruption_inventory(
                        samples,
                        watched,
                        tracker.custody_identities(watched),
                        latest_cargo_observations,
                    )
                else:
                    # Resource pressure and control-plane failure already stop
                    # globally. Never delay their kill for negative cache facts.
                    interruption_inventory = CargoInterruptionInventory(
                        f"interruption inventory unavailable after {reason}"
                    )
            if guard_job is not None:
                # The Job is the exact Windows ownership boundary established
                # before the child was resumed.  Do not duplicate that authority
                # with PID-table termination, whose identities can race process
                # exit and wrapper descendants.
                if enforcer is not None:
                    # Job termination observes nothing; it is idempotent.
                    enforcer.begin(reason)
                _win_job.terminate_job(guard_job)
                if enforcer is not None:
                    enforcer.delivered(reason)
                _win_job.wait_until_empty(guard_job, timeout=termination_wait_s)
                after = _win_job.job_accounting(guard_job)
                if interruption_inventory is not None:
                    interruption_inventory = (
                        interruption_inventory.fence_process_births(
                            latest_job_process_generation,
                            after.total_processes,
                            closed=after.active_processes == 0,
                        )
                    )
                return
            if interruption_inventory is not None and interruption_inventory.complete:
                interruption_inventory = CargoInterruptionInventory(
                    "native interruption inventory lacks a process-birth fence"
                )
            # The protection decision reads the host table when polls read
            # partial rows. That read can stall, so it comes before the check
            # that defers to the enforcer, which may have signalled meanwhile.
            decision_samples = (
                sampler() if decide_on_host_table and custody_rows_partial else samples
            )
            if enforcer is not None and not enforcer.begin(reason):
                # The enforcer signalled the root group while this loop was
                # stalled. Its strike ends in the reap; the closure after the
                # loop observes what remains.
                with contextlib.suppress(subprocess.TimeoutExpired):
                    proc.wait(timeout=termination_wait_s)
                return
            termination_reports.append(
                _validated_termination_report(
                    terminate_watched_processes(
                        proc.pid,
                        samples=decision_samples,
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
            if enforcer is not None:
                enforcer.delivered(reason)

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
        output_stalled = False
        child_exit_usage: ChildExitResourceUsage | None = None
        last_limits: ResolvedMemoryLimits | None = None
        termination_wait_expired = False
        termination_wait_s = termination_wait_seconds(env)
        remembered_samples: Mapping[int, ProcessSample] | None = None
        remembered_watched: set[int] | None = None
        # Rows outside the tree that each poll still needs: a suite lease's
        # owner and guardian, and the guard's own lineage, which protection
        # walks for every tree member. Both are fixed for this run.
        declared_pids = (
            _suite_lease_process_ids(child_env) if cleanup_orphans else frozenset()
        ) | (
            _process_model.posix_custody_lineage(os.getpid())
            if custody_rows_partial
            else frozenset()
        )
        saw_cargo_build_state = _command_invokes_cargo_build_state(command)
        cargo_incremental_observations: set[CargoIncrementalObservation] = set()
        latest_cargo_observations: set[CargoIncrementalObservation] = set()
        interrupted_cargo_observations: set[CargoIncrementalObservation] = set()
        interruption_inventory: CargoInterruptionInventory | None = None
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
            observer_wall_time_s = max(0.0, time.perf_counter() - start)
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
                observer_cpu_duty_cycle=signed_ratio_value(
                    observer_cpu_time_s,
                    observer_wall_time_s,
                    direction=RatioDirection.RATIO,
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
                elapsed_s=time.perf_counter() - start,
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
            nonlocal interruption_inventory
            interruption_inventory = CargoInterruptionInventory(
                "interruption snapshot unavailable after sampling failure"
            )
            if remembered_samples is not None and remembered_watched is not None:
                # Sampling just failed: decide on the last poll's rows rather
                # than read again before the direct-child kill.
                terminate_owned_tree(
                    reason=reason,
                    samples=remembered_samples,
                    watched=remembered_watched,
                    grace=0.0,
                    decide_on_host_table=False,
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
        suite_custody_transfers = []

        def transfer_receipted_daemon_instances(
            fresh: Mapping[int, ProcessSample], *, watched: set[int], decisive: bool
        ) -> None:
            # Windows Job closure owns every assigned instance; never advertise
            # a lease export which Job completion would immediately terminate.
            if (
                not cleanup_orphans
                or _process_model.is_windows_process_model()
                or not child_env.get("MOLT_BACKEND_DAEMON_SUITE_LEASE")
            ):
                return
            # A transfer cannot release this guard's root group. Ordinary
            # commands with no live owned group outside it have nothing to
            # export, and must not contend on the shared adoption index.
            if not any(
                pid != tracker.root_pid and _sample_pgid(fresh[pid]) != tracker.root_pid
                for pid in watched
            ):
                return
            from molt import backend_daemon_suite_custody as suite_custody

            # A poll looks again on its next sample. The export before the
            # guard signals the tree, and the one after the child exits, are
            # the last looks: they wait for the adoption lock rather than
            # signal a daemon the suite adopted.
            for lease, identity, members in suite_custody.transferable_groups(
                child_env,
                project_root=ROOT,
                samples=fresh,
                acknowledge=False,
                wait_for_lock=decisive,
            ):
                expected = {
                    pid: process_identity(sample) for pid, sample in members.items()
                }
                if tracker.transfer_process_group(
                    identity.pid, samples=fresh, identities=expected
                ):
                    suite_custody_transfers.append(
                        {
                            "pgid": identity.pid,
                            "daemon_started_at_ns": identity.started_at_ns,
                            "config_digest": identity.config_digest,
                            "backend_sha256": identity.backend_sha256,
                            "suite_lease": identity.suite_lease,
                            "owner_pid": lease["owner_pid"],
                            "owner_started_at_ns": lease["owner_started_at_ns"],
                            "guardian_pid": lease["guardian_pid"],
                            "guardian_started_at_ns": lease["guardian_started_at_ns"],
                            "members": {
                                str(pid): value.started_at_ns
                                for pid, value in expected.items()
                            },
                        }
                    )

        def sample_tracked_tree(
            *,
            allow_transient_timeout: bool = False,
            before_signal: bool = False,
        ) -> tuple[Mapping[int, ProcessSample], set[int]] | None:
            nonlocal guard_interrupted, last_sample_cost_s, latest_cargo_observations
            latest_cargo_observations = (
                set()
            )  # Failed/remembered snapshots confer no interruption scope.
            nonlocal remembered_samples, remembered_watched
            nonlocal sampling_attempts, sampling_successes
            nonlocal sampling_wall_time_s, sampling_cpu_time_s
            nonlocal max_sampling_wall_time_s, max_sampling_cpu_time_s
            nonlocal sampling_process_rows, max_sampling_process_rows
            sample_started_ns = time.perf_counter_ns()
            sample_cpu_started_ns = time.process_time_ns()
            sampling_attempts += 1

            def record_sampling_cost(process_rows: int = 0) -> None:
                nonlocal last_sample_cost_s
                nonlocal sampling_wall_time_s, sampling_cpu_time_s
                nonlocal max_sampling_wall_time_s, max_sampling_cpu_time_s
                nonlocal sampling_process_rows, max_sampling_process_rows
                wall_cost = (time.perf_counter_ns() - sample_started_ns) / 1_000_000_000
                cpu_cost = (
                    time.process_time_ns() - sample_cpu_started_ns
                ) / 1_000_000_000
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
                if sampler is sample_processes:
                    # One poll needs the tree and the declared rows, not the
                    # host table (docs/design/memory_guard_custody_sampling.md).
                    samples = sample_processes_custody(
                        _process_model.CustodyInterest(
                            tree_pids=frozenset(
                                {tracker.root_pid, *(remembered_watched or ())}
                            ),
                            exact_pids=declared_pids,
                        )
                    )
                else:
                    samples = sampler()
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
            sampling_successes += 1
            watched = tracker.update(samples)
            latest_cargo_observations = observe_owned_incremental_state(
                samples, watched, tracker.custody_identities(watched)
            )
            cargo_incremental_observations.update(latest_cargo_observations)
            record_sampling_cost(len(samples))
            # A snapshot that precedes a signal is the tree's last export.
            transfer_receipted_daemon_instances(
                samples, watched=watched, decisive=before_signal
            )
            watched = tracker.update(samples)
            remembered_samples = samples
            remembered_watched = set(watched)
            return samples, watched

        def child_exit_observed() -> bool:
            # The owned child handle is the sole exit authority on every
            # platform. ChildExecutionClock reserves and reaps a real child and
            # publishes its rusage; a raw PID wait would race that reaper.
            return proc.poll() is not None

        def await_child_exit(wait_s: float) -> bool:
            """Wait up to ``wait_s`` on the owned handle; True once it exited."""
            try:
                proc.wait(timeout=wait_s)
            except subprocess.TimeoutExpired:
                # An exit can publish while the bounded wait reports expiry.
                return child_exit_observed()
            return True

        output_size_seen = -1
        output_silence_s = 0.0

        def command_output_stalled(watched_s: float) -> bool:
            """Whether the captured output stopped growing for the bound.

            Silence accrues only for ``watched_s``, the part of the poll the
            guard spent waiting as it planned. Time beyond that plan (a host
            freeze that stops guard and child alike, a stalled sample) is
            not the command's silence: a frozen host must not end a command
            that resumes its output with the host.
            """
            nonlocal output_size_seen, output_silence_s
            if no_output_timeout is None:
                return False
            size = sum(
                os.fstat(capture.fileno()).st_size
                for capture in (stdout_capture, stderr_capture)
            )
            if size != output_size_seen:
                output_size_seen, output_silence_s = size, 0.0
                return False
            output_silence_s += watched_s
            return output_silence_s >= no_output_timeout

        def next_poll_wait_s(*, fast_start: bool) -> float:
            elapsed = time.perf_counter() - start
            wait_s = paced_poll_interval(poll_interval, last_sample_cost_s)
            if fast_start and elapsed < DEFAULT_FAST_START_DURATION_SEC:
                wait_s = min(wait_s, DEFAULT_FAST_START_POLL_INTERVAL_SEC)
            if timeout is not None:
                wait_s = max(0.0, min(wait_s, timeout - elapsed))
            return wait_s

        def enforce_snapshot(
            samples: Mapping[int, ProcessSample], watched: set[int]
        ) -> bool:
            """Account, record and enforce one tree observation; True on a trip."""
            nonlocal saw_cargo_build_state, peak, peak_total, last_limits
            nonlocal violation, limit_at_violation
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
            if worker_peaks is not None:
                worker_peaks.observe(samples, watched)
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
            if violation is None:
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
                return False
            limit_at_violation = current_limits
            # Evidence writes and the kill's host-table read follow; the
            # enforcer signals the root group if they stall past the slack.
            if enforcer is not None:
                enforcer.request("rss_limit", grace=0.25)
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
                elapsed_s=time.perf_counter() - start,
            )
            # The poll's export gave up the adoption lock after its bound; the
            # export before a signal waits for it, so the kill spares a daemon
            # the suite adopted, as the timeout and cancellation kills do.
            transfer_receipted_daemon_instances(samples, watched=watched, decisive=True)
            watched = tracker.update(samples)
            terminate_owned_tree(
                reason="rss_limit",
                samples=samples,
                watched=watched,
                grace=0.25,
            )
            return True

        # One observation of the launched tree is both the orphan-cleanup
        # baseline and the first enforced, recorded sample. Each later sample
        # follows a bounded wait on the owned child handle, which returns as
        # soon as the child exits.
        snapshot = sample_tracked_tree(allow_transient_timeout=True)
        baseline_authoritative = not cleanup_orphans
        if cleanup_orphans and snapshot is not None and not guard_interrupted:
            baseline_pgids = _live_process_group_ids(snapshot[0])
            baseline_authoritative = True
        child_exited = False
        previous_poll_at = start
        while not guard_interrupted:
            if snapshot is not None and enforce_snapshot(*snapshot):
                break
            planned_wait_s = next_poll_wait_s(fast_start=snapshot is not None)
            if await_child_exit(planned_wait_s):
                child_exited = True
                break
            now = time.perf_counter()
            watched_s = min(max(0.0, now - previous_poll_at), planned_wait_s)
            previous_poll_at = now
            cancelled = (
                cancellation_watch is not None and cancellation_watch.requested()
            )
            if cancelled:
                progressed("cancellation_observed")
            if guard_signal is not None or cancelled:
                signal_snapshot = sample_tracked_tree(before_signal=True)
                assert signal_snapshot is not None
                samples, watched = signal_snapshot
                if guard_interrupted:
                    break
                _update_active_guard_marker(
                    guard_marker,
                    guard_token,
                    status="cancellation_terminating"
                    if cancelled
                    else "guard_signal_terminating",
                    cancelled=cancelled,
                    child_process=guarded_child_process_payload(child_process),
                    guard_signal=guard_signal,
                    elapsed_s=now - start,
                )
                saw_cargo_build_state = (
                    saw_cargo_build_state
                    or _samples_include_cargo_build_state(samples, watched)
                )
                terminate_owned_tree(
                    reason="owner_cancellation" if cancelled else "guard_signal",
                    samples=samples,
                    watched=watched,
                    grace=0.0 if cancelled else OPERATOR_SIGNAL_GRACE_S,
                )
                progressed("tree_signalled")
                try:
                    proc.wait(timeout=termination_wait_s)
                except subprocess.TimeoutExpired:
                    termination_wait_expired = True
                progressed("child_wait_finished")
                break
            stalled = command_output_stalled(watched_s)
            if stalled or (timeout is not None and now - start >= timeout):
                timed_out = True
                output_stalled = stalled
                stop_reason = "output_stalled" if stalled else "timeout"
                if stalled and enforcer is not None:
                    # The export below may wait for the adoption lock.
                    enforcer.request(stop_reason, grace=0.25)
                timeout_snapshot = sample_tracked_tree(before_signal=True)
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
                    reason=stop_reason,
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
        # The run loop has delivered or the child has exited. The enforcer
        # stays armed until no signal of the root remains to deliver.
        if cancellation_watch is not None:
            cancellation_watch.stop()
        if signal_watch is not None:
            signal_watch.stop()
        assert enforcer is not None
        if child_exited and violation is None:
            child_exit_usage = _take_child_exit_usage(proc)
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
        post_exit_observation: TerminalProcessObservation | None = None
        root_exit_census: dict[str, object] = {}
        try:
            if proc.returncode is None and not guard_interrupted:
                try:
                    proc.wait(timeout=max(1.0, poll_interval * 4.0))
                except subprocess.TimeoutExpired:
                    # The export below may wait for the adoption lock; the
                    # enforcer signals the root group if it waits past the
                    # slack.
                    enforcer.request("post_loop_unreaped_child", grace=0.0)
                    post_loop_snapshot = sample_tracked_tree(before_signal=True)
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
            # Every signal of the root is delivered: the rest is observation,
            # which a backstop must not race.
            enforced_reasons, enforcer_reports = enforcer.stop()
            if "timeout" in enforced_reasons:
                timed_out = True
            if "owner_cancellation" in enforced_reasons:
                cancelled = True
            exit_census, exit_census_error = take_child_exit_census(proc)
            if exit_census is not None and not guard_interrupted:
                # A child orphaned between two samples is reparented at its
                # parent's exit; only the census of the root's still-reserved
                # group puts it under custody.
                root_exit_census["admitted_pids"] = sorted(
                    tracker.admit_reserved_group_members(exit_census)
                )
            if exit_census_error is not None:
                root_exit_census["error"] = exit_census_error
            if (
                cleanup_orphans
                and not guard_interrupted
                and sampler is _process_model.sample_processes
                and proc.returncode is not None
                and child_process.pgid == proc.pid
                and exit_census is not None
                and not exit_census
                and exit_census_error is None
                and tracker.known_pids == {proc.pid}
            ):
                # The reserved pre-reap census found no unseen group member,
                # no other PID is tracked, and the kernel now proves absence.
                # This fact covers only the root group. Nested guard records
                # retain their separate, lazy host observation at closure.
                post_exit_observation = RootGroupAbsence.observe(proc.pid)
            if (
                cleanup_orphans
                and not guard_interrupted
                and not _process_model.is_windows_process_model()
                and child_env.get("MOLT_BACKEND_DAEMON_SUITE_LEASE")
                and not isinstance(post_exit_observation, RootGroupAbsence)
            ):
                post_exit_observation = sampler()
                transfer_receipted_daemon_instances(
                    post_exit_observation,
                    watched=tracker.update(post_exit_observation),
                    decisive=True,
                )
            if (
                cleanup_orphans
                and not guard_interrupted
                and not isinstance(post_exit_observation, RootGroupAbsence)
            ):
                try:
                    # One observation after the root's exit serves every
                    # cleanup decision, and the closure proof, until the
                    # guard signals a process. A signal retires it.
                    if post_exit_observation is None:
                        post_exit_observation = sampler()
                    tracked_orphans = cleanup_tracked_orphans(
                        proc.pid,
                        tracker=tracker,
                        sampler=sampler,
                        samples=post_exit_observation,
                        grace=0.25,
                        root_reaped=proc.returncode is not None,
                    )
                    if tracked_orphans.termination_reports:
                        post_exit_observation = None
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
                            samples=post_exit_observation,
                            grace=0.25,
                        )
                        if repo_orphans.termination_reports:
                            post_exit_observation = None
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
                    post_exit_observation = None
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
                        tail_handle.seek(
                            max(0, tail_handle.tell() - capture_tail_bytes)
                        )
                        tail_data = tail_handle.read()
                    stdout = (
                        tail_data.decode(encoding, errors=errors) if text else tail_data
                    )
                else:
                    stdout_capture.seek(0)
                    stdout = stdout_capture.read()
            if stderr_capture is not None:
                stderr_capture.flush()
                if capture_tail_bytes is not None and stderr_capture_path is not None:
                    with Path(stderr_capture_path).open("rb") as tail_handle:
                        tail_handle.seek(0, os.SEEK_END)
                        tail_handle.seek(
                            max(0, tail_handle.tell() - capture_tail_bytes)
                        )
                        tail_data = tail_handle.read()
                    stderr = (
                        tail_data.decode(encoding, errors=errors) if text else tail_data
                    )
                else:
                    stderr_capture.seek(0)
                    stderr = stderr_capture.read()
        finally:
            if stdout_capture is not None:
                stdout_capture.close()
            if stderr_capture is not None:
                stderr_capture.close()
        finished = time.perf_counter()
        child_started = _read_child_started_at(launch.started_read_fd)
        elapsed_start = start
        elapsed_s = max(0.0, finished - elapsed_start)
        if child_clock is not None:
            precise_start = (
                child_started if child_started is not None else child_clock.started
            )
            child_elapsed_s = (
                None
                if child_clock.finished is None or precise_start is None
                else max(0.0, child_clock.finished - precise_start)
            )
        else:
            child_elapsed_s = None
        # Preserve captured guest bytes before adding any guard diagnostics.
        child_stderr = stderr if capture_output else None
        returncode = proc.returncode
        if violation is not None:
            returncode = GUARD_RETURN_CODE
        if timed_out:
            returncode = TIMEOUT_RETURN_CODE
            timeout_msg = (
                f"memory_guard: no output for {no_output_timeout:.2f}s\n"
                if output_stalled
                else f"memory_guard: timeout after {timeout:.2f}s\n"
            )
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
        if cancelled:
            returncode = GUARD_RETURN_CODE
            stderr = _append_guard_message(
                stderr,
                "memory_guard: owner requested cancellation; see terminal child custody\n",
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
        for enforcement in enforcer_reports:
            termination_reports.append(
                completed_enforcement_report(
                    enforcement,
                    identities=tracker.custody_identities(
                        set(tracker.known_pids or ())
                    ),
                    sampler=sampler,
                    grace=termination_wait_s,
                )
            )
        progressed("descendant_closure_started")
        descendants_closed, scratch_closure_evidence = (
            _temporary_artifact_descendant_closure(
                proc=proc,
                child_process=child_process,
                tracker=tracker,
                sampler=sampler,
                windows_job_cleanup=windows_job_cleanup,
                windows_process_model=_process_model.is_windows_process_model(),
                posix_process_model=os.name == "posix",
                cleanup_orphans=cleanup_orphans,
                guard_interrupted=guard_interrupted,
                termination_wait_expired=termination_wait_expired,
                sampling_telemetry=final_sampling_telemetry,
                termination_reports=termination_reports,
                probe_grace=termination_wait_s,
                active_dir=guard_marker.parent,
                final_observation=post_exit_observation,
            )
        )
        if root_exit_census:
            scratch_closure_evidence["root_exit_census"] = root_exit_census
        if suite_custody_transfers:
            scratch_closure_evidence["suite_custody_transfers"] = (
                suite_custody_transfers
            )
            # A lost receiver cannot authorize scratch retirement. Its EOF
            # guardian owns the bounded drain; preserve evidence until closure.
            from molt import backend_daemon_suite_custody as suite_custody

            fresh = sampler()
            for transfer in suite_custody_transfers:
                receiver_path = transfer["suite_lease"]
                live_members = [
                    sample
                    for sample in fresh.values()
                    if sample.pgid == transfer["pgid"]
                ]
                if live_members and (
                    not isinstance(receiver_path, str)
                    or suite_custody.live_lease(
                        Path(receiver_path), project_root=ROOT, samples=fresh
                    )
                    is None
                ):
                    descendants_closed = False
                    scratch_closure_evidence["closed"] = False
                    scratch_closure_evidence["transfer_receiver_lost"] = True
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
                descendants_closed=descendants_closed,
                eligible_observations=frozenset(interrupted_cargo_observations),
                interruption_inventory_complete=(
                    interruption_inventory is not None
                    and interruption_inventory.complete
                ),
                interruption_inventory_error=(
                    interruption_inventory.error
                    if interruption_inventory is not None
                    else "interruption snapshot unavailable"
                ),
                profile_lock_settle_s=min(termination_wait_s, 1.0),
                observations=tuple(
                    sorted(
                        cargo_incremental_observations,
                        key=lambda item: (
                            item.incremental_dir,
                            item.rustc_pid,
                            item.rustc_started_at_ns,
                        ),
                    )
                ),
            )
            stderr = _append_guard_message(
                stderr,
                f"{_cargo_incremental_quarantine_message(cargo_incremental_quarantine)}\n",
                text=text,
            )
            if cargo_incremental_quarantine.errors:
                stderr = _append_guard_message(
                    stderr,
                    "memory_guard: cargo recovery "
                    + (
                        "deferral details: "
                        if cargo_incremental_quarantine.ownership_status == "deferred"
                        else "errors: "
                    )
                    + f"{'; '.join(cargo_incremental_quarantine.errors)}\n"
                    + _cargo_recovery_next_action(cargo_incremental_quarantine)
                    + "\n",
                    text=text,
                )
        peak_job_commit_bytes = (
            None
            if windows_job_cleanup is None
            else windows_job_cleanup.after.peak_job_commit_bytes
        )
        scratch_clean_success = bool(
            descendants_closed
            and final_returncode == 0
            and violation is None
            and not timed_out
            and guard_signal is None
            and not cancelled
            and not termination_wait_expired
            and not orphaned_process_groups
            and not termination_reports
            and (
                windows_job_cleanup is None
                or not windows_job_cleanup.terminated_remaining_processes
            )
        )
        cleanup_error = finish_temporary_artifacts(
            closed=descendants_closed,
            success=scratch_clean_success,
            evidence=scratch_closure_evidence,
        )
        progressed("temporary_artifacts_finished")
        # Guard elapsed time includes exact descendant closure plus scratch
        # retirement/reclamation; storage cleanup latency must remain visible.
        wall_finished = time.perf_counter()
        elapsed_s = max(0.0, wall_finished - elapsed_start)
        # The independent child interval must fit inside the guard wall budget.
        if child_elapsed_s is not None and child_elapsed_s > elapsed_s:
            child_elapsed_s = None
        scratch_state = (
            None if temporary_artifacts is None else temporary_artifacts.get("state")
        )
        retention_errors: tuple[str, ...] = ()
        if temporary_artifacts is not None:
            retention = temporary_artifacts.get("retention")
            if isinstance(retention, Mapping):
                raw_errors = retention.get("errors")
                if isinstance(raw_errors, Sequence) and not isinstance(
                    raw_errors, (str, bytes)
                ):
                    retention_errors = tuple(str(error) for error in raw_errors)
        scratch_failure_details: list[str] = []
        if cleanup_error:
            scratch_failure_details.append(cleanup_error)
        if not descendants_closed:
            scratch_failure_details.append(
                "guarded descendant closure remained indeterminate"
            )
        if scratch_state in {"blocked", "cleanup-error", "indeterminate"}:
            scratch_failure_details.append(
                f"temporary artifact terminal state is {scratch_state!r}"
            )
        if retention_errors:
            scratch_failure_details.append(
                "temporary artifact retention sweep reported errors: "
                + json.dumps(list(retention_errors), sort_keys=True)
            )
        infrastructure_failure = (
            GuardInfrastructureFailure(
                phase="temporary_artifact_custody",
                details=tuple(scratch_failure_details),
            )
            if scratch_failure_details
            else None
        )
        if infrastructure_failure is not None:
            stderr = _append_guard_message(
                stderr,
                "memory_guard: temporary artifact custody incomplete: "
                f"{'; '.join(scratch_failure_details)}; "
                "see temporary_artifacts in the guard summary\n",
                text=text,
            )
            if final_returncode == 0:
                final_returncode = INFRASTRUCTURE_RETURN_CODE
        result = GuardResult(
            returncode=final_returncode,
            violation=violation,
            peak=peak,
            peak_total=peak_total,
            stdout=stdout,
            stderr=stderr,
            child_stderr=child_stderr,
            timed_out=timed_out,
            cancelled=cancelled,
            descendants_closed=descendants_closed,
            elapsed_s=elapsed_s,
            child_elapsed_s=child_elapsed_s,
            cleanup_elapsed_s=(
                None
                if child_elapsed_s is None
                else max(0.0, elapsed_s - child_elapsed_s)
            ),
            limit_at_violation=limit_at_violation,
            orphaned_process_groups=orphaned_process_groups,
            cargo_incremental_quarantine=cargo_incremental_quarantine,
            guard_signal=guard_signal,
            child_process=child_process,
            owned_process_identities=tuple(
                sorted(tracker.custody_identities(tracker.known_pids or ()).items())
            ),
            termination_reports=tuple(termination_reports),
            sampling_telemetry=final_sampling_telemetry,
            peak_job_commit_bytes=peak_job_commit_bytes,
            windows_job_cleanup=windows_job_cleanup,
            temporary_artifacts=temporary_artifacts,
            child_returncode=proc.returncode,
            infrastructure_failure=infrastructure_failure,
            worker_peaks=None if worker_peaks is None else worker_peaks.summary(),
            output_stall_s=no_output_timeout if output_stalled else None,
        )
        state_closed = result.descendants_closed
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="completed",
            # Without proven closure, orphaned processes may still run: the
            # record then stays unresolved custody (active_custody.terminal).
            descendants_closed=result.descendants_closed,
            owned_process_identities=process_identities_payload(
                result.owned_process_identities
            ),
            returncode=result.returncode,
            child_returncode=result.child_returncode,
            infrastructure_failure=infrastructure_failure_payload(
                result.infrastructure_failure
            ),
            timed_out=result.timed_out,
            elapsed_s=result.elapsed_s,
            child_elapsed_s=result.child_elapsed_s,
            cleanup_elapsed_s=result.cleanup_elapsed_s,
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
            temporary_artifacts=result.temporary_artifacts,
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
        if summary_publisher is not None:
            # No running summary may follow the terminal one the caller writes.
            summary_publisher.retire()
        return result
    except BaseException as exc:
        caught_exception = exc
        _update_active_guard_marker(
            guard_marker,
            guard_token,
            status="guard_exception",
            exception_type=type(exc).__name__,
            exception=str(exc),
            child_process=guarded_child_process_payload(child_process),
            child_returncode=None if proc is None else proc.returncode,
            termination_reports=termination_reports_payload(tuple(termination_reports)),
            temporary_artifacts=temporary_artifacts,
        )
        raise
    finally:
        if cancellation_watch is not None:
            cancellation_watch.stop()
        if signal_watch is not None:
            signal_watch.stop()
        if enforcer is not None:
            enforcer.stop()
        if summary_publisher is not None:
            # The normal path re-raised a writer failure; here an exception
            # is already propagating.
            with contextlib.suppress(BaseException):
                summary_publisher.retire()
        # The finalizer reads the published exit only. poll() re-raises a
        # reaper failure, which would abort the remaining custody cleanup.
        if proc is not None and proc.returncode is None:
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
                if proc.returncode is None:
                    terminate_direct_child_handle(
                        reason="run_guarded_finalizer_direct_child_handle"
                    )
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status="finalizer_completed",
                # The finalizer signals the tree but proves no closure.
                descendants_closed=False,
                child_process=guarded_child_process_payload(child_process),
                child_returncode=proc.returncode,
                termination_reports=termination_reports_payload(
                    tuple(termination_reports)
                ),
            )
        if scratch_lease is not None and not scratch_finalized:
            no_child_launched = proc is None
            exceptional_evidence = {
                "schema": "molt.guard-scratch-closure.v1",
                "authority": (
                    "no-child-launched"
                    if no_child_launched
                    else "guard-exception-indeterminate"
                ),
                "closed": no_child_launched,
                "direct_child_reaped": (
                    True if proc is None else proc.returncode is not None
                ),
                "exception_type": (
                    None
                    if caught_exception is None
                    else type(caught_exception).__name__
                ),
            }
            exceptional_cleanup_error = finish_temporary_artifacts(
                closed=no_child_launched,
                success=False,
                evidence=exceptional_evidence,
            )
            if exceptional_cleanup_error and caught_exception is not None:
                caught_exception.add_note(
                    "temporary artifact terminalization failed: "
                    f"{exceptional_cleanup_error}"
                )
            _update_active_guard_marker(
                guard_marker,
                guard_token,
                status=(
                    "guard_exception"
                    if caught_exception is not None
                    else "finalizer_completed"
                ),
                descendants_closed=no_child_launched,
                child_process=guarded_child_process_payload(child_process),
                child_returncode=None if proc is None else proc.returncode,
                termination_reports=termination_reports_payload(
                    tuple(termination_reports)
                ),
                temporary_artifacts=temporary_artifacts,
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
        if caught_exception is None:
            _retire_guard_custody(guard_marker, guard_token)
        if proc is None or state_closed:
            # proc=None is the existing no-child-launched proof, including
            # validation/admission/spawn refusal. A reaped root alone cannot
            # authorize releasing a use whose descendants remain unknown.
            atexit.unregister(state_use.close)
            state_use.close()


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
) -> ProcessEnvironment:
    return _repro_context.test_custody_launch_env(
        command,
        environ=os.environ if environ is None else environ,
        cwd=cwd,
        root=ROOT,
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
infrastructure_failure_payload = _reporting.infrastructure_failure_payload


def _incident_payload(result: GuardResult) -> dict[str, object] | None:
    return _reporting.incident_payload(result, signal_payload=_exit_signal_payload)


def _write_summary_json(
    path: str,
    *,
    result: GuardResult,
    **context: Unpack[_reporting.GuardReportContext],
) -> None:
    _reporting.write_summary_json(
        path,
        **context,
        result=result,
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


def _default_incident_summary_path(environ: Mapping[str, str]) -> Path:
    return _reporting.default_incident_summary_path(
        memory_guard_state_root(ROOT, environ)
    )


def _write_running_summary_json(
    path: str,
    *,
    child_process: GuardedChildProcess | None = None,
    **context: Unpack[_reporting.GuardReportContext],
) -> None:
    _reporting.write_running_summary_json(
        path,
        **context,
        child_process=child_process,
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


def _child_env_without_internal_keys(environ: Mapping[str, str]) -> ProcessEnvironment:
    return _cli_contract.child_env_without_internal_keys(
        environ,
        internal_env_keys=_INTERNAL_ENV_KEYS,
    )


def _worker_env(
    environ: Mapping[str, str],
    command: Sequence[str],
    *,
    launch_id: str | None = None,
    startup_json: str | None = None,
) -> ProcessEnvironment:
    return _cli_contract.worker_env(
        environ,
        command,
        worker_env_name=INTERNAL_WORKER_ENV,
        command_env_name=INTERNAL_COMMAND_ENV,
        launch_id=launch_id,
        startup_json=startup_json,
    )


def _worker_argv(
    args: argparse.Namespace, *, verifier_bytecode_prefix: Path
) -> list[str]:
    return _cli_contract.worker_argv(
        args,
        python_executable=sys.executable,
        script_path=Path(__file__).resolve(),
        verifier_bytecode_prefix=verifier_bytecode_prefix,
    )


def main(
    argv: Sequence[str] | None = None,
    *,
    hide_command_argv: bool = False,
    execve: Callable[[str, list[str], Mapping[str, str]], object] = os.execve,
    environ: Mapping[str, str] | None = None,
) -> int:
    current_env = os.environ if environ is None else environ
    maintenance_cwd = Path.cwd()
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
    from molt.dx import checkout_state_maintenance_environment

    selected = checkout_state_maintenance_environment(ROOT, current_env)
    if selected is not None:
        current_env = selected
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
        from molt.memory_guard_paths import memory_guard_state_root

        # POSIX exec replaces this frame; retained control custody, rather than
        # a process-local temporary context, owns the worker's startup namespace.
        startup_root = memory_guard_state_root(ROOT, current_env)
        startup_root.mkdir(parents=True, exist_ok=True)
        verifier_bytecode_prefix = _temporary_artifacts.new_temporary_directory(
            startup_root, prefix="worker-bytecode-"
        )
        worker_argv = _worker_argv(
            args, verifier_bytecode_prefix=verifier_bytecode_prefix
        )
        if _process_model.is_windows_process_model():
            completed = subprocess.run(
                worker_argv,
                env=_worker_env(current_env, command).child_block(),
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
            _worker_env(current_env, command).child_block(),
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
    closure_progress = (
        None
        if args.progress_file is None
        else closure_progress_notifier(args.progress_file)
    )
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
        cancellation_requested=(
            None if args.cancel_file is None else Path(args.cancel_file).exists
        ),
        closure_progress=closure_progress,
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
    # The summary holds the repro record whenever there is an incident; once
    # it is written, the terminal report names it instead of repeating it.
    repro_record: str | None = None
    summary_failed = False
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
            # The report below still prints the reason, with the repro whole.
            print(f"memory_guard: failed to write summary JSON: {exc}", file=sys.stderr)
            summary_failed = True
        else:
            repro_record = args.summary_json
            if closure_progress is not None:
                closure_progress("summary_published")
    elif incident is not None:
        incident_summary_path = _default_incident_summary_path(current_env)
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
            if repro_payload is not None:
                repro_payload["summary_json"] = str(incident_summary_path)
            repro_record = str(incident_summary_path)
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
        repro_record=repro_record,
    )
    if summary_failed:
        return 2 if result.returncode == 0 else result.returncode
    # A worker with startup custody belongs to a caller that still verifies
    # source and publishes its own terminal receipt. It collects at that outer
    # boundary; this standalone facade has already published its guard result.
    if not current_env.get("MOLT_MEMORY_GUARD_STARTUP_JSON") and not current_env.get(
        "MOLT_PROOF_QUEUE"
    ):
        from molt.dx import collect_checkout_state_after_result

        collect_checkout_state_after_result(current_env, cwd=maintenance_cwd)
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(hide_command_argv=True))

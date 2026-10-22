from __future__ import annotations

from collections.abc import Callable, Mapping
import dataclasses
import errno
import io
import json
import os
from pathlib import Path
import platform
import shutil
import enum
import signal
import subprocess
import sys
import threading
import time
import traceback
import types
from typing import Any

import pytest

from tools.memory_guard_core import (
    active_custody,
    cargo_quarantine,
    memory_limits,
    process_custody,
    reporting,
)
from molt import process_model, windows_process_snapshot

import tools.memory_guard as memory_guard
from molt import file_publication, host_capacity
from molt.backend_daemon_suite_custody import LEASE_ENV
import molt.dx as molt_dx
from molt.custody_layout import out_of_tree_scratch_root
from tests.process_guard_common import (
    HUNG_CHILD_SOURCE,
    check_output_guarded_test_process,
    current_thread_only,
    guard_custody_env,
    install_module_os_view,
    install_module_view,
    install_thread_module_view,
    hung_child_marker,
    processes_carrying,
    run_in_fresh_guard_observer,
    start_owned_test_process,
    stop_test_processes_carrying,
)
from molt.memory_guard_paths import (
    active_guard_marker_dir,
    pytest_guard_summary_dir,
)

# These tests fake process data the session sentinel also reads.
pytestmark = pytest.mark.usefixtures("session_sentinel_paused")
# The guard re-exports process_custody's signal helpers, and process_model
# reads os.getpid for the watched set, so one fake must reach all three.
_GUARD_MODULES = (memory_guard, process_custody, process_model)


def _fake_guard(monkeypatch, attribute: str, real: object, **fakes: Any) -> None:
    """Fake ``real`` in every memory-guard module, on this test's thread only.

    Other threads of the pytest process call these modules too; they keep
    the real functions.
    """
    install_thread_module_view(monkeypatch, attribute, real, *_GUARD_MODULES, **fakes)


# The real scratch authority, for tests that prove custody end to end in a
# private state root; the autouse fixture below replaces it for all others.
_REAL_SCRATCH = (
    memory_guard._temporary_artifacts.acquire_guard_scratch,
    memory_guard._temporary_artifacts.finish_guard_scratch,
)


@pytest.mark.parametrize("phase", ["temporary_artifact_custody", "rss_trip_evidence"])
def test_infrastructure_failure_has_one_exact_wire_authority(
    phase: process_custody.GuardInfrastructurePhase,
) -> None:
    failure = memory_guard.GuardInfrastructureFailure(
        phase=phase, details=("invalid index", "closure unknown")
    )
    payload = {
        "phase": phase,
        "details": ["invalid index", "closure unknown"],
    }
    assert failure.json_payload() == payload
    assert memory_guard.infrastructure_failure_payload(failure) == payload
    assert memory_guard.GuardInfrastructureFailure.from_payload(payload) == failure
    assert memory_guard.GuardInfrastructureFailure.from_payload(None) is None
    assert memory_guard.infrastructure_failure_payload(None) is None


@pytest.mark.parametrize(
    "payload",
    [
        False,
        "failure",
        [],
        {},
        {"phase": "unknown", "details": ["failure"]},
        {"phase": [], "details": ["failure"]},
        {"phase": {}, "details": ["failure"]},
        {"phase": False, "details": ["failure"]},
        {"phase": "temporary_artifact_custody", "details": []},
        {"phase": "temporary_artifact_custody", "details": [""]},
        {"phase": "temporary_artifact_custody", "details": [" "]},
        {"phase": "temporary_artifact_custody", "details": [1]},
        {"phase": "temporary_artifact_custody", "details": "failure"},
        {"phase": "temporary_artifact_custody", "details": ("failure",)},
        {
            "phase": "temporary_artifact_custody",
            "details": ["failure"],
            "ignored": True,
        },
    ],
)
def test_infrastructure_failure_rejects_malformed_wire_payload(payload) -> None:
    with pytest.raises(ValueError, match="invalid guard infrastructure failure"):
        memory_guard.GuardInfrastructureFailure.from_payload(payload)


@pytest.fixture
def fake_popen_without_windows_job(monkeypatch: pytest.MonkeyPatch) -> None:
    """Keep synthetic Popen models independent of the real Windows kernel."""

    monkeypatch.setattr(
        memory_guard._win_job,
        "create_kill_on_close_job",
        lambda: None,
    )
    _patch_temporary_artifact_closure_closed(monkeypatch)


@pytest.fixture(autouse=True)
def guard_modules_use_a_private_os(monkeypatch: pytest.MonkeyPatch) -> None:
    """Every ``os`` patch in this file stays inside the guard modules."""
    install_module_os_view(
        monkeypatch,
        memory_guard,
        process_custody,
        process_model,
        cargo_quarantine,
        windows_process_snapshot,
        reporting,
        memory_limits,
    )


@pytest.fixture(autouse=True)
def no_inherited_daemon_suite_lease(monkeypatch: pytest.MonkeyPatch) -> None:
    """Keep guards under test out of the session's backend-daemon suite lease.

    The pytest session sentinel exports its live lease through os.environ so
    compiled test programs share one daemon. A guard under test inherits that
    environment and would sample the tree for, and transfer into, the live
    session lease after its child exits, so its result would depend on
    whether the file runs under the session sentinel.
    """
    monkeypatch.delenv(LEASE_ENV, raising=False)


@pytest.fixture(autouse=True)
def isolated_guard_scratch(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Keep memory-guard unit tests out of persistent repository scratch."""

    class FakeLease:
        def __init__(self) -> None:
            self.target = tmp_path / f"guard-scratch-{time.time_ns()}"
            self.target.mkdir()
            self.released = False

        def release(self) -> None:
            self.released = True

    def acquire(_root: Path, _env: Mapping[str, str]) -> FakeLease:
        return FakeLease()

    def finish(
        lease: FakeLease,
        *,
        closed: bool,
        success: bool,
        evidence: Mapping[str, object],
    ) -> Mapping[str, object]:
        del evidence
        lease.release()
        if not closed:
            state = "indeterminate"
        elif success:
            # The real authority removes a reclaimed generation's receipts.
            return {"state": "reclaimed", "receipt": None}
        else:
            state = "retained"
        return {"state": state, "receipt": str(tmp_path / "owner.json")}

    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "acquire_guard_scratch",
        acquire,
    )
    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "finish_guard_scratch",
        finish,
    )


def _guard_termination_report(
    *,
    reason: str = "test_cleanup",
    root_pid: int = 100,
    root_pgid: int | None = 100,
    watched_pids: tuple[int, ...] = (),
    actions: tuple[memory_guard.GuardTerminationAction, ...] = (),
) -> memory_guard.GuardTerminationReport:
    return memory_guard.GuardTerminationReport(
        reason=reason,
        started_at="2026-05-21T12:00:00Z",
        completed_at="2026-05-21T12:00:01Z",
        root_pid=root_pid,
        root_pgid=root_pgid,
        root_sid=None,
        grace_sec=0.125,
        watched_pids=watched_pids,
        protected_pgids=(),
        escaped_pids=(),
        remaining_pgids=(),
        remaining_pids=(),
        actions=actions,
    )


def _complete_sampling_telemetry() -> memory_guard.GuardSamplingTelemetry:
    return memory_guard.GuardSamplingTelemetry(
        attempts=1,
        successes=1,
        transient_failures=0,
    )


def _posix_custody_boundary(monkeypatch: pytest.MonkeyPatch) -> types.SimpleNamespace:
    """Select POSIX custody and its signal vocabulary on every host.

    Windows has no SIGKILL, and its ``signal.Signals`` cannot name 9, so the
    emulation carries its own enum for the names custody records (HF-49).
    Platform dispatch belongs to process_model; changing custody's local
    ``os`` observations alone does not select that model. Other consumers
    and threads keep the canonical host predicate.
    """
    install_thread_module_view(
        monkeypatch,
        "_process_model",
        process_model,
        memory_guard,
        process_custody,
        is_windows_process_model=lambda: False,
    )
    custody_signal = types.SimpleNamespace(**vars(signal))
    custody_signal.SIGKILL = 9
    custody_signal.Signals = enum.IntEnum(
        "Signals", {"SIGTERM": int(signal.SIGTERM), "SIGKILL": 9}
    )
    monkeypatch.setattr(process_custody, "signal", custody_signal)
    return custody_signal


@pytest.mark.parametrize("host_name", ["nt", "posix"])
def test_posix_custody_boundary_keeps_native_dispatch_for_other_consumers(
    monkeypatch: pytest.MonkeyPatch, host_name: str
) -> None:
    from tests.process_guard_common import ModuleView

    native_model = process_model
    native_os = native_model.os
    native_predicate = native_model.is_windows_process_model
    native_result = native_predicate()
    real_os_name = os.name
    # Only this test module's helper sees the synthetic baseline. Its
    # non-owner-thread dispatch still forwards to the real host predicate.
    monkeypatch.setitem(
        globals(),
        "process_model",
        ModuleView(
            native_model,
            is_windows_process_model=current_thread_only(
                lambda: host_name == "nt", native_predicate
            ),
        ),
    )
    assert process_model.is_windows_process_model() is (host_name == "nt")
    # Seed the callers too: omitting the helper's dispatch override must fail
    # the synthetic Windows case even when this test runs on a POSIX host.
    for caller in (memory_guard, process_custody):
        monkeypatch.setattr(caller, "_process_model", process_model)
    custody_signal = _posix_custody_boundary(monkeypatch)
    seen: list[tuple[bool, bool, bool]] = []
    worker = threading.Thread(
        target=lambda: seen.append(
            (
                process_custody._process_model.is_windows_process_model(),
                memory_guard._process_model.is_windows_process_model(),
                native_model.is_windows_process_model(),
            )
        )
    )
    worker.start()
    worker.join(5)

    assert not worker.is_alive()
    assert not process_custody._process_model.is_windows_process_model()
    assert not memory_guard._process_model.is_windows_process_model()
    assert native_model.is_windows_process_model is native_predicate
    assert native_predicate() is native_result
    assert native_result is (real_os_name == "nt")
    assert native_model.os is native_os
    assert native_os.name == real_os_name
    assert seen == [(native_result, native_result, native_result)]
    assert os.name == real_os_name
    assert custody_signal.Signals(9).name == "SIGKILL"


def _guarded_child(
    *, pid: int = 101, pgid: int | None = 101
) -> memory_guard.GuardedChildProcess:
    return memory_guard.GuardedChildProcess(
        pid=pid,
        pgid=pgid,
        sid=pgid,
        command=("python", "worker.py"),
        started_at="2026-05-21T12:00:00Z",
    )


def _windows_job_cleanup(*, active_processes: int) -> memory_guard.WindowsJobCleanup:
    accounting = memory_guard._win_job.WindowsJobAccounting(
        total_processes=1,
        active_processes=active_processes,
        total_terminated_processes=0,
        peak_job_commit_bytes=4096,
        total_user_time_100ns=0,
        total_kernel_time_100ns=0,
        total_page_fault_count=0,
    )
    resources = memory_guard._win_job.WindowsSystemResources(
        process_count=1,
        thread_count=1,
        system_handle_count=1,
        guard_handle_count=1,
        commit_total_bytes=1,
        commit_limit_bytes=2,
        commit_peak_bytes=1,
        physical_total_bytes=2,
        physical_available_bytes=1,
    )
    return memory_guard.WindowsJobCleanup(
        before=accounting,
        after=accounting,
        system_before=resources,
        system_after=resources,
        terminated_remaining_processes=False,
        remaining_processes=(),
        elapsed_s=0.01,
        initial_process_ids=(),
        escalation_process_ids=(),
        natural_exit_wait_s=0.0,
    )


def _patch_guard_popen_without_windows_job(
    monkeypatch: pytest.MonkeyPatch,
    popen_factory: Callable[..., Any],
) -> None:
    """Install a synthetic child handle with no kernel presence.

    The guard reads its child's birth, and cleanup reads a root's group and
    session, from the kernel by PID. A synthetic PID can name an unrelated
    live host process (on a busy Mac it often does), which binds the fake
    child to a foreign birth and makes the result depend on the host. Those
    reads report the synthetic child as unobservable on every host, exactly
    as when the PID is free. No synthetic child receives a real Windows Job
    handle either.
    """

    synthetic_pids: set[int] = set()

    def spawn(*args: object, **kwargs: object) -> Any:
        proc = popen_factory(*args, **kwargs)
        synthetic_pids.add(proc.pid)
        return proc

    def unobservable_if_synthetic(read: Callable[[int], Any]) -> Callable[[int], Any]:
        return lambda pid: None if pid in synthetic_pids else read(pid)

    # memory_guard alone sees the synthetic Popen: the session sentinel's
    # thread launches ps through the real subprocess module meanwhile.
    install_module_view(monkeypatch, "subprocess", subprocess, memory_guard)
    monkeypatch.setattr(memory_guard.subprocess, "Popen", spawn)
    for owner, name in (
        (process_custody, "_safe_getpgid"),
        (process_custody, "_safe_getsid"),
        (process_model, "process_started_at_ns"),
    ):
        monkeypatch.setattr(
            owner, name, unobservable_if_synthetic(getattr(owner, name))
        )
    monkeypatch.setattr(
        memory_guard._win_job,
        "create_kill_on_close_job",
        lambda: None,
    )
    _patch_temporary_artifact_closure_closed(monkeypatch)


def _patch_temporary_artifact_closure_closed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        memory_guard,
        "_temporary_artifact_descendant_closure",
        lambda **_kwargs: (
            True,
            {
                "schema": "molt.guard-scratch-closure.v1",
                "authority": "synthetic-test-closed",
                "closed": True,
            },
        ),
    )


@pytest.mark.parametrize(
    "operation",
    ["terminate_watched_processes", "cleanup_tracked_orphans", "_terminate_single_pid"],
)
def test_guard_cleanup_does_not_rebind_shared_custody_callbacks(
    monkeypatch: pytest.MonkeyPatch, operation: str
) -> None:
    owners = (
        (
            process_custody,
            (
                "_current_protected_process_group_ids",
                "_filter_protected_watched_pids",
                "terminate_watched_processes",
            ),
        ),
        (
            process_model,
            (
                "is_windows_process_model",
                "sample_processes",
                "sample_processes_posix",
                "sample_processes_windows",
                "sample_processes_custody",
            ),
        ),
    )
    originals = [
        (owner, name, getattr(owner, name)) for owner, names in owners for name in names
    ]
    for owner, name, callback in originals:
        # Restore either authority if a regressed facade mutates it before failing.
        monkeypatch.setattr(owner, name, callback)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})
    if operation == "cleanup_tracked_orphans":
        memory_guard.cleanup_tracked_orphans(
            0, tracker=process_custody.ProcessTreeTracker(0)
        )
    elif operation == "_terminate_single_pid":
        memory_guard._terminate_single_pid(0, grace=0.0)
    else:
        memory_guard.terminate_watched_processes(0)
    assert all(getattr(owner, name) is callback for owner, name, callback in originals)


def test_termination_report_validator_rejects_fake_drift() -> None:
    with pytest.raises(TypeError, match="must return GuardTerminationReport"):
        memory_guard._validated_termination_report(
            None,
            caller="terminate_watched_processes",
        )


def test_termination_report_batch_validator_rejects_fake_drift() -> None:
    with pytest.raises(TypeError, match="must return GuardTerminationReport"):
        memory_guard._validated_termination_reports(
            (_guard_termination_report(), None),
            caller="cleanup_tracked_orphans",
        )


def test_temporary_artifact_closure_without_launched_child_is_exact() -> None:
    sampled = False

    def forbidden_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sampled
        sampled = True
        raise AssertionError("no-child closure must not sample")

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=None,
        child_process=None,
        tracker=None,
        sampler=forbidden_sampler,
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=False,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=None,
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is True
    assert sampled is False
    assert evidence["authority"] == "no-child-launched"
    assert evidence["closed"] is True


def test_temporary_artifact_windows_closure_requires_exact_empty_job() -> None:
    proc = types.SimpleNamespace(returncode=0)

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=proc,
        child_process=_guarded_child(),
        tracker=None,
        sampler=lambda: {},
        windows_job_cleanup=_windows_job_cleanup(active_processes=0),
        windows_process_model=True,
        posix_process_model=False,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=None,
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is True
    assert evidence["authority"] == "windows-job-accounting"
    assert evidence["windows_job_cleanup"]["after"]["active_processes"] == 0


@pytest.mark.parametrize(
    ("returncode", "cleanup"),
    [
        (None, _windows_job_cleanup(active_processes=0)),
        (0, None),
        (0, _windows_job_cleanup(active_processes=1)),
    ],
)
def test_temporary_artifact_windows_closure_rejects_incomplete_custody(
    returncode: int | None,
    cleanup: memory_guard.WindowsJobCleanup | None,
) -> None:
    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=returncode),
        child_process=_guarded_child(),
        tracker=None,
        sampler=lambda: {},
        windows_job_cleanup=cleanup,
        windows_process_model=True,
        posix_process_model=False,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=None,
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is False
    assert evidence["closed"] is False


def test_temporary_artifact_posix_closure_requires_empty_group_and_final_sample(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        memory_guard,
        "_process_group_exited_or_unobservable",
        lambda _pgid, *, grace: True,
    )

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=memory_guard.ProcessTreeTracker(101),
        sampler=lambda: {},
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is True
    assert evidence["root_process_group_closed"] is True
    assert evidence["remaining_tracked_pids"] == []
    assert evidence["root_process_group_members"] == []


def test_temporary_artifact_posix_closure_rejects_final_survivor(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        memory_guard,
        "_process_group_exited_or_unobservable",
        lambda _pgid, *, grace: True,
    )
    tracker = memory_guard.ProcessTreeTracker(101)
    tracker.known_pids.add(202)
    tracker.known_identities[202] = memory_guard.ProcessIdentity(started_at_ns=22)
    survivor = memory_guard.ProcessSample(
        pid=202,
        ppid=1,
        rss_kb=1,
        command="python survivor.py",
        pgid=101,
        started_at_ns=22,
    )

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=tracker,
        sampler=lambda: {202: survivor},
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is False
    assert evidence["remaining_tracked_pids"] == [202]
    assert evidence["root_process_group_members"] == [202]


def test_temporary_artifact_posix_closure_rejects_final_sampler_error(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        memory_guard,
        "_process_group_exited_or_unobservable",
        lambda _pgid, *, grace: True,
    )

    def broken_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        raise RuntimeError("snapshot unavailable")

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=memory_guard.ProcessTreeTracker(101),
        sampler=broken_sampler,
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is False
    assert evidence["final_sample_error"] == "RuntimeError: snapshot unavailable"


@pytest.mark.parametrize(
    "failure_kind",
    [
        "telemetry",
        "zero_observations",
        "termination_action",
        "still_live_action",
    ],
)
def test_temporary_artifact_posix_closure_rejects_prior_custody_gap(
    failure_kind: str,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        memory_guard,
        "_process_group_exited_or_unobservable",
        lambda _pgid, *, grace: True,
    )
    telemetry = _complete_sampling_telemetry()
    reports: tuple[memory_guard.GuardTerminationReport, ...] = ()
    if failure_kind == "telemetry":
        telemetry = dataclasses.replace(
            telemetry,
            attempts=2,
            successes=1,
            transient_failures=1,
        )
    elif failure_kind == "zero_observations":
        telemetry = dataclasses.replace(telemetry, attempts=0, successes=0)
    elif failure_kind == "termination_action":
        reports = (
            _guard_termination_report(
                actions=(
                    memory_guard.GuardTerminationAction(
                        target_kind="process",
                        target_id=202,
                        signal=None,
                        signal_name=None,
                        result="skipped_missing_identity",
                    ),
                )
            ),
        )
    else:
        reports = (
            _guard_termination_report(
                actions=(
                    memory_guard.GuardTerminationAction(
                        target_kind="process",
                        target_id=202,
                        signal=memory_guard.signal.SIGTERM,
                        signal_name="SIGTERM",
                        result="still_live",
                    ),
                )
            ),
        )

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=memory_guard.ProcessTreeTracker(101),
        sampler=lambda: {},
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=telemetry,
        termination_reports=reports,
        probe_grace=0.0,
    )

    assert closed is False
    if failure_kind in {"telemetry", "zero_observations"}:
        assert evidence["sampling_enforcement_complete"] is False
    elif failure_kind == "termination_action":
        assert evidence["termination_action_gaps"][0]["result"] == (
            "skipped_missing_identity"
        )
    else:
        assert evidence["termination_action_gaps"][0]["result"] == "still_live"


@pytest.mark.parametrize(
    "case",
    [
        "proven_exit",
        "missing_only",
        "sent_only",
        "different_target",
        "different_report",
        "remaining_marker",
        "prior_failure",
        "later_failure",
        "prior_custody_gap",
        "later_custody_gap",
        "later_missing",
        "later_liveness",
        "unsupported_target_kind",
        "exit_with_signal",
        "exit_with_error",
        "prior_failure_then_completed",
        "prior_custody_gap_then_completed",
    ],
)
def test_temporary_artifact_posix_closure_reconciles_only_terminal_liveness(
    monkeypatch: pytest.MonkeyPatch, case: str
) -> None:
    def action(result: str, *, target: int = 101):
        return memory_guard.GuardTerminationAction(
            target_kind="process_group",
            target_id=target,
            signal=None,
            signal_name=None,
            result=result,
        )

    actions = [action("still_live"), action("exited")]
    if case in {"missing_only", "sent_only"}:
        actions[-1] = action("missing" if case == "missing_only" else "sent")
    elif case == "different_target":
        actions[-1] = action("exited", target=202)
    elif case in {"prior_failure", "later_failure"}:
        actions.insert(0 if case == "prior_failure" else len(actions), action("failed"))
    elif case in {"prior_custody_gap", "later_custody_gap"}:
        actions.insert(
            0 if case == "prior_custody_gap" else len(actions),
            action("skipped_identity_mismatch"),
        )
    elif case in {"later_missing", "later_liveness"}:
        actions.append(action("missing" if case == "later_missing" else "still_live"))
    elif case == "unsupported_target_kind":
        actions = [
            dataclasses.replace(row, target_kind="process_tree") for row in actions
        ]
    elif case == "exit_with_signal":
        actions[-1] = dataclasses.replace(actions[-1], signal=signal.SIGTERM)
    elif case == "exit_with_error":
        actions[-1] = dataclasses.replace(actions[-1], error="unresolved observation")
    elif case in {"prior_failure_then_completed", "prior_custody_gap_then_completed"}:
        actions.insert(
            0,
            action(
                "failed"
                if case == "prior_failure_then_completed"
                else "skipped_identity_mismatch"
            ),
        )
        actions[-1] = action("completed_or_missing")
    reports = (
        _guard_termination_report(
            reason="tracked_orphan_cleanup", actions=tuple(actions)
        ),
    )
    if case == "different_report":
        reports = tuple(
            _guard_termination_report(reason="tracked_orphan_cleanup", actions=(row,))
            for row in actions
        )
    elif case == "remaining_marker":
        reports = (dataclasses.replace(reports[0], remaining_pgids=(101,)),)
    monkeypatch.setattr(
        memory_guard,
        "_process_group_exited_or_unobservable",
        lambda _pid, *, grace: True,
    )
    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=memory_guard.ProcessTreeTracker(101),
        sampler=lambda: {},
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=reports,
        probe_grace=0.0,
    )
    assert closed is (case == "proven_exit"), evidence
    assert bool(evidence["termination_action_gaps"]) is (case != "proven_exit")
    result = memory_guard.GuardResult(
        returncode=0,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        orphaned_process_groups=(101,),
        termination_reports=reports,
        child_stderr="",
    )
    incident = memory_guard._incident_payload(result)
    assert incident is not None
    assert incident["reason"] == (
        "orphaned_processes_cleaned"
        if case == "proven_exit"
        else "orphan_cleanup_incomplete"
    )
    assert (incident.get("orphan_cleanup_status") == "incomplete") is (
        case != "proven_exit"
    )


def test_temporary_artifact_posix_closure_requires_orphan_cleanup(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    group_probed = False

    def forbidden_group_probe(_pgid: int, *, grace: float) -> bool:
        nonlocal group_probed
        group_probed = True
        raise AssertionError("disabled orphan cleanup must reject before probing")

    monkeypatch.setattr(
        memory_guard,
        "_process_group_exited_or_unobservable",
        forbidden_group_probe,
    )

    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=memory_guard.ProcessTreeTracker(101),
        sampler=lambda: {},
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=False,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=(),
        probe_grace=0.0,
    )

    assert closed is False
    assert group_probed is False
    assert evidence["cleanup_orphans_enabled"] is False


def test_parse_process_table_keeps_commands_with_spaces() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1  2048 python worker.py --flag value
          11    10  4096 /bin/sh -c echo hi
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
    )
    assert samples[11].command == "/bin/sh -c echo hi"


def test_active_guard_marker_records_death_capsule(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    marker_dir = tmp_path / "active"
    monkeypatch.setattr(process_model, "process_started_at_ns", lambda pid: 100)

    token, marker = memory_guard._write_active_guard_marker(
        os.getpid(),
        command=("python", "-c", "print('ok')"),
        cwd=tmp_path,
        environ={"MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path)},
    )

    assert marker.parent == marker_dir
    payload = json.loads(marker.read_text(encoding="utf-8"))
    assert payload["schema_version"] == 2
    assert payload["pid"] == os.getpid()
    assert payload["token"] == token
    assert payload["guard_process"] == {"pid": os.getpid(), "started_at_ns": 100}
    assert payload["child_launch_state"] == "not_started"
    assert payload["command"] == ["python", "-c", "print('ok')"]
    assert payload["cwd"] == str(tmp_path.resolve(strict=False))
    assert payload["status"] == "guard_starting"
    assert payload["created_at"]
    assert payload["updated_at"]

    memory_guard._update_active_guard_marker(
        marker,
        "wrong-token",
        status="corrupted",
    )
    assert json.loads(marker.read_text(encoding="utf-8"))["status"] == (
        "guard_starting"
    )

    memory_guard._update_active_guard_marker(
        marker,
        token,
        status="child_running",
        child_launch_state="recorded",
        child_process={"pid": 123, "started_at_ns": 200, "command": ["python"]},
    )
    updated = json.loads(marker.read_text(encoding="utf-8"))
    assert updated["status"] == "child_running"
    assert updated["child_process"]["pid"] == 123
    assert updated["child_process"]["started_at_ns"] == 200


def test_new_guard_preserves_prior_custody_records(tmp_path: Path) -> None:
    marker_dir = tmp_path / "active"
    marker_dir.mkdir()
    prior: dict[Path, bytes] = {}
    states = ("child_running", "guard_exception", "completed", "finalizer_completed")
    # Exceed the former 128-record cache cap. Unresolved and terminal parent
    # records are both evidence; neither age nor a newer launch supersedes them.
    for index in range(140):
        marker = marker_dir / f"guard-{index + 1}-{index:032x}.json"
        content = json.dumps(
            {
                "pid": index + 1,
                "token": f"{index:032x}",
                "status": states[index % len(states)],
                "termination_reports": [{"watched_pids": [index + 1000]}],
            }
        ).encode()
        marker.write_bytes(content)
        os.utime(marker, (index + 1, index + 1))
        prior[marker] = content
    unreadable = marker_dir / "guard-999-unresolved.json"
    unreadable.write_bytes(b"{incomplete")
    prior[unreadable] = b"{incomplete"

    _, new_marker = memory_guard._write_active_guard_marker(
        os.getpid(),
        command=("python", "-c", "pass"),
        cwd=tmp_path,
        environ={"MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path)},
    )

    assert new_marker not in prior
    assert set(marker_dir.glob("*.json")) == {*prior, new_marker}
    assert set(marker_dir.glob("*.lock")) == {new_marker.with_suffix(".lock")}
    assert {path: path.read_bytes() for path in prior} == prior


def _use_real_scratch(monkeypatch: pytest.MonkeyPatch) -> None:
    acquire, finish = _REAL_SCRATCH
    monkeypatch.setattr(
        memory_guard._temporary_artifacts, "acquire_guard_scratch", acquire
    )
    monkeypatch.setattr(
        memory_guard._temporary_artifacts, "finish_guard_scratch", finish
    )


def _guarded_pass(
    state_root: Path, environment=None, *, cwd=None, logical_cwd=None, text=True
) -> memory_guard.GuardResult:
    return memory_guard.run_guarded(
        [sys.executable, "-c", "pass"],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=None,
        cwd=cwd,
        logical_cwd=logical_cwd,
        text=text,
        env={
            **(os.environ if environment is None else environment),
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root),
        },
    )


def test_a_guarded_run_issues_no_fsync(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Guard custody records describe processes, so none crosses a barrier.

    Each guarded launch paid 29 fsyncs (HF-382): 0.1 to 0.5 s on the
    integrator Mac's external guard volume. An OS crash that loses a record
    also killed every process the record names; the crash-residue tests of
    the marker and scratch readers prove what each lost, empty or older
    record does.
    """
    _use_real_scratch(monkeypatch)
    flushes: list[str] = []
    real_fsync = os.fsync

    def counted(fd: int) -> None:
        flushes.append("".join(traceback.format_stack(limit=6)))
        real_fsync(fd)

    # Every barrier of the guard's records goes through the publication
    # authority; count it there, on this thread only (the session sentinel
    # shares the process).
    install_thread_module_view(monkeypatch, "os", os, file_publication, fsync=counted)
    result = _guarded_pass(tmp_path / "state")
    assert result.returncode == 0
    assert result.temporary_artifacts is not None
    assert result.temporary_artifacts["state"] == "reclaimed"
    assert flushes == []


@pytest.mark.parametrize("text", [False, True])
def test_completed_guard_retires_its_marker_lock_and_scratch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, text: bool
) -> None:
    _use_real_scratch(monkeypatch)
    state_root = tmp_path / "memory_guard"
    environment, roots = _managed_guard_use_environment(tmp_path)
    assert _guarded_pass(state_root, environment, text=text).returncode == 0
    _assert_guard_use_retirement(roots, True)
    assert list((state_root / "active").iterdir()) == []
    (retired,) = (state_root / "retired").glob("guard-*.json")
    payload = json.loads(retired.read_text(encoding="utf-8"))
    assert payload["pid"] == os.getpid()
    assert payload["status"] == "completed"
    assert payload["descendants_closed"] is True
    assert payload["temporary_artifacts"]["state"] == "reclaimed"
    assert payload["temporary_artifacts"]["receipt"] is None
    generations = [
        path.name for path in (tmp_path / "gs").iterdir() if len(path.name) == 32
    ]
    assert generations == []


@pytest.mark.parametrize("relocated", [False, True])
def test_unproven_closure_keeps_the_completed_marker_active(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, relocated: bool
) -> None:
    import atexit
    import gc
    import weakref

    proven = memory_guard._temporary_artifact_descendant_closure
    environment, roots = _managed_guard_use_environment(tmp_path)
    logical = tmp_path / "original"
    physical = tmp_path / "controller"
    logical.mkdir()
    physical.mkdir()
    if relocated:
        environment["CARGO_TARGET_DIR"] = os.path.relpath(roots[1], logical)
    callbacks = []
    observed_closed = []

    def register(callback):
        callbacks.append(callback)
        return atexit.register(callback)

    def unregister(callback):
        if callback in callbacks:
            callbacks.remove(callback)
        atexit.unregister(callback)

    install_module_view(
        monkeypatch,
        "atexit",
        atexit,
        memory_guard,
        register=register,
        unregister=unregister,
    )

    def unproven(**kwargs: Any) -> tuple[bool, dict[str, object]]:
        _closed, evidence = proven(**kwargs)
        observed_closed.append(_closed)
        return False, {**evidence, "closed": False}

    monkeypatch.setattr(
        memory_guard, "_temporary_artifact_descendant_closure", unproven
    )
    state_root = tmp_path / "memory_guard"
    result = _guarded_pass(
        state_root,
        environment,
        cwd=physical if relocated else None,
        logical_cwd=logical if relocated else None,
    )
    assert not result.descendants_closed
    del result
    gc.collect()
    _assert_guard_use_retirement(roots, False)
    # Independent original closure proved this fixture child actually ended;
    # only now may test cleanup release the deliberately retained unknown use.
    assert observed_closed == [True]
    assert len(callbacks) == 1
    retained_stack = weakref.ref(callbacks[0].__self__)
    callback = callbacks[0]
    unregister(callback)
    callback()
    del callback
    gc.collect()
    assert retained_stack() is None
    _assert_guard_use_retirement(roots, True)
    active = state_root / "active"
    (marker,) = active.glob("guard-*.json")
    payload = json.loads(marker.read_text(encoding="utf-8"))
    assert payload["status"] == "completed"
    assert payload["descendants_closed"] is False
    # Orphans may still run beside its artifacts: it protects them.
    assert not active_custody.read_marker_record(marker).terminal
    assert active_custody.has_active_guard_marker(active)


def test_spawn_failure_records_that_no_child_exists(tmp_path: Path) -> None:
    state_root = tmp_path / "memory_guard"
    missing = tmp_path / "molt-no-such-command"
    environment, roots = _managed_guard_use_environment(tmp_path)
    with pytest.raises(FileNotFoundError):
        memory_guard.run_guarded(
            [str(missing)],
            max_rss_kb=512 * 1024,
            poll_interval=0.01,
            child_rlimit_kb=None,
            env={**environment, "MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)},
        )
    (marker,) = (state_root / "active").glob("guard-*.json")
    payload = json.loads(marker.read_text(encoding="utf-8"))
    assert payload["status"] == "guard_exception"
    assert payload["child_launch_state"] == "failed"
    assert payload["child_process"] is None
    assert payload["spawn_error_type"] == "FileNotFoundError"
    record = active_custody.read_marker_record(marker)
    # Once the guard pid is gone, nothing can still hold its launch.
    observation = active_custody.observe_custody(record, {}, now_s=time.time())
    assert observation.state == "closed"

    _assert_guard_use_retirement(roots, True)


def test_exiting_guard_sweeps_dead_custody_records(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _use_real_scratch(monkeypatch)
    state_root = tmp_path / "memory_guard"
    active = state_root / "active"
    live = set(process_model.sample_processes())
    dead = [pid for pid in range(70_000, 90_000) if pid not in live][
        : active_custody.AUTO_SWEEP_GROWTH + 1
    ]
    for pid in dead:
        token = f"{pid:032x}"
        active_custody.write_active_guard_marker(
            active / f"guard-{pid}-{token}.json",
            {
                "schema_version": 2,
                "pid": pid,
                "token": token,
                "status": "launch_prepared",
                "guard_process": {"pid": pid, "started_at_ns": 1},
                "child_process": None,
                "child_launch_state": "not_started",
            },
        )
    assert _guarded_pass(state_root).returncode == 0
    assert list(active.iterdir()) == []
    assert len(list((state_root / "retired").glob("guard-*.json"))) == len(dead) + 1
    receipt = json.loads((state_root / "sweep.json").read_text(encoding="utf-8"))
    assert receipt["remaining"] == 0


def test_active_guard_markers_follow_external_artifact_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo_root = tmp_path / "repo"
    artifact_root = tmp_path / "artifacts"
    # The synthetic repo is a plain clone, not explicit host scratch.
    monkeypatch.setattr(
        molt_dx, "_host_scratch_roots", lambda: ((tmp_path / "ambient").resolve(),)
    )

    # A plain clone's guard state goes to its out-of-tree scratch.
    default_markers = active_guard_marker_dir(repo_root, {})
    assert default_markers == (
        out_of_tree_scratch_root(repo_root) / "memory_guard" / "active"
    )
    assert repo_root.resolve() not in default_markers.parents
    assert active_guard_marker_dir(
        repo_root, {"MOLT_EXT_ROOT": str(artifact_root)}
    ) == (artifact_root / "tmp" / "memory_guard" / "active").resolve(strict=False)
    # Candidate roots count only when the run asks for an external root.
    assert (
        active_guard_marker_dir(
            repo_root, {"MOLT_EXTERNAL_ARTIFACT_ROOTS": str(artifact_root)}
        )
        == default_markers
    )
    # Memory scratch never moves the markers every observer reads.
    ram = tmp_path / "ram"
    ram.mkdir()
    assert (
        active_guard_marker_dir(repo_root, {"MOLT_SCRATCH_STORAGE": str(ram)})
        == default_markers
    )
    state_root = tmp_path / "proof-control" / "memory_guard"
    assert active_guard_marker_dir(
        repo_root,
        {
            "MOLT_EXT_ROOT": str(artifact_root),
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root),
        },
    ) == (state_root / "active").resolve(strict=False)
    # Pytest custody sits beside the unconfigured guard state, out of tree.
    default_pytest = pytest_guard_summary_dir(repo_root, {})
    assert default_pytest == default_markers.parent.parent / "pytest-memory-guard"
    assert repo_root.resolve() not in default_pytest.parents
    assert pytest_guard_summary_dir(
        repo_root, {"MOLT_EXT_ROOT": str(artifact_root)}
    ) == (artifact_root / "tmp" / "pytest-memory-guard").resolve(strict=False)
    assert pytest_guard_summary_dir(
        repo_root,
        {"MOLT_MEMORY_GUARD_STATE_ROOT": str(state_root)},
    ) == (state_root.parent / "pytest-memory-guard").resolve(strict=False)


def test_parse_process_table_reads_process_group_ids() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1    10  2048 python worker.py --flag value
          11    10    10  4096 /bin/sh -c echo hi
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
        pgid=10,
    )
    assert samples[11].pgid == 10


def test_parse_process_table_reads_process_elapsed_age() -> None:
    samples = memory_guard.parse_process_table(
        """
          10     1    10  2048  901 python worker.py --flag value
          11    10    10  4096  01:02:03 /bin/sh -c echo hi
          12    10    10  4096  2-03:04:05 python slow.py
        """
    )

    assert samples[10] == memory_guard.ProcessSample(
        pid=10,
        ppid=1,
        rss_kb=2048,
        command="python worker.py --flag value",
        pgid=10,
        elapsed_sec=901,
    )
    assert samples[11].elapsed_sec == 3723
    assert samples[12].elapsed_sec == 183845


def test_parse_process_table_with_start_produces_creation_identity() -> None:
    samples = memory_guard.parse_process_table_with_start(
        "10 1 10 2048 Thu Jul 17 07:15:01 2026 python worker.py --flag value\n"
    )

    assert samples[10].command == "python worker.py --flag value"
    assert samples[10].started_at_ns is not None
    assert memory_guard.process_identity(samples[10]).started_at_ns == (
        samples[10].started_at_ns
    )


def _write_linux_proc_sample(
    root: Path,
    *,
    pid: int,
    ppid: int,
    pgid: int,
    start_ticks: int,
) -> None:
    proc = root / str(pid)
    proc.mkdir(parents=True)
    # Fields after the comm: state, ppid, pgid, ..., starttime (index 19),
    # vsize (20), rss pages (21). The sampler reads rss from this row, so the
    # fixture carries no `status` file at all.
    rss_pages = 4096 // memory_guard._process_model._LINUX_PAGE_KB
    tail = [
        "S",
        str(ppid),
        str(pgid),
        *(["0"] * 16),
        str(start_ticks),
        "0",
        str(rss_pages),
    ]
    (proc / "stat").write_text(
        f"{pid} (worker) {' '.join(tail)}\n",
        encoding="utf-8",
    )
    (proc / "cmdline").write_bytes(b"python\0worker.py\0")


def test_linux_custody_poll_walks_thread_children_not_every_proc_entry(
    tmp_path: Path,
) -> None:
    model = memory_guard._process_model
    # 100 forked 101 from its main thread; 101 forked 102 from a second
    # thread. 103 was reparented to init and stays tracked by pid. 104's
    # number was reused by an unrelated process between the two reads. 500 is
    # the suite lease owner: its row, never its children. 900 is unrelated.
    for pid, ppid, pgid in (
        (100, 50, 100),
        (101, 100, 100),
        (102, 101, 100),
        (103, 1, 103),
        (104, 999, 999),
        (500, 1, 500),
        (501, 500, 500),
        (900, 1, 900),
    ):
        _write_linux_proc_sample(
            tmp_path, pid=pid, ppid=ppid, pgid=pgid, start_ticks=pid
        )
    for pid, tasks in {
        100: {100: "101 104"},
        101: {101: "", 1011: "102"},
        102: {102: ""},
        103: {103: ""},
        500: {500: "501"},
    }.items():
        for tid, children in tasks.items():
            task = tmp_path / str(pid) / "task" / str(tid)
            task.mkdir(parents=True)
            (task / "children").write_text(children, encoding="utf-8")

    interest = model.CustodyInterest(
        tree_pids=frozenset({100, 103}), exact_pids=frozenset({500})
    )
    custody = model.sample_processes_linux_proc_custody(
        interest, tmp_path, uptime_sec=1000.0
    )
    full = model.sample_processes_linux_proc(tmp_path, uptime_sec=1000.0)

    assert set(custody) == {100, 101, 102, 103, 500}
    by_custody = model.ProcessTreeTracker(100)
    by_custody.known_pids.add(103)
    by_table = model.ProcessTreeTracker(100)
    by_table.known_pids.add(103)
    assert by_custody.update(custody) == by_table.update(full) == {100, 101, 102, 103}
    assert custody[102].rss_kb == full[102].rss_kb == 4096


def test_linux_proc_sampler_binds_lineage_identity_command_and_rss(
    tmp_path: Path,
) -> None:
    _write_linux_proc_sample(
        tmp_path,
        pid=200,
        ppid=100,
        pgid=200,
        start_ticks=321,
    )

    samples = process_model.sample_processes_linux_proc(tmp_path, uptime_sec=1000.0)

    assert samples[200].ppid == 100
    assert samples[200].pgid == 200
    assert samples[200].command == "python worker.py"
    assert samples[200].rss_kb == 4096
    assert samples[200].started_at_ns is not None
    assert samples[200].elapsed_sec is not None


def test_linux_proc_sampler_binds_no_argv_for_a_pid_reused_before_its_read(
    tmp_path: Path,
) -> None:
    _write_linux_proc_sample(
        tmp_path,
        pid=200,
        ppid=100,
        pgid=200,
        start_ticks=321,
    )
    observations = iter(
        (
            (100, 200, 321_000, "worker"),
            (4, 200, 322_000, "System"),
        )
    )

    sample = process_model.sample_processes_linux_proc(
        tmp_path,
        stat_reader=lambda _pid, _root: next(observations),
        uptime_sec=1000.0,
    )[200]

    # The snapshot keeps the instance it read; the later cmdline belongs to
    # another birth, so it binds no argv.
    assert (sample.ppid, sample.pgid, sample.started_at_ns) == (100, 200, 321_000)
    assert sample.argv == ()
    assert sample.command == "worker"


def test_linux_proc_sampler_reads_cmdline_only_for_rows_a_caller_reads(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model = memory_guard._process_model
    for pid in (200, 201, 202):
        _write_linux_proc_sample(tmp_path, pid=pid, ppid=1, pgid=pid, start_ticks=pid)
    reads: list[int] = []
    real_argv = model._linux_proc_argv

    def counted_argv(pid: int, proc_root: Path) -> tuple[str, ...] | None:
        reads.append(pid)
        return real_argv(pid, proc_root)

    monkeypatch.setattr(model, "_linux_proc_argv", counted_argv)
    samples = model.sample_processes_linux_proc(
        tmp_path,
        stat_reader=lambda pid, _root: (1, pid, pid * 10_000_000, "worker"),
        uptime_sec=1000.0,
    )

    assert reads == []
    assert sorted(samples) == [200, 201, 202]
    assert samples[201].command == "python worker.py"
    assert samples[201].argv == ("python", "worker.py")
    assert samples[201].command == "python worker.py"
    assert reads == [201]


def test_linux_proc_sampler_preserves_typed_enumeration_failure(
    tmp_path: Path,
) -> None:
    with pytest.raises(memory_guard.ProcessSnapshotError, match="enumeration failed"):
        process_model.sample_processes_linux_proc(tmp_path / "missing")


_DARWIN_VIEWER_UID = 501


def _darwin_kernel_row(
    status: int = 2,
    *,
    ppid: int = 3,
    pgid: int = 7,
    started_at_ns: int = 123_000_000,
    command: str = "python3.12",
    uid: int = _DARWIN_VIEWER_UID,
):
    model = memory_guard._process_model
    return model._DarwinKernelProcRow(
        status=status,
        ppid=ppid,
        pgid=pgid,
        started_at_ns=started_at_ns,
        command=command,
        uid=uid,
    )


def _install_darwin_host(
    monkeypatch: pytest.MonkeyPatch,
    table: Mapping[int, Any],
    *,
    argv: Callable[[int], tuple[str, ...] | None],
    later_rows: Callable[[int], Any] | None = None,
    resident_kb: Callable[[int], int] | None = None,
    viewer_uid: int = _DARWIN_VIEWER_UID,
) -> dict[str, list[int]]:
    """Fake the Darwin kernel through the sampler's own module seams."""
    model = memory_guard._process_model
    reads: dict[str, list[int]] = {"argv": [], "kernel_row": [], "resident_kb": []}

    def read_argv(pid: int) -> tuple[str, ...] | None:
        reads["argv"].append(pid)
        return argv(pid)

    def read_kernel_row(pid: int) -> Any:
        reads["kernel_row"].append(pid)
        return (later_rows or table.get)(pid)

    def read_resident_kb(pid: int) -> int:
        reads["resident_kb"].append(pid)
        return 2048 if resident_kb is None else resident_kb(pid)

    monkeypatch.setattr(model, "_darwin_proc_table", lambda: dict(table))
    monkeypatch.setattr(model, "_darwin_proc_argv", read_argv)
    monkeypatch.setattr(model, "_darwin_proc_kernel_row", read_kernel_row)
    monkeypatch.setattr(model, "_darwin_proc_resident_kb", read_resident_kb)
    monkeypatch.setattr(model, "_darwin_viewer_uid", lambda: viewer_uid)
    return reads


def test_darwin_sampler_keeps_bound_launcher_arguments_for_host_protection(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model = memory_guard._process_model
    _install_darwin_host(
        monkeypatch,
        {7: _darwin_kernel_row(started_at_ns=123_456_789, command="node")},
        argv=lambda _pid: (
            "node",
            "/opt/node_modules/@openai/codex/bin/codex.js",
            "app-server",
        ),
    )

    samples = model._sample_processes_darwin()

    assert samples[7].ppid == 3
    assert samples[7].started_at_ns == 123_456_789
    assert samples[7].rss_kb == 2048
    assert "@openai/codex" in samples[7].command
    assert memory_guard.is_host_control_plane_process(samples[7])


def test_darwin_sampler_reads_argv_only_for_rows_a_caller_reads(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model = memory_guard._process_model
    table = {
        pid: _darwin_kernel_row(ppid=1, pgid=pid, started_at_ns=pid * 1_000)
        for pid in (7, 8, 9)
    }
    reads = _install_darwin_host(
        monkeypatch, table, argv=lambda pid: ("worker", str(pid))
    )

    samples = model._sample_processes_darwin()

    assert reads["argv"] == []
    assert reads["kernel_row"] == []
    assert (samples[8].ppid, samples[8].pgid, samples[8].started_at_ns) == (
        1,
        8,
        8_000,
    )
    assert samples[8].command == "worker 8"
    assert samples[8].argv == ("worker", "8")
    assert reads["argv"] == [8]
    assert reads["kernel_row"] == [8]
    assert reads["resident_kb"] == []
    assert dataclasses.replace(samples[8], rss_kb=1) == memory_guard.ProcessSample(
        pid=8,
        ppid=1,
        rss_kb=1,
        command="worker 8",
        pgid=8,
        elapsed_sec=samples[8].elapsed_sec,
        started_at_ns=8_000,
        argv=("worker", "8"),
    )


def test_darwin_sampler_sizes_only_visited_processes_once(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    model = memory_guard._process_model
    table = {
        7: _darwin_kernel_row(ppid=3, started_at_ns=7_000),
        8: _darwin_kernel_row(ppid=7, pgid=8, started_at_ns=8_000),
        9: _darwin_kernel_row(ppid=1, pgid=9, started_at_ns=9_000),
    }
    resident = {7: 40, 8: 60, 9: 10_000}
    reads = _install_darwin_host(
        monkeypatch,
        table,
        argv=lambda pid: ("worker", str(pid)),
        resident_kb=resident.__getitem__,
    )
    samples = model._sample_processes_darwin()
    tracker = model.ProcessTreeTracker(7)
    watched = tracker.update(samples)
    assert watched == {7, 8}
    assert reads == {"argv": [], "kernel_row": [], "resident_kb": []}, reads

    # Command classification and the identity census must never size a row.
    assert samples[9].argv == ("worker", "9")
    assert reads["resident_kb"] == []
    peak = model.peak_rss(samples, root_pid=7, watched=watched)
    assert peak is not None and (peak.pid, peak.rss_kb) == (8, 60)
    assert reads["resident_kb"] == [7, 8]

    # An independent mutable authority changes, but this sample stays frozen
    # across every accounting consumer.
    resident.update({7: 0, 8: 0})
    total = model.total_rss(samples, root_pid=7, watched=watched)
    assert total is not None and total.rss_kb == 100
    violation = model.find_rss_violation(
        samples, root_pid=7, watched=watched, max_rss_kb=50
    )
    assert violation is not None and (violation.pid, violation.rss_kb) == (8, 60)
    aggregate = model.find_rss_violation(
        samples, root_pid=7, watched=watched, max_rss_kb=100, max_total_rss_kb=90
    )
    assert aggregate is not None and aggregate.rss_kb == 100
    assert samples[7].rss_kb == 40
    assert reads["resident_kb"] == [7, 8]

    # A fresh sample binds a fresh value; an explicitly reported outside row
    # pays for that row only. Serialization visits details intentionally.
    fresh = model._sample_processes_darwin()
    assert fresh[7].rss_kb == 0
    resident[7] = 200
    assert fresh[7].rss_kb == 0
    assert dataclasses.asdict(samples[9])["rss_kb"] == 10_000
    assert reads["resident_kb"] == [7, 8, 7, 9]


@pytest.mark.parametrize("later", [None, "zombie", "reused"])
def test_darwin_sampler_binds_zero_rss_after_sampled_instance_exits(
    monkeypatch: pytest.MonkeyPatch, later: str | None
) -> None:
    model = memory_guard._process_model
    row = _darwin_kernel_row(started_at_ns=7_000)
    later_row = {
        None: None,
        "zombie": _darwin_kernel_row(model._DARWIN_SZOMB, started_at_ns=7_000),
        "reused": _darwin_kernel_row(started_at_ns=8_000),
    }[later]
    reads = _install_darwin_host(
        monkeypatch,
        {7: row},
        argv=lambda _pid: ("worker",),
        later_rows=lambda _pid: later_row,
    )
    sample = model._sample_processes_darwin()[7]
    assert sample.rss_kb == sample.rss_kb == 0
    assert sample.started_at_ns == 7_000
    assert reads["resident_kb"] == [7]
    assert reads["argv"] == []


def test_native_resident_binding_runs_once_across_readers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from concurrent.futures import ThreadPoolExecutor

    model = memory_guard._process_model
    reads = _install_darwin_host(
        monkeypatch,
        {7: _darwin_kernel_row()},
        argv=lambda _pid: ("worker",),
    )
    sample = model._sample_processes_darwin()[7]
    with ThreadPoolExecutor(max_workers=4) as pool:
        assert list(pool.map(lambda _: sample.rss_kb, range(16))) == [2048] * 16
    assert reads["resident_kb"] == [7]
    assert reads["argv"] == []


@pytest.mark.parametrize(
    "later_row",
    [
        pytest.param(None, id="reaped"),
        pytest.param("szomb", id="exited-unreaped"),
        pytest.param("reused", id="pid-reused"),
    ],
)
def test_darwin_sampler_binds_no_argv_once_the_sampled_birth_is_gone(
    monkeypatch: pytest.MonkeyPatch,
    later_row: str | None,
) -> None:
    model = memory_guard._process_model
    sampled = _darwin_kernel_row(started_at_ns=123_456_789, command="node")
    later = {
        None: None,
        "szomb": _darwin_kernel_row(model._DARWIN_SZOMB, started_at_ns=123_456_789),
        "reused": _darwin_kernel_row(started_at_ns=123_456_790, command="node"),
    }[later_row]
    _install_darwin_host(
        monkeypatch,
        {7: sampled},
        argv=lambda _pid: ("node", "/opt/node_modules/@openai/codex/bin/codex.js"),
        later_rows=lambda _pid: later,
    )

    sample = model._sample_processes_darwin()[7]

    # The table row is one atomic kernel copy of the sampled instance; argv
    # read after that instance ended belongs to no one and binds nothing.
    assert (sample.ppid, sample.pgid, sample.started_at_ns) == (3, 7, 123_456_789)
    assert sample.argv == ()
    assert sample.command == "node"
    assert not memory_guard.is_host_control_plane_process(sample)


def test_darwin_sampler_omits_exited_process_awaiting_wait(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A SZOMB row stays in the kernel table; it is no live member."""
    model = memory_guard._process_model
    _install_darwin_host(
        monkeypatch,
        {
            7: _darwin_kernel_row(model._DARWIN_SZOMB),
            8: _darwin_kernel_row(pgid=8, started_at_ns=123_456_789),
        },
        argv=lambda _pid: ("cargo", "build"),
    )

    samples = model._sample_processes_darwin()

    assert 7 not in samples
    assert samples[8].started_at_ns == 123_456_789


@pytest.mark.parametrize("viewer_uid", [_DARWIN_VIEWER_UID, 0])
def test_darwin_sampler_leaves_rows_withheld_from_the_viewer_unbound(
    monkeypatch: pytest.MonkeyPatch,
    viewer_uid: int,
) -> None:
    """launchd answers neither KERN_PROCARGS2 nor task info to another user."""
    model = memory_guard._process_model
    reads = _install_darwin_host(
        monkeypatch,
        {
            1: _darwin_kernel_row(
                ppid=0, pgid=1, started_at_ns=5_000, command="launchd", uid=0
            )
        },
        argv=lambda _pid: ("/sbin/launchd",),
        viewer_uid=viewer_uid,
    )

    sample = model._sample_processes_darwin()[1]

    if viewer_uid == 0:
        # Root reads every process, so the row binds like any other.
        assert sample.started_at_ns == 5_000
        assert sample.argv == ("/sbin/launchd",)
        assert reads["resident_kb"] == []
        assert sample.rss_kb == 2048
        assert reads["resident_kb"] == [1]
        return
    assert (sample.ppid, sample.pgid, sample.rss_kb) == (0, 1, 0)
    assert sample.started_at_ns is None
    assert sample.argv == ()
    assert sample.command == "launchd"
    assert reads == {"argv": [], "kernel_row": [], "resident_kb": []}


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin kernel process table")
def test_actual_darwin_kernel_table_matches_this_process_and_ps() -> None:
    model = memory_guard._process_model
    authority = model._load_darwin_process_authority()

    assert authority.kinfo.size == model._DARWIN_KINFO_PROC_SIZE
    table = authority.kernel_table()
    row = authority.kernel_row(os.getpid())
    assert row is not None
    assert table[os.getpid()] == row
    assert row.status != model._DARWIN_SZOMB
    assert (row.ppid, row.pgid, row.uid) == (os.getppid(), os.getpgrp(), os.geteuid())
    # ps formats the same birth to the second through its own code path.
    started = str(
        check_output_guarded_test_process(
            ["ps", "-o", "lstart=", "-p", str(os.getpid())],
            env={**os.environ, "LC_ALL": "C"},
            timeout=30.0,
        )
    ).strip()
    ps_started_s = time.mktime(time.strptime(started, "%a %b %d %H:%M:%S %Y"))
    assert abs(row.started_at_ns / 1_000_000_000 - ps_started_s) < 1.0
    assert row.started_at_ns == model._darwin_proc_started_at_ns(os.getpid())


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin kernel process table")
def test_actual_darwin_sampler_omits_unreaped_child() -> None:
    model = memory_guard._process_model
    # The child is owned by this guarded test but not reaped until after the
    # kernel zombie observation. Popen execs a fresh interpreter instead of
    # forking the threaded guard/test process.
    child = start_owned_test_process(
        [
            sys.executable,
            "-I",
            "-S",
            "-c",
            "import os; os.write(1, b'x'); os.read(0, 1)",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    try:
        assert child.stdout is not None and child.stdin is not None
        assert child.stdout.read(1) == b"x"
        sample = model.sample_processes_posix().get(child.pid)
        assert sample is not None
        assert sample.ppid == os.getpid()
        assert sample.started_at_ns == model._darwin_proc_started_at_ns(child.pid)
        child.stdin.close()
        # An exiting process fails libproc lookups while its kernel status is
        # still SRUN, so wait on the kernel row itself (HF-88).
        deadline = time.monotonic() + 5.0
        while True:
            row = model._darwin_proc_kernel_row(child.pid)
            if row is not None and row.status == model._DARWIN_SZOMB:
                break
            assert time.monotonic() < deadline, "child never reached SZOMB"
            time.sleep(0.02)
        assert row.started_at_ns == sample.started_at_ns
        assert child.pid in model._darwin_proc_table()
        assert child.pid not in model.sample_processes_posix()
    finally:
        if child.stdin is not None and not child.stdin.closed:
            child.stdin.close()
        child.wait(timeout=5)
        if child.stdout is not None:
            child.stdout.close()
    assert model._darwin_proc_kernel_row(child.pid) is None
    assert child.pid not in model.sample_processes_posix()


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin kernel process filters")
def test_actual_darwin_custody_poll_tracks_the_tree_the_host_table_does() -> None:
    model = memory_guard._process_model
    authority = model._load_darwin_process_authority()
    # A live tree: this test's child holds two real grandchildren.
    child = start_owned_test_process(
        [
            sys.executable,
            "-I",
            "-S",
            "-c",
            "import os, subprocess\n"
            "kids = [subprocess.Popen(['/bin/sleep', '30']) for _ in range(2)]\n"
            "os.write(1, b'x'); os.read(0, 1)\n"
            "for kid in kids: kid.kill(); kid.wait()\n",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    try:
        assert child.stdout is not None and child.stdin is not None
        assert child.stdout.read(1) == b"x"
        table = authority.kernel_table()
        grandchildren = {pid for pid, row in table.items() if row.ppid == child.pid}
        assert len(grandchildren) == 2
        assert set(authority.child_pids(child.pid)) == grandchildren
        lineage = model.posix_custody_lineage(os.getpid())
        assert {os.getpid(), 1} <= lineage
        interest = model.CustodyInterest(
            tree_pids=frozenset({child.pid}), exact_pids=lineage
        )
        custody = model._sample_processes_darwin_custody(interest)
        full = model._darwin_samples(table)
        by_table = model.ProcessTreeTracker(child.pid).update(full)
        by_custody = model.ProcessTreeTracker(child.pid).update(custody)
        assert by_custody == {child.pid, *grandchildren}
        # Rows outside the tree differ in count, never in the tracked set.
        assert by_custody == by_table
        assert len(custody) < len(full)
        # Protection judges tree members through their ancestry, which the
        # lineage keeps: the verdicts match the host table's, owned or not.
        groups = {full[pid].pgid for pid in by_table}
        for owned in (frozenset(by_table), frozenset()):
            verdicts = []
            for samples in (custody, full):
                protected = model.protected_process_group_ids(
                    samples,
                    self_pid=os.getpid(),
                    self_pgid=os.getpgrp(),
                    owned_pids=owned,
                )
                verdicts.append(
                    (
                        {pgid for pgid in groups if pgid in protected},
                        model.filter_protected_watched_pids(
                            samples,
                            set(by_table),
                            protected_pgids=protected,
                            current_pid=os.getpid(),
                        ),
                    )
                )
            assert verdicts[0] == verdicts[1]
    finally:
        if not child.stdin.closed:
            child.stdin.close()
        child.wait(timeout=5)
        if child.stdout is not None:
            child.stdout.close()


@pytest.mark.skipif(
    not memory_guard._process_model.posix_custody_reads_partial_rows(),
    reason="this host reads the process table for custody",
)
def test_actual_custody_poll_judges_a_live_tree_as_the_host_table_does() -> None:
    model = memory_guard._process_model
    child = start_owned_test_process(
        [
            sys.executable,
            "-I",
            "-S",
            "-c",
            "import os, subprocess\n"
            "kids = [subprocess.Popen(['sleep', '30']) for _ in range(2)]\n"
            "os.write(1, b'x'); os.read(0, 1)\n"
            "for kid in kids: kid.kill(); kid.wait()\n",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
    )
    try:
        assert child.stdout is not None and child.stdin is not None
        assert child.stdout.read(1) == b"x"
        full = model.sample_processes_posix()
        custody = model.sample_processes_posix_custody(
            model.CustodyInterest(
                tree_pids=frozenset({child.pid}),
                exact_pids=model.posix_custody_lineage(os.getpid()),
            )
        )
        by_table = model.ProcessTreeTracker(child.pid).update(full)
        by_custody = model.ProcessTreeTracker(child.pid).update(custody)
        assert len(by_custody) == 3 and by_custody == by_table
        assert len(custody) < len(full)
        groups = {full[pid].pgid for pid in by_table}
        for owned in (frozenset(by_table), frozenset()):
            verdicts = []
            for samples in (custody, full):
                protected = model.protected_process_group_ids(
                    samples,
                    self_pid=os.getpid(),
                    self_pgid=os.getpgrp(),
                    owned_pids=owned,
                )
                verdicts.append(
                    (
                        {pgid for pgid in groups if pgid in protected},
                        model.filter_protected_watched_pids(
                            samples,
                            set(by_table),
                            protected_pgids=protected,
                            current_pid=os.getpid(),
                        ),
                    )
                )
            assert verdicts[0] == verdicts[1]
    finally:
        if not child.stdin.closed:
            child.stdin.close()
        child.wait(timeout=5)
        if child.stdout is not None:
            child.stdout.close()


def test_descendant_pids_includes_grandchildren() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root"),
        101: memory_guard.ProcessSample(101, 100, 20, "child"),
        102: memory_guard.ProcessSample(102, 101, 30, "grandchild"),
        200: memory_guard.ProcessSample(200, 1, 999_999, "unrelated"),
    }

    assert memory_guard.descendant_pids(samples, 100) == {100, 101, 102}


def test_default_windows_sampler_uses_hard_timeout_snapshot(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    reads: list[str] = []

    def bounded_snapshot():
        reads.append("bounded")
        return [(7, 1, "python.exe", 3, 123, "full")]

    def native_snapshot():
        raise AssertionError("default sampling bypassed hard-timeout isolation")

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        process_model, "_windows_process_snapshot_rows_hard_timeout", bounded_snapshot
    )
    monkeypatch.setattr(
        windows_process_snapshot, "_windows_process_snapshot_rows", native_snapshot
    )

    samples = memory_guard.sample_processes()

    assert sorted(samples) == [7]
    assert samples[7].ppid == 1
    assert samples[7].started_at_ns == 123
    assert samples[7].command == "python.exe"
    assert reads == ["bounded"]


def test_custody_ancestry_freezes_live_births_and_request_admission():
    sample = memory_guard.ProcessSample
    tracker = memory_guard.ProcessTreeTracker(100)
    # Deliberately leaf-first: lineage cannot depend on sampler row ordering.
    samples = {
        300: sample(300, 200, 10, "compiler", pgid=300, started_at_ns=3000),
        200: sample(200, 100, 10, "batch", pgid=200, started_at_ns=2000),
        100: sample(100, 1, 10, "suite", pgid=100, started_at_ns=1000),
    }
    tracker.update(samples, observed_at_ns=100)
    record = tracker.custody_ancestry_payload({300: samples[300]})[0]
    assert record["admitted_at_ns"] == 100
    assert record["ancestors"] == [
        {"pid": 200, "started_at_ns": 2000},
        {"pid": 100, "started_at_ns": 1000},
    ]
    reparented = {300: sample(300, 1, 10, "compiler", pgid=300, started_at_ns=3000)}
    tracker.update(reparented, observed_at_ns=200)
    assert tracker.custody_ancestry_payload(reparented) == [record]
    assert tracker.custody_ancestry_payload(reparented, excluded_roots={200}) == []
    degraded = {300: sample(300, 1, 10, "compiler", pgid=300)}
    tracker.update(degraded, observed_at_ns=300)
    assert tracker.custody_ancestry_payload(degraded) == []
    assert tracker.custody_ancestry_payload(reparented) == [record]
    reused = {300: sample(300, 1, 10, "unrelated", pgid=300, started_at_ns=3001)}
    tracker.update(reused, observed_at_ns=400)
    assert tracker.custody_ancestry_payload(reused) == []


@pytest.mark.parametrize("leaf_first", [False, True])
def test_stale_parent_birth_cannot_admit_membership_or_request_ancestry(leaf_first):
    sample = memory_guard.ProcessSample
    tracker = memory_guard.ProcessTreeTracker(100)
    rows = [
        sample(100, 1, 10, "suite", started_at_ns=1000),
        sample(200, 100, 10, "batch", started_at_ns=2000),
        sample(300, 200, 10, "compiler", started_at_ns=3000),
        sample(400, 300, 10, "older unrelated", started_at_ns=2500),
        sample(500, 400, 10, "unrelated child", started_at_ns=4000),
    ]
    samples = {row.pid: row for row in (reversed(rows) if leaf_first else rows)}
    assert tracker.update(samples, observed_at_ns=100) == {100, 200, 300}
    assert memory_guard.watched_pids(samples, 100) == {100, 200, 300}
    assert memory_guard.total_rss(samples, root_pid=100).rss_kb == 30
    assert tracker.custody_identities({400, 500}) == {}
    assert (
        tracker.custody_ancestry_payload({400: samples[400], 500: samples[500]}) == []
    )
    assert tracker.custody_ancestry_payload({300: samples[300]})[0]["ancestors"] == [
        {"pid": 200, "started_at_ns": 2000},
        {"pid": 100, "started_at_ns": 1000},
    ]


@pytest.mark.parametrize(
    "parent_birth,child_birth,admitted",
    [
        (1000, 1000, True),
        (1000, 1001, True),
        (1001, 1000, False),
        (None, 1000, False),
        (1000, None, False),
        (0, 1000, False),
        (1000, 0, False),
        (-1, 1000, False),
        (1000, -1, False),
        (True, 1000, False),
        (1, True, False),
        (1000.0, 1000, False),
        (1000, 1000.0, False),
        ("1000", 1000, False),
        (1000, "1000", False),
    ],
)
def test_new_custody_edge_requires_ordered_exact_births(
    parent_birth, child_birth, admitted
):
    sample = memory_guard.ProcessSample
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        200: sample(200, 100, 10, "worker", started_at_ns=child_birth),
        100: sample(100, 1, 10, "suite", started_at_ns=parent_birth),
    }
    assert tracker.update(samples, observed_at_ns=100) == (
        {100, 200} if admitted else {100}
    )
    assert memory_guard.watched_pids(samples, 100) == (
        {100, 200} if admitted else {100}
    )
    payload = tracker.custody_ancestry_payload({200: samples[200]})
    assert bool(payload) is admitted


def test_historical_custody_does_not_create_an_impossible_new_parent_edge():
    sample = memory_guard.ProcessSample
    # Explicitly adopted members can predate the command root. Their historical
    # membership is authoritative, but a stale PPID cannot manufacture ancestry.
    tracker = memory_guard.ProcessTreeTracker(
        100,
        known_pids={200},
        known_identities={200: memory_guard.ProcessIdentity(1000)},
    )
    samples = {
        100: sample(100, 1, 10, "new command", started_at_ns=2000),
        200: sample(200, 100, 10, "adopted worker", started_at_ns=1000),
    }
    assert tracker.update(samples, observed_at_ns=100) == {100, 200}
    assert tracker.custody_ancestry_payload({200: samples[200]}) == []


def test_custody_ancestry_stays_cut_after_suite_adopted_daemon_exits():
    sample = memory_guard.ProcessSample
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: sample(100, 1, 10, "suite", started_at_ns=1000),
        200: sample(200, 100, 10, "batch", started_at_ns=2000),
        300: sample(300, 200, 10, "daemon", started_at_ns=3000),
        400: sample(400, 300, 10, "compiler", started_at_ns=4000),
    }
    tracker.update(samples, observed_at_ns=100)
    tracker.cut_ancestry_at(
        {300: memory_guard.ProcessIdentity(3000)}, observed_at_ns=110
    )
    assert tracker.custody_ancestry_payload({300: samples[300]}) == []
    assert tracker.custody_ancestry_payload({400: samples[400]})[0]["ancestors"] == [
        {"pid": 300, "started_at_ns": 3000}
    ]
    remaining = {400: sample(400, 1, 10, "compiler", started_at_ns=4000)}
    tracker.update(remaining, observed_at_ns=200)
    assert tracker.custody_ancestry_payload(remaining)[0]["ancestors"] == [
        {"pid": 300, "started_at_ns": 3000}
    ]


def test_watched_pids_excludes_unobserved_reparented_process_group_members() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=1000
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=100, started_at_ns=2000
        ),
        102: memory_guard.ProcessSample(
            102, 1, 30, "reparented", pgid=100, started_at_ns=1000
        ),
        200: memory_guard.ProcessSample(
            200, 1, 999_999, "unrelated", pgid=200, started_at_ns=1000
        ),
    }

    assert memory_guard.watched_pids(samples, 100) == {100, 101}


def test_watched_pids_excludes_host_control_plane_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
        200: memory_guard.ProcessSample(200, 1, 20, "unrelated", pgid=200),
    }

    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_plain_claude_control_plane_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "claude",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
        200: memory_guard.ProcessSample(200, 1, 20, "unrelated", pgid=200),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_watched_pids_excludes_claude_code_executable_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/opt/homebrew/bin/claude-code --continue",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_codex_app_and_cli_are_host_control_plane_on_all_platform_shapes() -> None:
    samples = [
        memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
        ),
        memory_guard.ProcessSample(
            101,
            1,
            500_000,
            "/opt/homebrew/bin/codex exec --sandbox danger-full-access",
            pgid=101,
        ),
        memory_guard.ProcessSample(
            102,
            1,
            500_000,
            "/home/adpen/.local/bin/codex --continue",
            pgid=102,
        ),
        memory_guard.ProcessSample(
            103,
            1,
            500_000,
            "node /usr/local/lib/node_modules/@openai/codex/bin/codex.js",
            pgid=103,
        ),
        memory_guard.ProcessSample(
            104,
            1,
            500_000,
            r"C:\Users\adpen\AppData\Roaming\npm\codex.cmd exec",
            pgid=None,
        ),
        memory_guard.ProcessSample(
            105,
            1,
            500_000,
            r"powershell.exe -File C:\Users\adpen\AppData\Roaming\npm\codex.ps1",
            pgid=None,
        ),
    ]

    assert all(memory_guard.is_host_control_plane_process(sample) for sample in samples)


@pytest.mark.parametrize("typed_argv", [False, True])
@pytest.mark.parametrize(
    ("argv", "protected"),
    [
        (
            ("/home/operator/.codex/worktrees/molt/.venv/bin/python", "-c", "pass"),
            False,
        ),
        (
            ("/home/operator/.claude/worktrees/molt/.venv/bin/python", "worker.py"),
            False,
        ),
        ((r"C:\Users\operator\.codex\worktrees\molt\.venv\Scripts\python.exe",), False),
        ((r"C:\Users\operator\.claude\worktrees\molt\target\molt-backend.exe",), False),
        (
            (
                "/usr/bin/python",
                "worker.py",
                "/home/operator/.codex/worktrees/input.py",
            ),
            False,
        ),
        (
            (
                "/usr/bin/python",
                "worker.py",
                "/home/operator/.claude/worktrees/input.py",
            ),
            False,
        ),
        ((r"C:\Users\operator\.codex\tmp\python.exe", "worker.py"), False),
        (("/home/operator/.codex/plugins-other/python", "worker.py"), False),
        (("/home/operator/.claude/plugins-other/python", "worker.py"), False),
        (("/home/operator/.codex/plugins/cache/host/node", "server.js"), True),
        (("/home/operator/.claude/plugins/cache/host/python", "server.py"), True),
        (("/home/operator/.codex/runtimes/node/bin/node", "helper.js"), True),
        (("/home/operator/.claude/runtimes/node/bin/node", "helper.js"), True),
        ((r"C:\Users\operator\.codex\vendor_imports\node.exe", "helper.js"), True),
        (("/bin/zsh", "/home/operator/.codex/shell_snapshots/host.sh"), True),
        (("/bin/zsh", "/home/operator/.claude/shell-snapshots/host.sh"), True),
        # A genuine host executable inside a checkout remains protected.
        (("/home/operator/.codex/worktrees/tool/codex", "app-server"), True),
        (("/home/operator/.claude/worktrees/tool/node_repl", "--stdio"), True),
    ],
)
def test_agent_home_is_not_a_host_identity(
    typed_argv: bool, argv: tuple[str, ...], protected: bool
) -> None:
    sample = memory_guard.ProcessSample(
        200,
        1,
        1,
        " ".join(argv),
        started_at_ns=2000,
        argv=argv if typed_argv else None,
    )
    assert memory_guard.is_host_control_plane_process(sample) is protected


@pytest.mark.parametrize("agent_home", [".codex", ".claude"])
def test_checkout_classification_preserves_instance_and_host_protections(
    agent_home: str,
) -> None:
    sample = memory_guard.ProcessSample
    interpreter = f"/home/operator/{agent_home}/worktrees/molt/.venv/bin/python"
    samples = {
        50: sample(50, 1, 1, "codex app-server", pgid=50, started_at_ns=50),
        100: sample(
            100,
            50,
            1,
            f"{interpreter} tools/memory_guard.py",
            pgid=100,
            started_at_ns=100,
        ),
        200: sample(200, 100, 1, f"{interpreter} -c pass", pgid=200, started_at_ns=200),
        201: sample(201, 50, 1, f"{interpreter} -c pass", pgid=201, started_at_ns=201),
        202: sample(202, 777, 1, f"{interpreter} -c pass", pgid=202, started_at_ns=202),
        300: sample(
            300,
            100,
            1,
            f"/home/operator/{agent_home}/plugins/cache/host/python server.py",
            pgid=300,
            started_at_ns=300,
        ),
        301: sample(
            301,
            100,
            1,
            f"/home/operator/{agent_home}/worktrees/tool/codex app-server",
            pgid=301,
            started_at_ns=301,
        ),
    }
    protected = process_model.protected_process_group_ids(
        samples, self_pid=100, self_pgid=100, owned_pids={200, 300, 301}
    )
    assert protected == {50, 100, 201, 202, 300, 301}
    assert process_model.filter_protected_watched_pids(
        samples, {200, 201, 202, 300, 301}, protected_pgids=protected, current_pid=100
    ) == {200}

    # Mere placement under a checkout cannot exempt an unknown generation from
    # host ancestry protection, even when the caller supplies its numeric PID.
    unbound = {**samples, 200: dataclasses.replace(samples[200], started_at_ns=None)}
    assert 200 in process_model.protected_process_group_ids(
        unbound, self_pid=100, self_pgid=100, owned_pids={200, 300, 301}
    )


@pytest.mark.parametrize(
    "command",
    [
        "/home/operator/.codex/plugins/cache/host/node server.js",
        "/home/operator/.claude/plugins/cache/host/python server.py",
        "/home/operator/.codex/runtimes/node/bin/node helper.js",
        "/home/operator/.claude/worktrees/tool/codex app-server",
    ],
)
def test_retained_host_helper_identity_never_reaches_signal_boundary(
    monkeypatch: pytest.MonkeyPatch, command: str
) -> None:
    sample = memory_guard.ProcessSample(
        300, 999, 1, command, pgid=300, started_at_ns=300
    )
    sent: list[tuple[int, int]] = []

    def forbidden_send(pid: int, signum: int):
        sent.append((pid, signum))
        raise AssertionError("host helper reached signal boundary")

    monkeypatch.setattr(process_custody, "_send_pid_signal_action", forbidden_send)
    action = process_custody._send_pid_signal_if_identity_action(
        sample.pid,
        memory_guard.process_identity(sample),
        signal.SIGTERM,
        sampler=lambda: {sample.pid: sample},
    )
    assert action.result == "skipped_host_control_plane"
    assert sent == []


def test_host_command_cache_reuses_text_but_not_process_or_lineage_verdicts() -> None:
    process_model._cached_host_control_plane_command.cache_clear()
    worker = memory_guard.ProcessSample(100, 1, 20, "/usr/bin/worker", pgid=100)
    assert not memory_guard.is_host_control_plane_process(worker)
    assert not memory_guard.is_host_control_plane_process(
        dataclasses.replace(worker, pid=200, started_at_ns=2)
    )
    assert process_model._cached_host_control_plane_command.cache_info().hits == 1

    host = dataclasses.replace(worker, command="codex app-server", started_at_ns=3)
    child = memory_guard.ProcessSample(101, 100, 20, "/usr/bin/worker", pgid=101)
    assert memory_guard.is_host_control_plane_process(host)
    assert memory_guard.protected_process_group_ids({100: host, 101: child}) == {
        100,
        101,
    }
    # Reparenting changes protection even when every lexical cache entry hits.
    reparented = dataclasses.replace(child, ppid=1)
    assert memory_guard.protected_process_group_ids({100: host, 101: reparented}) == {
        100
    }
    # Reusing a PID for a non-host process must not retain host protection.
    replacement = dataclasses.replace(worker, started_at_ns=4)
    assert (
        memory_guard.protected_process_group_ids({100: replacement, 101: child})
        == set()
    )


@pytest.mark.parametrize(
    ("authority", "replacement", "command"),
    [
        (
            "HOST_CONTROL_PLANE_TOKENS",
            ("supervisor-marker",),
            "worker supervisor-marker",
        ),
        (
            "HOST_CONTROL_PLANE_EXECUTABLE_NAMES",
            frozenset({"worker"}),
            "/usr/bin/worker",
        ),
        ("HOST_CONTROL_PLANE_LAUNCHER_NAMES", frozenset({"worker"}), "worker codex.js"),
        (
            "HOST_CONTROL_PLANE_ARG_EXECUTABLE_NAMES",
            frozenset({"worker.js"}),
            "node worker.js",
        ),
    ],
)
def test_host_command_cache_includes_current_policy(
    monkeypatch: pytest.MonkeyPatch,
    authority: str,
    replacement: tuple[str, ...] | frozenset[str],
    command: str,
) -> None:
    sample = memory_guard.ProcessSample(100, 1, 20, command)
    original = getattr(process_model, authority)
    assert not memory_guard.is_host_control_plane_process(sample)
    monkeypatch.setattr(process_model, authority, replacement)
    assert memory_guard.is_host_control_plane_process(sample)
    monkeypatch.setattr(process_model, authority, original)
    assert not memory_guard.is_host_control_plane_process(sample)


def test_watched_pids_excludes_node_launched_claude_code_group() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "node /opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
            pgid=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
        ),
    }

    assert memory_guard.is_host_control_plane_process(samples[100])
    assert memory_guard.watched_pids(samples, 100) == set()


def test_process_tree_tracker_keeps_reparented_new_session_child_after_seen() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    first = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=101, started_at_ns=101
        ),
        102: memory_guard.ProcessSample(
            102, 101, 30, "grandchild", pgid=102, started_at_ns=102
        ),
    }

    assert tracker.update(first) == {100, 101, 102}

    reparented = {
        101: memory_guard.ProcessSample(
            101, 1, 20, "child", pgid=101, started_at_ns=101
        ),
        102: memory_guard.ProcessSample(
            102, 1, 30, "grandchild", pgid=102, started_at_ns=102
        ),
    }

    assert tracker.update(reparented) == {101, 102}
    violation = memory_guard.find_rss_violation(
        reparented,
        root_pid=100,
        max_rss_kb=25,
        tracker=tracker,
    )
    assert violation == memory_guard.RssViolation(
        pid=102,
        rss_kb=30,
        command="grandchild",
    )


def test_process_tree_tracker_stale_pid_cannot_admit_unrelated_child() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(100, 1, 10, "guard", started_at_ns=1),
        101: memory_guard.ProcessSample(101, 100, 20, "compiler", started_at_ns=2),
    }
    assert tracker.update(initial) == {100, 101}

    # PID 101 has exited. A later unrelated process reports the stale number as
    # its parent; an absent historical PID is not live custody authority.
    reused_parent_edge = {
        100: memory_guard.ProcessSample(100, 1, 10, "guard", started_at_ns=1),
        900: memory_guard.ProcessSample(
            900,
            101,
            500_000,
            "NVIDIA Overlay.exe",
            started_at_ns=99,
        ),
    }
    assert tracker.update(reused_parent_edge) == {100}


def test_process_tree_tracker_identity_ignores_mutable_command_and_group() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(100, 1, 10, "guard", pgid=100, started_at_ns=1),
        200: memory_guard.ProcessSample(
            200, 100, 20, "python worker.py", pgid=100, started_at_ns=2
        ),
    }
    assert tracker.update(initial) == {100, 200}

    execed_and_reparented = {
        100: initial[100],
        200: memory_guard.ProcessSample(
            200,
            1,
            20,
            "/opt/molt-backend --daemon",
            pgid=200,
            started_at_ns=2,
        ),
    }

    assert tracker.update(execed_and_reparented) == {100, 200}
    assert tracker.custody_identities({200}) == {
        200: memory_guard.process_identity(initial[200])
    }


def test_process_tree_tracker_revokes_same_command_pid_reuse() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    initial = {
        100: memory_guard.ProcessSample(100, 1, 10, "guard", pgid=100, started_at_ns=1),
        200: memory_guard.ProcessSample(
            200, 100, 20, "worker", pgid=200, started_at_ns=2
        ),
    }
    tracker.update(initial)
    reused = {
        100: initial[100],
        200: memory_guard.ProcessSample(
            200, 1, 20, "worker", pgid=200, started_at_ns=3
        ),
    }

    assert tracker.update(reused) == {100}
    assert tracker.custody_identities({200}) == {}


def test_process_tree_tracker_weak_reused_parent_cannot_admit_child() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    root = memory_guard.ProcessSample(100, 1, 10, "guard", started_at_ns=100)
    tracker.update({100: root})

    weak_reused_root = memory_guard.ProcessSample(
        100,
        1,
        10,
        "unreadable.exe",
        started_at_ns=None,
    )
    unrelated_child = memory_guard.ProcessSample(
        200,
        100,
        20,
        "worker.exe",
        started_at_ns=200,
    )

    assert tracker.update({100: weak_reused_root, 200: unrelated_child}) == {100}
    assert tracker.custody_identities({100}) == {
        100: memory_guard.process_identity(root)
    }


def test_windows_termination_requires_creation_identity(monkeypatch) -> None:
    sample = process_custody.ProcessSample(
        200,
        100,
        20,
        "worker.exe",
        started_at_ns=None,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

    report = process_custody.terminate_watched_processes(
        100,
        samples={200: sample},
        watched={200},
        expected_identities={200: process_custody.process_identity(sample)},
        root_owned=True,
    )

    assert sent == []
    assert any(
        action.target_id == 200 and action.result == "skipped_ambiguous_identity"
        for action in report.actions
    )


def test_windows_termination_uses_tracker_identity_not_fresh_pid_owner(
    monkeypatch,
) -> None:
    tracker = process_custody.ProcessTreeTracker(100)
    original = process_custody.ProcessSample(
        100,
        1,
        10,
        "guard.exe",
        started_at_ns=1,
    )
    child = process_custody.ProcessSample(
        200,
        100,
        20,
        "rustc.exe",
        started_at_ns=2,
    )
    tracker.update({100: original, 200: child})
    reused = process_custody.ProcessSample(
        200,
        4,
        20,
        "System",
        started_at_ns=3,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

    report = process_custody.terminate_watched_processes(
        100,
        samples={200: reused},
        watched={200},
        tracker=tracker,
        root_owned=True,
    )

    assert sent == []
    assert any(
        action.target_id == 200 and action.result == "skipped_identity_mismatch"
        for action in report.actions
    )


def test_windows_termination_refuses_ambiguous_process_fanout(monkeypatch) -> None:
    root_pid = 100
    samples = {
        pid: process_custody.ProcessSample(
            pid,
            root_pid if pid != root_pid else 1,
            10,
            f"process-{pid}",
            started_at_ns=pid,
        )
        for pid in range(
            root_pid,
            root_pid + process_custody.MAX_TERMINATION_PID_FANOUT + 1,
        )
    }
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        process_custody,
        "_terminate_pid_if_identity_action",
        lambda *_args, **_kwargs: pytest.fail("ambiguous tree must not be signaled"),
    )

    report = process_custody.terminate_watched_processes(
        root_pid,
        samples=samples,
        watched=set(samples),
        root_owned=True,
    )

    assert report.reason == "windows_pid_tree_ambiguous_fanout"
    assert len(report.actions) == 1
    assert report.actions[0].result == "skipped_ambiguous_fanout"


def test_process_tree_tracker_does_not_absorb_root_ambient_process_group() -> None:
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: memory_guard.ProcessSample(100, 50, 10, "pytest current", pgid=500),
        50: memory_guard.ProcessSample(
            50,
            1,
            20,
            "/Applications/Codex.app/Contents/MacOS/Codex app-server",
            pgid=500,
        ),
        200: memory_guard.ProcessSample(
            200,
            50,
            30,
            "/Users/adpena/Projects/molt/.venv/bin/python3 tests/molt_diff.py",
            pgid=200,
        ),
    }

    assert tracker.update(samples) == {100}
    assert tracker.known_pids == {100}
    assert tracker.known_pgids == {100}


def test_process_tree_tracker_does_not_absorb_learned_descendant_process_group_peer() -> (
    None
):
    tracker = memory_guard.ProcessTreeTracker(100)
    samples = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=777, started_at_ns=101
        ),
        200: memory_guard.ProcessSample(
            200, 1, 999, "unrelated", pgid=777, started_at_ns=200
        ),
    }

    assert tracker.update(samples) == {100, 101}
    assert tracker.known_pids == {100, 101}
    assert tracker.known_pgids == {100, 777}


def test_find_rss_violation_ignores_unobserved_reparented_process_group_member() -> (
    None
):
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", pgid=100),
        101: memory_guard.ProcessSample(101, 1, 26_000_000, "reparented", pgid=100),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation is None


def test_terminate_watched_processes_kills_only_root_group_and_tracked_pids(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101, 1, 20, "child", pgid=101, started_at_ns=101
        ),
        102: process_custody.ProcessSample(
            102, 1, 30, "grandchild", pgid=102, started_at_ns=102
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == process_custody.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        if sig == 0 and (pid, process_custody.signal.SIGKILL) in sent_pids:
            raise ProcessLookupError  # A SIGKILLed process leaves the table.
        sent_pids.append((pid, sig))

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=fake_killpg,
        kill=fake_kill,
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101, 102},
        grace=0.001,
    )

    assert (100, process_custody.signal.SIGTERM) in sent_groups
    assert (101, process_custody.signal.SIGTERM) not in sent_groups
    assert (102, process_custody.signal.SIGTERM) not in sent_groups
    assert (101, process_custody.signal.SIGTERM) in sent_pids
    assert (102, process_custody.signal.SIGTERM) in sent_pids
    assert (101, process_custody.signal.SIGKILL) in sent_pids
    assert (102, process_custody.signal.SIGKILL) in sent_pids


def test_terminate_watched_processes_skips_host_control_plane_root_group(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: process_custody.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
            started_at_ns=100,
        ),
        101: process_custody.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
            started_at_ns=101,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=lambda pgid, sig: sent_groups.append((pgid, sig)),
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert sent_groups == []
    assert sent_pids == []


def test_protected_process_groups_include_external_codex_descendant_not_owned_child() -> (
    None
):
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
            started_at_ns=1000,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            10_000,
            "/bin/zsh -l",
            pgid=101,
            started_at_ns=2000,
        ),
        777: memory_guard.ProcessSample(
            777,
            101,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=777,
            started_at_ns=3000,
        ),
        999: memory_guard.ProcessSample(
            999,
            100,
            30_000,
            "python tools/memory_guard.py -- pytest",
            pgid=999,
            started_at_ns=2000,
        ),
        200: memory_guard.ProcessSample(
            200,
            999,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=200,
            started_at_ns=3000,
        ),
    }

    protected = memory_guard.protected_process_group_ids(
        samples,
        self_pid=999,
        self_pgid=999,
    )

    assert 100 in protected
    assert 777 in protected
    assert 999 in protected
    assert 200 not in protected


def test_protected_process_groups_include_external_claude_descendant_not_owned_child() -> (
    None
):
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "claude --dangerously-skip-permissions",
            pgid=100,
            started_at_ns=1000,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            10_000,
            "/bin/zsh -c source /Users/adpena/.claude/shell-snapshots/snapshot-zsh",
            pgid=101,
            started_at_ns=2000,
        ),
        777: memory_guard.ProcessSample(
            777,
            101,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=777,
            started_at_ns=3000,
        ),
        999: memory_guard.ProcessSample(
            999,
            1,
            30_000,
            "python tools/memory_guard.py -- pytest",
            pgid=999,
            started_at_ns=1000,
        ),
        200: memory_guard.ProcessSample(
            200,
            999,
            250_000,
            "/Users/adpena/Projects/molt/target/dev-fast/molt-backend",
            pgid=200,
            started_at_ns=2000,
        ),
    }

    protected = memory_guard.protected_process_group_ids(
        samples,
        self_pid=999,
        self_pgid=999,
    )

    assert 100 in protected
    assert 101 in protected
    assert 777 in protected
    assert 999 in protected
    assert 200 not in protected


def test_terminate_single_process_group_refuses_protected_group(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: memory_guard.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
            started_at_ns=100,
        ),
        101: memory_guard.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend",
            pgid=100,
            started_at_ns=101,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=lambda pgid, sig: sent_groups.append((pgid, sig)),
    )

    assert memory_guard._terminate_single_process_group(100, grace=0.001) is True

    assert sent_groups == []


def test_escalation_pid_signal_revalidates_identity(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    original = memory_guard.ProcessSample(
        101,
        100,
        20,
        "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
        pgid=101,
        started_at_ns=101,
    )
    reused = memory_guard.ProcessSample(
        101,
        1,
        20,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        pgid=101,
        started_at_ns=9101,
    )
    sent_pids: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    action = memory_guard._send_pid_signal_if_identity_action(
        101,
        memory_guard.process_identity(original),
        memory_guard.signal.SIGKILL,
        sampler=lambda: {101: reused},
    )

    assert action.result == "skipped_identity_mismatch"
    assert sent_pids == []


def test_escalation_group_signal_rechecks_protected_group(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    original = memory_guard.ProcessSample(
        101,
        100,
        20,
        "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
        pgid=101,
        started_at_ns=101,
    )
    protected = memory_guard.ProcessSample(
        101,
        1,
        20,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        pgid=101,
        started_at_ns=101,
    )
    sent_groups: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        killpg=lambda pgid, sig: sent_groups.append((pgid, sig)),
    )

    action = memory_guard._send_process_group_signal_if_identities_match_action(
        101,
        {101: memory_guard.process_identity(original)},
        memory_guard.signal.SIGKILL,
        sampler=lambda: {101: protected},
    )

    assert action.result == "skipped_protected_group"
    assert sent_groups == []


def test_sigterm_pid_helper_revalidates_identity_before_signal(monkeypatch) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    original = memory_guard.ProcessSample(
        101,
        100,
        20,
        "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
        pgid=101,
        started_at_ns=101,
    )
    reused = memory_guard.ProcessSample(
        101,
        1,
        20,
        "/Applications/Codex.app/Contents/MacOS/Codex",
        pgid=101,
        started_at_ns=9101,
    )
    sent_pids: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    action = memory_guard._terminate_pid_if_identity_action(
        101,
        memory_guard.process_identity(original),
        sampler=lambda: {101: reused},
        grace=0.001,
    )

    assert action.result == "skipped_identity_mismatch"
    assert sent_pids == []


def test_terminate_watched_processes_revalidates_escaped_pid_before_sigterm(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    observed = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101,
            100,
            20,
            "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
            pgid=777,
            started_at_ns=101,
        ),
    }
    reused = {
        101: process_custody.ProcessSample(
            101,
            1,
            20,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=777,
            started_at_ns=9101,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        killpg=lambda pgid, sig: sent_groups.append((pgid, sig)),
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    report = process_custody.terminate_watched_processes(
        100,
        samples=observed,
        watched={100, 101},
        sampler=lambda: reused,
        grace=0.001,
    )

    assert any(
        action.target_kind == "process"
        and action.target_id == 101
        and action.result == "skipped_identity_mismatch"
        for action in report.actions
    )
    assert sent_groups == []
    assert sent_pids == []


def test_terminate_watched_processes_revalidates_root_group_before_sigterm(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    observed = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101,
            100,
            20,
            "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
            pgid=100,
            started_at_ns=101,
        ),
    }
    protected = {
        100: process_custody.ProcessSample(
            100,
            1,
            500_000,
            "/Applications/Codex.app/Contents/MacOS/Codex",
            pgid=100,
            started_at_ns=100,
        ),
        101: process_custody.ProcessSample(
            101,
            100,
            250_000,
            "/Users/adpena/Projects/molt/target/debug/molt-backend --owned",
            pgid=100,
            started_at_ns=101,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        killpg=lambda pgid, sig: sent_groups.append((pgid, sig)),
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    report = process_custody.terminate_watched_processes(
        100,
        samples=observed,
        watched={100, 101},
        sampler=lambda: protected,
        grace=0.001,
    )

    assert any(
        action.target_kind == "process_group"
        and action.target_id == 100
        and action.result == "skipped_protected_group"
        for action in report.actions
    )
    assert sent_groups == []
    assert sent_pids == []


def test_terminate_watched_processes_filters_protected_escaped_pid(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101,
            100,
            500_000,
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            pgid=777,
            started_at_ns=101,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == process_custody.signal.SIGTERM:
            raise ProcessLookupError

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=fake_killpg,
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert (100, process_custody.signal.SIGTERM) in sent_groups
    assert all(pid != 101 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_killpgs_shared_child_group(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101, 100, 20, "child", pgid=777, started_at_ns=101
        ),
        200: process_custody.ProcessSample(
            200, 1, 999, "unrelated", pgid=777, started_at_ns=200
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == process_custody.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        if sig == 0 and (pid, process_custody.signal.SIGKILL) in sent_pids:
            raise ProcessLookupError  # A SIGKILLed process leaves the table.
        sent_pids.append((pid, sig))

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=fake_killpg,
        kill=fake_kill,
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert (100, process_custody.signal.SIGTERM) in sent_groups
    assert all(pgid != 777 for pgid, _sig in sent_groups)
    assert (101, process_custody.signal.SIGTERM) in sent_pids
    assert (101, process_custody.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_kills_learned_group_peer(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    tracker = process_custody.ProcessTreeTracker(100)
    samples = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101, 100, 20, "child", pgid=777, started_at_ns=101
        ),
        200: process_custody.ProcessSample(
            200, 1, 999, "unrelated", pgid=777, started_at_ns=200
        ),
    }
    assert tracker.update(samples) == {100, 101}
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == process_custody.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        if sig == 0 and (pid, process_custody.signal.SIGKILL) in sent_pids:
            raise ProcessLookupError  # A SIGKILLed process leaves the table.
        sent_pids.append((pid, sig))

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=fake_killpg,
        kill=fake_kill,
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        tracker=tracker,
        grace=0.001,
    )

    assert all(pgid != 777 for pgid, _sig in sent_groups)
    assert (101, process_custody.signal.SIGTERM) in sent_pids
    assert (101, process_custody.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_terminate_watched_processes_never_killpgs_mixed_root_group(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: process_custody.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: process_custody.ProcessSample(
            101, 100, 20, "child", pgid=100, started_at_ns=101
        ),
        200: process_custody.ProcessSample(
            200, 1, 999, "unrelated", pgid=100, started_at_ns=200
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "sample_processes", lambda: samples)

    def fake_killpg(pgid, sig):
        sent_groups.append((pgid, sig))
        if sig == process_custody.signal.SIGTERM:
            raise ProcessLookupError

    def fake_kill(pid, sig):
        if sig == 0 and (pid, process_custody.signal.SIGKILL) in sent_pids:
            raise ProcessLookupError  # A SIGKILLed process leaves the table.
        sent_pids.append((pid, sig))

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        killpg=fake_killpg,
        kill=fake_kill,
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        watched={100, 101},
        grace=0.001,
    )

    assert sent_groups == []
    assert (100, process_custody.signal.SIGKILL) in sent_pids
    assert (101, process_custody.signal.SIGKILL) in sent_pids
    assert all(pid != 200 for pid, _sig in sent_pids)


def test_termination_report_decides_only_the_terminated_tree_groups(
    monkeypatch,
) -> None:
    """HF-368: a termination binds argv for its tree, never the host's rows."""
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    bound: list[int] = []
    dead_groups: set[int] = set()
    rows = {
        100: (1, 100, "python -c 'import time; time.sleep(60)'"),
        101: (100, 100, "python -c 'import time; time.sleep(60)'"),
        # Unrelated host rows: a control-plane app and a process whose parent
        # is unobserved. A host-wide decision protects both groups.
        500: (1, 500, "/Applications/Claude.app/Contents/MacOS/Claude"),
        501: (4242, 501, "/usr/sbin/unrelated-daemon"),
    }

    def table() -> dict[int, process_custody.ProcessSample]:
        def row(pid: int, ppid: int, pgid: int, command: str):
            def bind() -> tuple[str, tuple[str, ...], str]:
                bound.append(pid)
                return command, tuple(command.split()), "full"

            return process_model.native_process_sample(
                pid=pid,
                ppid=ppid,
                rss_kb=1,
                pgid=pgid,
                elapsed_sec=1,
                started_at_ns=pid,
                bind_command=bind,
            )

        return {
            pid: row(pid, ppid, pgid, command)
            for pid, (ppid, pgid, command) in rows.items()
            if pgid not in dead_groups
        }

    sent_groups: list[tuple[int, int]] = []

    def fake_killpg(pgid: int, sig: int) -> None:
        if pgid in dead_groups:
            raise ProcessLookupError
        if sig:
            sent_groups.append((pgid, sig))
            dead_groups.add(pgid)

    def fake_kill(pid: int, sig: int) -> None:
        if rows[pid][1] in dead_groups:
            raise ProcessLookupError
        pytest.fail("the fully owned group closes by its group signal")

    monkeypatch.setattr(process_model, "sample_processes", table)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        killpg=fake_killpg,
        kill=fake_kill,
    )

    report = process_custody.terminate_watched_processes(
        100,
        samples=table(),
        watched={100, 101},
        grace=0.001,
        root_owned=True,
    )

    assert (100, process_custody.signal.SIGTERM) in sent_groups
    assert report.remaining_pids == () and report.remaining_pgids == ()
    assert report.protected_pgids == ()
    assert not {500, 501} & set(bound)


def test_terminate_watched_processes_never_kills_host_control_plane_group(
    monkeypatch,
) -> None:
    if process_custody.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    samples = {
        100: process_custody.ProcessSample(
            100,
            27404,
            20,
            "uv run python tests/molt_diff.py --jobs 1",
            pgid=700,
            started_at_ns=100,
        ),
        27404: process_custody.ProcessSample(
            27404,
            27335,
            500_000,
            "/Applications/Codex.app/Contents/Resources/codex app-server",
            pgid=700,
            started_at_ns=50,
        ),
    }
    sent_groups: list[tuple[int, int]] = []
    sent_pids: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgrp=lambda: 999,
        getpid=lambda: 999,
        killpg=lambda pgid, sig: sent_groups.append((pgid, sig)),
        kill=lambda pid, sig: sent_pids.append((pid, sig)),
    )

    process_custody.terminate_watched_processes(
        100,
        samples=samples,
        watched={100},
        grace=0.001,
    )

    assert sent_groups == []
    assert sent_pids == []


def test_find_rss_violation_ignores_unrelated_processes() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", started_at_ns=1000),
        101: memory_guard.ProcessSample(
            101, 100, 26_000_000, "child", started_at_ns=2000
        ),
        200: memory_guard.ProcessSample(
            200, 1, 40_000_000, "unrelated", started_at_ns=1000
        ),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation == memory_guard.RssViolation(
        pid=101,
        rss_kb=26_000_000,
        command="child",
    )


def test_find_rss_violation_returns_highest_descendant() -> None:
    samples = {
        100: memory_guard.ProcessSample(100, 1, 10, "root", started_at_ns=1000),
        101: memory_guard.ProcessSample(
            101, 100, 28_000_000, "smaller", started_at_ns=2000
        ),
        102: memory_guard.ProcessSample(
            102, 100, 29_000_000, "larger", started_at_ns=2000
        ),
    }

    violation = memory_guard.find_rss_violation(
        samples, root_pid=100, max_rss_kb=25_000_000
    )

    assert violation is not None
    assert violation.pid == 102
    assert violation.rss_gb == pytest.approx(29_000_000 / (1024 * 1024))


def test_find_rss_violation_catches_aggregate_process_tree_rss() -> None:
    samples = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=1000
        ),
        101: memory_guard.ProcessSample(
            101, 100, 15_000_000, "child-a", pgid=100, started_at_ns=2000
        ),
        102: memory_guard.ProcessSample(
            102, 100, 15_000_000, "child-b", pgid=100, started_at_ns=2000
        ),
        200: memory_guard.ProcessSample(
            200, 1, 40_000_000, "unrelated", pgid=200, started_at_ns=1000
        ),
    }

    violation = memory_guard.find_rss_violation(
        samples,
        root_pid=100,
        max_rss_kb=25_000_000,
        max_total_rss_kb=25_000_000,
    )

    assert violation == memory_guard.RssViolation(
        pid=100,
        rss_kb=30_000_010,
        command="process tree aggregate",
        scope="process_tree",
    )


def test_max_rss_gb_accepts_high_workstation_limits() -> None:
    assert memory_guard.max_rss_kb_from_gb(96) == 96 * 1024 * 1024


def test_max_rss_gb_must_leave_margin_below_hard_cap() -> None:
    with pytest.raises(ValueError, match="below 112"):
        memory_guard.max_rss_kb_from_gb(112)


def test_max_global_rss_gb_must_leave_workstation_margin() -> None:
    assert memory_guard.max_global_rss_kb_from_gb(128) == 128 * 1024 * 1024
    with pytest.raises(ValueError, match="below 4096"):
        memory_guard.max_global_rss_kb_from_gb(4096)


def test_memory_guard_defaults_adapt_to_live_memory_budget() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_MEMORY_TOTAL_GB": "128",
            "MOLT_BENCH_MEMORY_AVAILABLE_GB": "96",
        },
    )

    assert budget.reserve_gb == pytest.approx(7.68)
    assert budget.max_process_rss_gb == pytest.approx(46.262016)
    assert budget.max_total_rss_gb == pytest.approx(51.40224)
    assert budget.max_global_rss_gb == pytest.approx(85.6704)
    assert memory_guard.DEFAULT_POLL_INTERVAL_SEC == 0.10


def test_adaptive_budget_samples_host_once(monkeypatch) -> None:
    observations = []

    def probe():
        observations.append(None)
        return 12 * 1024**3, 11 * 1024**3

    monkeypatch.setattr(host_capacity, "_probe_memory_bytes", probe)
    budget = memory_guard.adaptive_memory_budget("MOLT_TEST", {})
    assert observations == [None]
    assert budget.physical_gb == 12
    assert budget.available_gb == 11
    assert budget.max_total_rss_gb == pytest.approx(5.82)


@pytest.mark.parametrize("tree_gb,process_gb", [(2, 1.8), (9, 8.1), (50, 12)])
def test_unobserved_host_still_obeys_the_delegated_tree(
    monkeypatch, tree_gb, process_gb
) -> None:
    monkeypatch.setattr(host_capacity, "_probe_memory_bytes", lambda: (None, None))
    env = {host_capacity.MEMORY_SLICE_ENV: str(tree_gb * 1024**3)}
    budget = memory_guard.adaptive_memory_budget("MOLT_TEST", env)
    assert budget.source == "fallback"
    assert budget.max_total_rss_gb == tree_gb
    assert budget.max_process_rss_gb == pytest.approx(process_gb)
    assert budget.max_global_rss_gb >= budget.max_total_rss_gb


def test_zero_available_memory_remains_observed_pressure(monkeypatch) -> None:
    monkeypatch.setattr(host_capacity, "_probe_memory_bytes", lambda: (12 * 1024**3, 0))
    budget = memory_guard.adaptive_memory_budget("MOLT_TEST", {})
    assert budget.available_gb == 0
    assert budget.source == "available"
    assert budget.max_global_rss_gb == 0.25


def test_adaptive_budget_scales_up_and_down_with_live_available_memory() -> None:
    high = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_MEMORY_TOTAL_GB": "128",
            "MOLT_BENCH_MEMORY_AVAILABLE_GB": "120",
        },
    )
    pressured = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_MEMORY_TOTAL_GB": "128",
            "MOLT_BENCH_MEMORY_AVAILABLE_GB": "32",
        },
    )

    assert high.reserve_gb == pytest.approx(7.68)
    assert high.max_global_rss_gb == pytest.approx(108.9504)
    assert high.max_total_rss_gb == pytest.approx(65.37024)
    assert high.max_process_rss_gb == pytest.approx(58.833216)
    assert pressured.reserve_gb == pytest.approx(high.reserve_gb)
    assert pressured.max_global_rss_gb == pytest.approx(23.5904)
    assert pressured.max_total_rss_gb == pytest.approx(14.15424)
    assert pressured.max_process_rss_gb == pytest.approx(12.738816)
    assert high.max_global_rss_gb > pressured.max_global_rss_gb
    assert high.available_gb - high.max_global_rss_gb > high.reserve_gb
    assert pressured.available_gb - pressured.max_global_rss_gb > pressured.reserve_gb


def test_adaptive_budget_accounts_guarded_tree_rss_without_self_tightening() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_MEMORY_TOTAL_GB": "128",
            "MOLT_BENCH_MEMORY_AVAILABLE_GB": "46",
        },
        accounted_rss_kb=50 * 1024 * 1024,
    )

    assert budget.accounted_rss_gb == pytest.approx(50.0)
    assert budget.available_gb == pytest.approx(96.0)
    assert budget.max_process_rss_gb == pytest.approx(46.262016)
    assert budget.max_total_rss_gb == pytest.approx(51.40224)
    assert budget.max_global_rss_gb == pytest.approx(85.6704)


def test_adaptive_budget_clamps_large_hosts_below_rss_conversion_cap() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_MEMORY_TOTAL_GB": "512",
            "MOLT_BENCH_MEMORY_AVAILABLE_GB": "500",
        },
    )

    assert budget.reserve_gb == pytest.approx(12.0)
    assert budget.max_global_rss_gb == pytest.approx(473.36)
    assert budget.max_total_rss_gb == pytest.approx(
        memory_guard.DEFAULT_HARD_MAX_RSS_GB - 0.001
    )
    assert budget.max_process_rss_gb == pytest.approx(100.7991)
    assert memory_guard.max_rss_kb_from_gb(budget.max_total_rss_gb) > 0
    assert memory_guard.max_rss_kb_from_gb(budget.max_process_rss_gb) > 0


def test_guard_exports_its_tree_cap_as_the_memory_slice() -> None:
    from tools.memory_guard_core.process_custody import (
        _inject_guard_memory_contract_env,
    )

    env: dict[str, str] = {}
    _inject_guard_memory_contract_env(
        env,
        max_rss_kb=4 * 1024 * 1024,
        child_rlimit_kb=None,
        max_total_rss_kb=10 * 1024 * 1024,
    )
    assert env[host_capacity.MEMORY_SLICE_ENV] == str(10 * 1024**3)
    # A nested guard narrows its parent's slice and never widens it.
    _inject_guard_memory_contract_env(
        env,
        max_rss_kb=4 * 1024 * 1024,
        child_rlimit_kb=None,
        max_total_rss_kb=20 * 1024 * 1024,
    )
    assert env[host_capacity.MEMORY_SLICE_ENV] == str(10 * 1024**3)
    _inject_guard_memory_contract_env(
        env,
        max_rss_kb=4 * 1024 * 1024,
        child_rlimit_kb=None,
        max_total_rss_kb=6 * 1024 * 1024,
    )
    assert env[host_capacity.MEMORY_SLICE_ENV] == str(6 * 1024**3)


def test_available_memory_bytes_reads_the_host_capacity_authority(
    monkeypatch,
) -> None:
    # Host-specific page accounting belongs to test_host_capacity. This
    # consumer forwards the exact request and reads the available field.
    requests = []

    def capacity(environ=None):
        requests.append(environ)
        return host_capacity.MemoryCapacity(91 * 4096, 17 * 4096, 5 * 4096)

    monkeypatch.setattr(
        host_capacity,
        "memory_capacity",
        current_thread_only(capacity, host_capacity.memory_capacity),
    )
    environment = {}
    assert memory_guard.available_memory_bytes(environ=environment) == 17 * 4096
    assert len(requests) == 1 and requests[0] is environment


def test_resolve_memory_limits_refreshes_dynamic_caps() -> None:
    seen_accounted: list[int] = []

    def provider(accounted_rss_kb: int) -> memory_guard.AdaptiveMemoryBudget:
        seen_accounted.append(accounted_rss_kb)
        return memory_guard.AdaptiveMemoryBudget(
            max_process_rss_gb=4.0,
            max_total_rss_gb=6.0,
            max_global_rss_gb=8.0,
            reserve_gb=1.0,
            physical_gb=16.0,
            available_gb=12.0,
            source="test",
            accounted_rss_gb=accounted_rss_kb / (1024 * 1024),
        )

    limits = memory_guard.resolve_memory_limits(
        max_process_rss_kb=2 * 1024 * 1024,
        max_total_rss_kb=3 * 1024 * 1024,
        max_global_rss_kb=5 * 1024 * 1024,
        adaptive_budget_provider=provider,
        dynamic_process_rss=True,
        dynamic_total_rss=True,
        dynamic_global_rss=False,
        accounted_rss_kb=12345,
    )

    assert seen_accounted == [12345]
    assert limits.max_process_rss_kb == 4 * 1024 * 1024
    assert limits.max_total_rss_kb == 6 * 1024 * 1024
    assert limits.max_global_rss_kb == 5 * 1024 * 1024


def test_memory_guard_adaptive_defaults_do_not_starve_small_hosts() -> None:
    budget = memory_guard.adaptive_memory_budget(
        "MOLT_BENCH",
        {
            "MOLT_BENCH_MEMORY_TOTAL_GB": "7",
            "MOLT_BENCH_MEMORY_AVAILABLE_GB": "5",
        },
    )

    assert budget.reserve_gb == pytest.approx(1.0)
    assert budget.max_process_rss_gb == pytest.approx(2.0952)
    assert budget.max_total_rss_gb == pytest.approx(2.328)
    assert budget.max_global_rss_gb == pytest.approx(3.88)


def test_default_child_rlimit_tracks_process_rss_budget() -> None:
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
    ) == pytest.approx(2.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=2.0,
        max_total_rss_gb=3.0,
        max_global_rss_gb=4.0,
    ) == pytest.approx(2.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=46.0,
        max_total_rss_gb=51.0,
        max_global_rss_gb=85.0,
    ) == pytest.approx(46.0)
    assert memory_guard.default_child_rlimit_gb(
        max_process_rss_gb=46.0,
        max_total_rss_gb=51.0,
    ) == pytest.approx(46.0)


def test_child_rss_backstop_preserves_sparse_virtual_address_reservations(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls: list[tuple[int, tuple[int, int]]] = []
    fake_resource = types.ModuleType("resource")
    rlimit_as, rlimit_data, rlimit_rss = 1, 2, 3
    setattr(fake_resource, "RLIM_INFINITY", -1)
    setattr(fake_resource, "RLIMIT_AS", rlimit_as)
    setattr(fake_resource, "RLIMIT_DATA", rlimit_data)
    setattr(fake_resource, "RLIMIT_RSS", rlimit_rss)
    setattr(fake_resource, "getrlimit", lambda _resource: (-1, -1))
    setattr(
        fake_resource,
        "setrlimit",
        lambda resource, limits: calls.append((resource, limits)),
    )
    monkeypatch.setitem(sys.modules, "resource", fake_resource)
    install_module_view(
        monkeypatch,
        "sys",
        sys,
        memory_guard,
        platform="linux",
    )

    memory_guard._apply_child_resource_limit(1024)

    assert [resource for resource, _limits in calls] == [rlimit_rss]
    assert rlimit_as not in [resource for resource, _limits in calls]
    assert rlimit_data not in [resource for resource, _limits in calls]
    assert all(limits == (1024 * 1024, 1024 * 1024) for _resource, limits in calls)


def test_run_command_passes_through_success() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.violation is None
    assert result.peak is not None
    assert result.peak.rss_kb > 0
    assert result.stdout == "ok\n"
    assert result.elapsed_s is not None
    assert result.elapsed_s > 0


def test_run_guarded_binary_capture_preserves_bytes() -> None:
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                "data = sys.stdin.buffer.read(); "
                "sys.stdout.buffer.write(data[::-1]); "
                "sys.stderr.buffer.write(b'err:' + data[:2])"
            ),
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        input=b"\xffabc",
        text=False,
    )

    assert result.returncode == 0
    assert result.stdout == b"cba\xff"
    assert result.stderr == b"err:\xffa"


def test_run_guarded_external_evidence_is_full_bounded_and_immutable(
    tmp_path: Path,
) -> None:
    stdout_path = tmp_path / "stdout.log"
    stderr_path = tmp_path / "stderr.log"
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import sys; "
                "sys.stdout.write('x' * 1000000 + 'stdout-end\\n'); "
                "sys.stderr.write('y' * 1000000 + 'stderr-end\\n')"
            ),
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        stdout_capture_path=stdout_path,
        stderr_capture_path=stderr_path,
        capture_tail_bytes=64,
    )

    assert result.returncode == 0
    assert result.stdout.endswith(f"stdout-end{os.linesep}")
    assert result.stderr.endswith(f"stderr-end{os.linesep}")
    assert len(result.stdout.encode()) <= 64
    assert len(result.stderr.encode()) <= 64
    assert stdout_path.stat().st_size > 1_000_000
    assert stderr_path.stat().st_size > 1_000_000
    with pytest.raises(FileExistsError):
        memory_guard.run_guarded(
            [sys.executable, "-c", "print('must not overwrite evidence')"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            stdout_capture_path=stdout_path,
            stderr_capture_path=stderr_path,
            capture_tail_bytes=64,
        )


def test_run_guarded_external_evidence_contract_rejects_ambiguous_paths(
    tmp_path: Path,
) -> None:
    shared = tmp_path / "shared.log"
    with pytest.raises(ValueError, match="must be distinct"):
        memory_guard.run_guarded(
            [sys.executable, "-c", "print('not launched')"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            stdout_capture_path=shared,
            stderr_capture_path=shared,
            capture_tail_bytes=64,
        )
    with pytest.raises(ValueError, match="requires external capture paths"):
        memory_guard.run_guarded(
            [sys.executable, "-c", "print('not launched')"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            capture_tail_bytes=64,
        )


_SIGTERM_CHILD = (
    """
import pathlib, signal, sys, time
receipt, ready, mode = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3]
pathlib.Path(sys.argv[4]).write_text(str(__import__("os").getpid()))
def drain(signum, frame):
    time.sleep(0.5)
    receipt.write_text("drained")
    sys.exit(3)
signal.signal(signal.SIGTERM, drain if mode == "drain" else signal.SIG_IGN)
ready.write_text("ready")
"""
    + HUNG_CHILD_SOURCE
)


def _signal_guard_parent(tmp_path: Path, mode: str) -> tuple[int, float, Path, int]:
    receipt, ready, pid_file = (tmp_path / n for n in ("receipt", "ready", "pid"))
    guard = subprocess.Popen(
        [
            sys.executable,
            str(memory_guard.ROOT / "tools" / "memory_guard.py"),
            "--max-rss-gb",
            "4",
            "--max-total-rss-gb",
            "8",
            "--",
            sys.executable,
            "-c",
            _SIGTERM_CHILD,
            str(receipt),
            str(ready),
            mode,
            str(pid_file),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + 60
    while not ready.exists():
        assert guard.poll() is None and time.monotonic() < deadline
        time.sleep(0.02)
    started = time.monotonic()
    guard.send_signal(signal.SIGTERM)
    returncode = guard.wait(timeout=60)
    return (
        returncode,
        time.monotonic() - started,
        receipt,
        int(pid_file.read_text(encoding="utf-8")),
    )


@pytest.mark.skipif(os.name != "posix", reason="POSIX signal grace")
def test_operator_signal_lets_the_guarded_tree_write_its_receipts(
    tmp_path: Path,
) -> None:
    # HF-232: a suite under the guard gets SIGTERM and a bounded grace to write
    # its summary; the guard still reports the operator's signal.
    returncode, elapsed, receipt, child_pid = _signal_guard_parent(tmp_path, "drain")
    assert returncode == 128 + signal.SIGTERM
    assert receipt.read_text(encoding="utf-8") == "drained"
    assert elapsed < memory_guard.OPERATOR_SIGNAL_GRACE_S + 10, elapsed
    assert child_pid not in memory_guard.sample_processes()


@pytest.mark.skipif(os.name != "posix", reason="POSIX signal grace")
def test_operator_signal_grace_is_bounded_for_a_child_that_ignores_it(
    tmp_path: Path,
) -> None:
    returncode, elapsed, receipt, child_pid = _signal_guard_parent(tmp_path, "ignore")
    assert returncode == 128 + signal.SIGTERM
    assert not receipt.exists()
    assert elapsed < memory_guard.OPERATOR_SIGNAL_GRACE_S + 10, elapsed
    assert child_pid not in memory_guard.sample_processes()


def test_run_guarded_interrupt_during_sampling_terminates_child_tree(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def interrupting_sampler():
        raise KeyboardInterrupt

    monkeypatch.setattr(memory_guard._win_job, "create_kill_on_close_job", lambda: None)
    result = memory_guard.run_guarded(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=interrupting_sampler,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert "memory_guard: interrupted" in result.stderr
    assert result.elapsed_s < 10


def test_run_guarded_interrupt_reuses_last_successful_descendant_snapshot(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 4242
    child_pid = 4243
    grandchild_pid = 4244
    samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid, 1, 64, "root", started_at_ns=root_pid
        ),
        child_pid: memory_guard.ProcessSample(
            child_pid, root_pid, 64, "child", started_at_ns=child_pid
        ),
        grandchild_pid: memory_guard.ProcessSample(
            grandchild_pid,
            child_pid,
            64,
            "grandchild",
            started_at_ns=grandchild_pid,
        ),
    }

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

    processes: list[FakePopen] = []

    def fake_popen(command: list[str], **kwargs: object) -> FakePopen:
        proc = FakePopen(command, **kwargs)
        processes.append(proc)
        return proc

    sample_calls = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_calls
        sample_calls += 1
        if sample_calls > 1:
            raise KeyboardInterrupt
        return samples

    terminations: list[dict[str, object]] = []

    def recording_terminate(
        root_pid: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        terminations.append({"root_pid": root_pid, **kwargs})
        processes[0].returncode = -15
        watched = kwargs.get("watched")
        return _guard_termination_report(
            reason=str(kwargs.get("reason", "test_cleanup")),
            root_pid=root_pid,
            root_pgid=root_pid,
            watched_pids=tuple(sorted(watched)) if isinstance(watched, set) else (),
        )

    _patch_guard_popen_without_windows_job(monkeypatch, fake_popen)
    monkeypatch.setattr(
        memory_guard, "terminate_watched_processes", recording_terminate
    )

    result = memory_guard.run_guarded(
        ["fake-python", "-c", "sleep"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=sampler,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert "memory_guard: interrupted" in result.stderr
    assert any(
        {root_pid, child_pid, grandchild_pid}.issubset(call.get("watched", set()))
        for call in terminations
    )
    assert all(call.get("root_owned") is True for call in terminations)


def test_run_guarded_sampler_failure_cleans_then_reraises(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 5252
    child_pid = 5253
    samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid, 1, 64, "root", started_at_ns=root_pid
        ),
        child_pid: memory_guard.ProcessSample(
            child_pid, root_pid, 64, "child", started_at_ns=child_pid
        ),
    }

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

    processes: list[FakePopen] = []

    def fake_popen(command: list[str], **kwargs: object) -> FakePopen:
        proc = FakePopen(command, **kwargs)
        processes.append(proc)
        return proc

    sample_calls = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_calls
        sample_calls += 1
        if sample_calls > 1:
            raise RuntimeError("sampler failed after custody")
        return samples

    terminations: list[dict[str, object]] = []

    def recording_terminate(
        root_pid: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        terminations.append({"root_pid": root_pid, **kwargs})
        processes[0].returncode = -15
        watched = kwargs.get("watched")
        return _guard_termination_report(
            reason=str(kwargs.get("reason", "test_cleanup")),
            root_pid=root_pid,
            root_pgid=root_pid,
            watched_pids=tuple(sorted(watched)) if isinstance(watched, set) else (),
        )

    _patch_guard_popen_without_windows_job(monkeypatch, fake_popen)
    monkeypatch.setattr(
        memory_guard, "terminate_watched_processes", recording_terminate
    )

    with pytest.raises(RuntimeError, match="sampler failed after custody"):
        memory_guard.run_guarded(
            ["fake-python", "-c", "sleep"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=sampler,
        )

    assert any(
        {root_pid, child_pid}.issubset(call.get("watched", set()))
        for call in terminations
    )
    assert all(call.get("root_owned") is True for call in terminations)


def test_run_guarded_finalizer_survives_owned_reaper_failure(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    # A failed sole reaper makes the handle's poll/wait raise. The guard must
    # surface that failure once and still finish custody: the finalizer reads
    # the published exit, so it cannot re-raise and skip handler restoration.
    reaper_failure = RuntimeError("owned child reaper failed")

    class FakePopen:
        pid = 6767
        stdin = None
        returncode: int | None = None
        _handle = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            raise reaper_failure

        def wait(self, timeout: float | None = None) -> int:
            raise reaper_failure

        def terminate(self) -> None:
            raise reaper_failure

        def kill(self) -> None:
            raise reaper_failure

    reasons: list[object] = []

    def recording_terminate(
        root_pid: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        reasons.append(kwargs.get("reason"))
        return _guard_termination_report(
            reason=str(kwargs.get("reason")), root_pid=root_pid
        )

    _patch_guard_popen_without_windows_job(monkeypatch, FakePopen)
    monkeypatch.setattr(
        memory_guard, "terminate_watched_processes", recording_terminate
    )
    handler_before = signal.getsignal(signal.SIGTERM)

    with pytest.raises(RuntimeError) as caught:
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=lambda: {},
        )

    assert caught.value is reaper_failure
    assert reasons == ["run_guarded_finalizer"]
    assert signal.getsignal(signal.SIGTERM) == handler_before


def test_run_guarded_windows_snapshot_timeout_preserves_healthy_child(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 6060

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            assert timeout is not None
            self.returncode = 0
            return 0

        def terminate(self) -> None:
            raise AssertionError("a telemetry timeout must not terminate the child")

        def kill(self) -> None:
            raise AssertionError("a telemetry timeout must not kill the child")

    process = FakePopen()
    _patch_guard_popen_without_windows_job(monkeypatch, lambda *_a, **_kw: process)

    def timed_out_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        raise memory_guard.WindowsProcessSnapshotTimeout("snapshot deadline")

    result = memory_guard.run_guarded(
        ["fake-python"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=timed_out_sampler,
    )

    assert result.returncode == 0
    assert result.violation is None
    assert result.sampling_telemetry is not None
    # The first observation (orphan baseline and first enforcement sample)
    # and post-exit orphan custody both degrade through the same authority
    # without rewriting the healthy child result. The child exits during the
    # first bounded wait, so no second live sample is taken.
    assert result.sampling_telemetry.attempts == 2
    assert result.sampling_telemetry.successes == 0
    assert result.sampling_telemetry.transient_failures == 2
    assert not result.sampling_telemetry.enforcement_complete
    assert "RSS enforcement was unobserved" in result.stderr


def test_run_guarded_observed_rss_violation_remains_fail_closed_after_timeout(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 6161

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(["fake-python"], timeout)
            return self.returncode

    process = FakePopen()
    _patch_guard_popen_without_windows_job(monkeypatch, lambda *_a, **_kw: process)
    sample_calls = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_calls
        sample_calls += 1
        if sample_calls == 1:
            raise memory_guard.WindowsProcessSnapshotTimeout("snapshot deadline")
        return {
            root_pid: memory_guard.ProcessSample(
                root_pid,
                1,
                1_001,
                "fake-python",
                started_at_ns=root_pid,
            )
        }

    def terminate(root: int, **kwargs: object) -> memory_guard.GuardTerminationReport:
        process.returncode = -15
        return _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            watched_pids=(root,),
        )

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", terminate)

    result = memory_guard.run_guarded(
        ["fake-python"],
        max_rss_kb=1_000,
        poll_interval=0.01,
        sampler=sampler,
        cleanup_orphans=False,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.pid == root_pid
    assert result.sampling_telemetry is not None
    assert result.sampling_telemetry.transient_failures == 1
    assert result.sampling_telemetry.successes == 1


def test_run_guarded_binds_root_identity_before_first_sampler(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
    tmp_path: Path,
) -> None:
    root_pid = 6262

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = 123

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

    process = FakePopen(["fake-python"])
    reports: list[dict[str, object]] = []

    def fake_terminate(root: int, **kwargs: object):
        reports.append({"root": root, **kwargs})
        process.returncode = -15
        return _guard_termination_report(reason="sampler_failure", root_pid=root)

    _patch_guard_popen_without_windows_job(monkeypatch, lambda *_a, **_kw: process)
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    monkeypatch.setattr(
        memory_guard,
        "windows_process_handle_started_at_ns",
        lambda handle: 987_654_300 if handle == 123 else None,
    )
    monkeypatch.setattr(memory_guard, "terminate_watched_processes", fake_terminate)

    def first_snapshot():
        (marker,) = (tmp_path / "active").glob("guard-*.json")
        payload = json.loads(marker.read_text(encoding="utf-8"))
        assert payload["child_launch_state"] == "recorded"
        assert payload["child_process"]["started_at_ns"] == 987_654_300
        raise RuntimeError("first snapshot failed")

    with pytest.raises(RuntimeError, match="first snapshot failed"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=first_snapshot,
            env={**os.environ, "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path)},
        )

    tracker = reports[0]["tracker"]
    assert isinstance(tracker, memory_guard.ProcessTreeTracker)
    assert tracker.custody_identities({root_pid}) == {
        root_pid: memory_guard.ProcessIdentity(987_654_300)
    }


def test_run_guarded_does_not_spawn_without_durable_launch_boundary(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    update = memory_guard.update_active_guard_marker

    def fail_pending(path, token, *, status, **fields):
        if status == "spawn_pending":
            return False
        return update(path, token, status=status, **fields)

    spawned = []

    def unexpected_spawn(*args, **kwargs):
        spawned.append(True)
        raise AssertionError("child started without marker custody")

    _patch_guard_popen_without_windows_job(monkeypatch, unexpected_spawn)
    monkeypatch.setattr(memory_guard, "update_active_guard_marker", fail_pending)
    with pytest.raises(RuntimeError, match="child launch boundary"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            env={**os.environ, "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path)},
        )
    assert spawned == []
    (marker,) = (tmp_path / "active").glob("guard-*.json")
    payload = json.loads(marker.read_text(encoding="utf-8"))
    assert payload["child_launch_state"] == "not_started"
    assert payload["child_process"] is None


def test_run_guarded_persistent_sampler_failure_reaps_owned_child_handle(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 6363

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None
        terminate_calls = 0
        kill_calls = 0

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def terminate(self) -> None:
            self.terminate_calls += 1
            self.returncode = 0
            raise ProcessLookupError

        def kill(self) -> None:
            self.kill_calls += 1
            self.returncode = -9

    process = FakePopen(["fake-python"])
    root_sample = memory_guard.ProcessSample(
        root_pid,
        1,
        64,
        "fake-python",
        started_at_ns=root_pid,
    )
    sample_count = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_count
        sample_count += 1
        if sample_count == 1:
            return {root_pid: root_sample}
        raise RuntimeError("persistent snapshot failure")

    _patch_guard_popen_without_windows_job(monkeypatch, lambda *_a, **_kw: process)
    monkeypatch.setattr(
        memory_guard,
        "terminate_watched_processes",
        lambda root, **kwargs: _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            actions=(
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=root,
                    signal=None,
                    signal_name=None,
                    result="skipped_sampler_failure",
                ),
            ),
        ),
    )

    with pytest.raises(RuntimeError, match="persistent snapshot failure"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=sampler,
        )

    assert process.terminate_calls == 1
    assert process.kill_calls == 0
    assert process.returncode == 0


def test_run_guarded_post_loop_sampler_failure_reaps_only_owned_child_handle(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 6464
    unrelated_pid = 7474

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None
        terminate_calls = 0
        kill_calls = 0

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def terminate(self) -> None:
            self.terminate_calls += 1

        def kill(self) -> None:
            self.kill_calls += 1
            self.returncode = -9

    process = FakePopen(["fake-python"])
    samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid,
            1,
            2_000_000,
            "fake-python",
            started_at_ns=root_pid,
        ),
        unrelated_pid: memory_guard.ProcessSample(
            unrelated_pid,
            1,
            64,
            "unrelated",
            started_at_ns=unrelated_pid,
        ),
    }
    sample_count = 0

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        nonlocal sample_count
        sample_count += 1
        if sample_count == 1:
            return samples
        raise RuntimeError("post-loop snapshot failure")

    watched_calls: list[set[int]] = []

    def record_termination(
        root: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        watched = set(kwargs.get("watched", set()))
        watched_calls.append(watched)
        return _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            watched_pids=tuple(sorted(watched)),
            actions=(
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=root,
                    signal=memory_guard.signal.SIGTERM,
                    signal_name="SIGTERM",
                    result="still_live",
                ),
            ),
        )

    _patch_guard_popen_without_windows_job(monkeypatch, lambda *_a, **_kw: process)
    monkeypatch.setattr(memory_guard, "terminate_watched_processes", record_termination)

    with pytest.raises(RuntimeError, match="post-loop snapshot failure"):
        memory_guard.run_guarded(
            ["fake-python"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            sampler=sampler,
            cleanup_orphans=False,
        )

    assert process.terminate_calls == 1
    assert process.kill_calls == 1
    assert process.returncode == -9
    assert watched_calls
    assert all(root_pid in watched for watched in watched_calls)
    assert all(unrelated_pid not in watched for watched in watched_calls)


def test_run_guarded_weak_sampler_reaps_only_owned_child_handle(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    root_pid = 6565
    unrelated_pid = 7575

    class FakePopen:
        pid = root_pid
        stdin = None
        returncode: int | None = None
        _handle = None
        terminate_calls = 0
        kill_calls = 0

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                raise subprocess.TimeoutExpired(self.command, timeout)
            return self.returncode

        def terminate(self) -> None:
            self.terminate_calls += 1

        def kill(self) -> None:
            self.kill_calls += 1
            self.returncode = -9

    process = FakePopen(["fake-python"])
    weak_samples = {
        root_pid: memory_guard.ProcessSample(
            root_pid,
            1,
            2_000_000,
            "fake-python",
            started_at_ns=None,
        ),
        unrelated_pid: memory_guard.ProcessSample(
            unrelated_pid,
            1,
            64,
            "unrelated",
            started_at_ns=None,
        ),
    }
    watched_calls: list[set[int]] = []

    def record_termination(
        root: int, **kwargs: object
    ) -> memory_guard.GuardTerminationReport:
        watched = set(kwargs.get("watched", set()))
        watched_calls.append(watched)
        return _guard_termination_report(
            reason=str(kwargs["reason"]),
            root_pid=root,
            watched_pids=tuple(sorted(watched)),
            actions=(
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=root,
                    signal=None,
                    signal_name=None,
                    result="skipped_missing_identity",
                ),
            ),
        )

    _patch_guard_popen_without_windows_job(monkeypatch, lambda *_a, **_kw: process)
    monkeypatch.setattr(memory_guard, "terminate_watched_processes", record_termination)

    result = memory_guard.run_guarded(
        ["fake-python"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=lambda: weak_samples,
        cleanup_orphans=False,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert process.terminate_calls == 1
    assert process.kill_calls == 1
    assert process.returncode == -9
    assert watched_calls
    assert all(unrelated_pid not in watched for watched in watched_calls)
    assert any(
        report.reason == "post_loop_unreaped_child_direct_child_handle"
        for report in result.termination_reports
    )


@pytest.mark.parametrize(
    ("case", "expected_closed"),
    [
        ("observed_descendant", True),
        ("escaped_descendant", True),
        ("unproven_root_exit", False),
        ("reused_root_pid", False),
        ("missing_descendant_birth", False),
        ("protected_group", False),
        ("unknown_group_member", False),
        ("signal_sample_failure", False),
        ("surviving_descendant", False),
        ("unobserved_descendant", False),
    ],
)
def test_reaped_root_cleanup_composes_with_scratch_closure(
    monkeypatch: pytest.MonkeyPatch, case: str, expected_closed: bool
) -> None:
    # Only OS observations/signals are simulated. Admission, signal-time birth
    # validation, termination reporting and the scratch consumer remain real.
    def absent_pid(_pid: int) -> int:
        raise ProcessLookupError

    monkeypatch.setattr(
        process_custody,
        "os",
        types.SimpleNamespace(
            name="posix",
            getpid=lambda: 999,
            getpgrp=lambda: 999,
            getpgid=absent_pid,
            getsid=absent_pid,
        ),
    )
    _posix_custody_boundary(monkeypatch)
    root = process_custody.ProcessSample(
        101, 999, 10, "python root.py", pgid=101, started_at_ns=11
    )
    descendant = process_custody.ProcessSample(
        202, 101, 20, "python child.py", pgid=101, started_at_ns=22
    )
    tracker = process_custody.ProcessTreeTracker(101)
    initial = {101: root}
    if case != "unobserved_descendant":
        initial[202] = descendant
    tracker.update(initial)
    live = {202: dataclasses.replace(descendant, ppid=1)}
    if case == "escaped_descendant":
        live[202] = dataclasses.replace(live[202], pgid=202)
    elif case == "reused_root_pid":
        live[101] = dataclasses.replace(root, ppid=1, started_at_ns=99)
    elif case == "missing_descendant_birth":
        live[202] = dataclasses.replace(live[202], started_at_ns=None)
    elif case in {"protected_group", "unknown_group_member"}:
        live[303] = process_custody.ProcessSample(
            303,
            1,
            30,
            "codex app-server" if case == "protected_group" else "unowned peer",
            pgid=101,
            started_at_ns=33,
        )
    sampling_calls = 0
    signals: list[tuple[str, int, int]] = []

    def sample() -> dict[int, process_custody.ProcessSample]:
        nonlocal sampling_calls
        sampling_calls += 1
        if case == "signal_sample_failure" and sampling_calls > 1:
            raise OSError("synthetic observation unavailable")
        return dict(live)

    def send(kind: str, target: int, signum: int):
        signals.append((kind, target, signum))
        if case != "surviving_descendant":
            for pid, row in list(live.items()):
                if (row.pgid if kind == "process_group" else pid) == target:
                    del live[pid]
        return process_custody._termination_action(
            target_kind=kind, target_id=target, signum=signum, result="sent"
        )

    monkeypatch.setattr(
        process_custody,
        "_send_process_group_signal_action",
        lambda pgid, signum: send("process_group", pgid, signum),
    )
    monkeypatch.setattr(
        process_custody,
        "_send_pid_signal_action",
        lambda pid, signum: send("process", pid, signum),
    )

    def group_closed(pgid: int, *, grace: float) -> bool:
        return not any(row.pgid == pgid for row in live.values())

    monkeypatch.setattr(
        process_custody, "_process_group_exited_or_unobservable", group_closed
    )
    monkeypatch.setattr(
        process_custody,
        "_pid_exited_or_unobservable",
        lambda pid, *, grace: pid not in live,
    )
    monkeypatch.setattr(
        memory_guard, "_process_group_exited_or_unobservable", group_closed
    )
    cleanup = process_custody.cleanup_tracked_orphans(
        101,
        tracker=tracker,
        sampler=sample,
        grace=0.0,
        root_reaped=case != "unproven_root_exit",
    )
    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=tracker,
        sampler=sample,
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=cleanup.termination_reports,
        probe_grace=0.0,
    )
    assert closed is expected_closed, evidence
    assert all(
        target not in {101, 303} for kind, target, _ in signals if kind == "process"
    )
    if expected_closed:
        assert not live
        assert evidence["termination_action_gaps"] == []
        assert evidence["root_process_group_closed"] is True
        assert evidence["remaining_tracked_pids"] == []
        assert cleanup.process_groups == (202 if case == "escaped_descendant" else 101,)
        assert signals == [
            ("process", 202, signal.SIGTERM)
            if case == "escaped_descendant"
            else ("process_group", 101, signal.SIGTERM)
        ]
    elif case == "unobserved_descendant":
        assert signals == []
        assert cleanup.termination_reports == ()
        assert evidence["root_process_group_closed"] is False
        assert evidence["remaining_tracked_pids"] == []
        assert evidence["root_process_group_members"] == [202]
    elif case in {
        "missing_descendant_birth",
        "protected_group",
        "signal_sample_failure",
    }:
        assert signals == []
    elif case in {"reused_root_pid", "unknown_group_member"}:
        assert all(kind != "process_group" for kind, _target, _sig in signals)
        assert (101 if case == "reused_root_pid" else 303) in live
    elif case == "surviving_descendant":
        assert 202 in live
        assert any(
            action.result == "still_live"
            for report in cleanup.termination_reports
            for action in report.actions
        )


@pytest.mark.parametrize("target_kind", ["process_group", "process"])
@pytest.mark.parametrize(
    "outcome",
    [
        "delayed_exit",
        "killed",
        "survivor",
        "sample_survivor_after_probe",
        "sampler_missing_only",
        "reused_at_escalation",
        "reused_at_terminal",
        "unowned_member",
        "protected_member",
        "escalation_sample_failure",
        "terminal_sample_failure",
        "terminal_probe_failure",
        "signal_failure",
        "final_group_live",
        "final_sample_failure",
    ],
)
def test_escalation_reconciles_only_proven_terminal_exit(
    monkeypatch: pytest.MonkeyPatch, target_kind: str, outcome: str
) -> None:
    # Real tracker, admission, signal-time custody, report and closure; only the
    # OS boundary is controlled. No signal can reach a real process.
    monkeypatch.setattr(
        process_custody,
        "os",
        types.SimpleNamespace(name="posix", getpid=lambda: 999, getpgrp=lambda: 999),
    )
    custody_signal = _posix_custody_boundary(monkeypatch)
    root = process_custody.ProcessSample(
        101, 999, 10, "python root.py", pgid=101, started_at_ns=11
    )
    child = process_custody.ProcessSample(
        202, 101, 20, "python child.py", pgid=101, started_at_ns=22
    )
    tracker = process_custody.ProcessTreeTracker(101)
    tracker.update({101: root, 202: child})
    escaped = target_kind == "process"
    live = {202: dataclasses.replace(child, ppid=1, pgid=202 if escaped else 101)}
    target = 202 if escaped else 101
    probes = 0
    signals: list[tuple[str, int, int]] = []
    closure_started = False

    def sample() -> dict[int, process_custody.ProcessSample]:
        if closure_started and outcome == "final_sample_failure":
            raise OSError("final snapshot unavailable")
        if probes == 1 and outcome == "escalation_sample_failure":
            raise OSError("escalation snapshot unavailable")
        if probes >= 2 and outcome == "terminal_sample_failure":
            raise OSError("terminal snapshot unavailable")
        return dict(live)

    def send(kind: str, pid: int, signum: int):
        signals.append((kind, pid, signum))
        assert (kind, pid) == (target_kind, target)
        if signum == custody_signal.SIGKILL:
            if outcome == "signal_failure":
                live.clear()  # A later empty sample cannot erase this failure.
                return process_custody._termination_action(
                    target_kind=kind,
                    target_id=pid,
                    signum=signum,
                    result="failed",
                    error="permission denied",
                )
            if outcome not in {"survivor", "sample_survivor_after_probe"}:
                live.clear()
        return process_custody._termination_action(
            target_kind=kind, target_id=pid, signum=signum, result="sent"
        )

    monkeypatch.setattr(
        process_custody,
        "_send_process_group_signal_action",
        lambda pid, signum: send("process_group", pid, signum),
    )
    monkeypatch.setattr(
        process_custody,
        "_send_pid_signal_action",
        lambda pid, signum: send("process", pid, signum),
    )

    def probe(pid: int, *, grace: float) -> bool:
        nonlocal probes
        assert pid == target
        probes += 1
        if probes == 1:
            # SIGTERM grace expires; the descendant can then exit before the
            # escalation snapshot. This is the retained macOS ordering.
            if outcome in {"delayed_exit", "sampler_missing_only"}:
                live.clear()
            elif outcome == "reused_at_escalation":
                live[202] = dataclasses.replace(live[202], started_at_ns=99)
            elif outcome in {"unowned_member", "protected_member"}:
                if escaped:
                    live[202] = dataclasses.replace(
                        live[202],
                        command="codex app-server"
                        if outcome == "protected_member"
                        else "peer",
                        started_at_ns=22 if outcome == "protected_member" else 99,
                    )
                else:
                    live[303] = process_custody.ProcessSample(
                        303,
                        1,
                        20,
                        "codex app-server" if outcome == "protected_member" else "peer",
                        pgid=101,
                        started_at_ns=33,
                    )
            return False
        if outcome == "reused_at_terminal":
            live[202] = dataclasses.replace(
                child, ppid=1, pgid=target, started_at_ns=99
            )
        if outcome == "terminal_probe_failure":
            raise OSError("kernel liveness unavailable")
        # An empty sampler snapshot is deliberately independent of the kernel
        # liveness oracle, including after escalation reports missing.
        return outcome not in {"survivor", "sampler_missing_only"}

    monkeypatch.setattr(process_custody, "_process_group_exited_or_unobservable", probe)
    monkeypatch.setattr(process_custody, "_pid_exited_or_unobservable", probe)
    final_probes: list[int] = []

    def final_probe(pgid: int, *, grace: float) -> bool:
        final_probes.append(pgid)
        return outcome != "final_group_live"

    monkeypatch.setattr(
        memory_guard, "_process_group_exited_or_unobservable", final_probe
    )
    cleanup = process_custody.cleanup_tracked_orphans(
        101, tracker=tracker, sampler=sample, grace=0.0, root_reaped=True
    )
    closure_started = True
    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=tracker,
        sampler=sample,
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=cleanup.termination_reports,
        probe_grace=0.0,
    )
    expected_closed = outcome in {"delayed_exit", "killed"}
    assert closed is expected_closed, evidence
    (report,) = cleanup.termination_reports
    actions = [
        a
        for a in report.actions
        if (a.target_kind, a.target_id) == (target_kind, target)
    ]
    assert actions[0].result == "still_live"  # Historical evidence is retained.
    assert signals[0] == (target_kind, target, signal.SIGTERM)
    terminal_proven = expected_closed or outcome in {
        "final_group_live",
        "final_sample_failure",
    }
    if terminal_proven:
        assert actions[-1].result == "exited"
        assert actions[-1].signal is None
        assert report.remaining_pgids == report.remaining_pids == ()
        assert cleanup.process_groups == (target,)
        assert final_probes == [101]
        assert probes == 2
    else:
        assert not any(action.result == "exited" for action in actions)
        assert target in (report.remaining_pids if escaped else report.remaining_pgids)
        assert cleanup.process_groups == ()
        assert evidence["termination_action_gaps"]
    if outcome == "delayed_exit":
        assert actions[1].result == "missing"
        assert signals == [(target_kind, target, signal.SIGTERM)]
    elif outcome in {
        "reused_at_escalation",
        "unowned_member",
        "protected_member",
        "escalation_sample_failure",
    }:
        assert signals == [(target_kind, target, signal.SIGTERM)]
        assert any(action.result.startswith("skipped_") for action in actions)
    elif outcome == "final_group_live":
        assert evidence["root_process_group_closed"] is False
    elif outcome == "final_sample_failure":
        assert evidence["final_sample_error"] == "OSError: final snapshot unavailable"
    if expected_closed:
        result = memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            orphaned_process_groups=cleanup.process_groups,
            termination_reports=cleanup.termination_reports,
            child_stderr="",
        )
        incident = memory_guard._incident_payload(result)
        assert incident is not None
        assert incident["reason"] == "orphaned_processes_cleaned"
        assert incident["process_groups"] == list(cleanup.process_groups)
        assert "orphan_cleanup_status" not in incident
        for override, reason in [
            ({"timed_out": True}, "timeout"),
            ({"guard_signal": int(signal.SIGTERM)}, "guard_interrupted"),
        ]:
            incident = memory_guard._incident_payload(
                dataclasses.replace(result, **override)
            )
            assert incident is not None
            assert incident["reason"] == reason
            assert "cleanup incomplete" not in str(incident["cleanup"])
            assert "orphan_cleanup_status" not in incident


def test_cleanup_tracked_orphans_terminates_live_tracked_groups(monkeypatch) -> None:
    tracker = process_custody.ProcessTreeTracker(100)
    assert tracker.known_pids is not None
    tracker.known_pids.update({200, 300})
    assert tracker.known_pgids is not None
    tracker.known_pgids.update({100, 300})
    samples = {
        200: process_custody.ProcessSample(
            pid=200,
            ppid=1,
            pgid=100,
            rss_kb=64,
            command="worker same group",
        ),
        300: process_custody.ProcessSample(
            pid=300,
            ppid=1,
            pgid=300,
            rss_kb=64,
            command="worker new group",
        ),
    }
    calls: list[dict[str, object]] = []
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        actions=(
            process_custody.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=process_custody.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
            process_custody.GuardTerminationAction(
                target_kind="process",
                target_id=300,
                signal=process_custody.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
        ),
    )

    def fake_terminate(root_pid, **kwargs):
        calls.append({"root_pid": root_pid, **kwargs})
        return report

    monkeypatch.setattr(process_custody, "terminate_watched_processes", fake_terminate)

    orphaned = process_custody.cleanup_tracked_orphans(
        100,
        tracker=tracker,
        sampler=lambda: samples,
        grace=0.125,
    )

    assert orphaned.process_groups == (100, 300)
    assert orphaned.termination_reports == (report,)
    assert calls[0]["root_pid"] == 100
    assert calls[0]["watched"] == {200, 300}
    assert calls[0]["grace"] == 0.125
    assert calls[0]["reason"] == "tracked_orphan_cleanup"


def test_cleanup_tracked_orphans_does_not_report_failed_actions_as_cleaned(
    monkeypatch,
) -> None:
    tracker = process_custody.ProcessTreeTracker(100)
    initial = {
        100: process_custody.ProcessSample(100, 1, 10, "guard.exe", started_at_ns=1),
        200: process_custody.ProcessSample(200, 100, 20, "worker.exe", started_at_ns=2),
    }
    tracker.update(initial)
    live = {200: initial[200]}
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        actions=(
            process_custody.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=process_custody.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    monkeypatch.setattr(
        process_custody,
        "terminate_watched_processes",
        lambda *_args, **_kwargs: report,
    )

    result = process_custody.cleanup_tracked_orphans(
        100,
        tracker=tracker,
        sampler=lambda: live,
    )

    assert result.process_groups == ()
    assert result.termination_reports == (report,)


def test_cleanup_group_completion_requires_every_detected_member() -> None:
    completed = memory_guard.GuardTerminationAction(
        target_kind="process",
        target_id=200,
        signal=memory_guard.signal.SIGTERM,
        signal_name="SIGTERM",
        result="completed_or_missing",
    )
    failed = memory_guard.GuardTerminationAction(
        target_kind="process",
        target_id=201,
        signal=memory_guard.signal.SIGTERM,
        signal_name="SIGTERM",
        result="failed",
        error="access denied",
    )

    assert (
        memory_guard._fully_completed_process_groups(
            {777: {200, 201}},
            _guard_termination_report(actions=(completed, failed)),
        )
        == set()
    )
    assert memory_guard._fully_completed_process_groups(
        {777: {200, 201}},
        _guard_termination_report(
            actions=(
                completed,
                memory_guard.GuardTerminationAction(
                    target_kind="process",
                    target_id=201,
                    signal=memory_guard.signal.SIGTERM,
                    signal_name="SIGTERM",
                    result="completed_or_missing",
                ),
            )
        ),
    ) == {777}
    # Neither successful group exit nor a later PID success erases a member's
    # failed authority; remaining markers independently prevent a clean claim.
    group_exit = dataclasses.replace(
        completed,
        target_kind="process_group",
        target_id=777,
        signal=None,
        signal_name=None,
        result="exited",
    )
    for report in (
        _guard_termination_report(actions=(completed, failed, group_exit)),
        _guard_termination_report(
            actions=(failed, dataclasses.replace(completed, target_id=201), completed)
        ),
        dataclasses.replace(
            _guard_termination_report(actions=(group_exit,)), remaining_pgids=(777,)
        ),
    ):
        assert (
            memory_guard._fully_completed_process_groups({777: {200, 201}}, report)
            == set()
        )


def test_cleanup_repo_scoped_orphans_since_baseline_only_drains_tracked_orphans(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    root = memory_guard.ROOT.as_posix()
    tracker = memory_guard.ProcessTreeTracker(100)
    tracker.update(
        {
            100: memory_guard.ProcessSample(
                pid=100,
                ppid=1,
                pgid=100,
                rss_kb=64,
                command=f"{root}/.venv/bin/python3 -m pytest tests/root.py",
                started_at_ns=100,
            ),
            200: memory_guard.ProcessSample(
                pid=200,
                ppid=100,
                pgid=200,
                rss_kb=64,
                command=f"{root}/.venv/bin/python3 -m molt.cli build main.py",
                started_at_ns=200,
            ),
            300: memory_guard.ProcessSample(
                pid=300,
                ppid=200,
                pgid=300,
                rss_kb=64,
                command=f"{root}/target/dev-fast/molt-backend --ir-file ir.json",
                started_at_ns=300,
            ),
        }
    )
    samples = {
        50: memory_guard.ProcessSample(
            pid=50,
            ppid=1,
            pgid=50,
            rss_kb=64,
            command="/bin/zsh -l",
            started_at_ns=50,
        ),
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=64,
            command=f"{root}/.venv/bin/python3 -m molt.cli build main.py",
            started_at_ns=200,
        ),
        300: memory_guard.ProcessSample(
            pid=300,
            ppid=200,
            pgid=300,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --ir-file ir.json",
            started_at_ns=300,
        ),
        400: memory_guard.ProcessSample(
            pid=400,
            ppid=50,
            pgid=400,
            rss_kb=64,
            command=f"{root}/.venv/bin/python3 -m pytest tests/some_test.py",
            started_at_ns=400,
        ),
        500: memory_guard.ProcessSample(
            pid=500,
            ppid=1,
            pgid=500,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --old",
            started_at_ns=500,
        ),
        550: memory_guard.ProcessSample(
            pid=550,
            ppid=1,
            pgid=550,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --untracked",
            started_at_ns=550,
        ),
        600: memory_guard.ProcessSample(
            pid=600,
            ppid=1,
            pgid=600,
            rss_kb=64,
            command="/Applications/Claude.app/Contents/MacOS/Claude",
            started_at_ns=600,
        ),
        601: memory_guard.ProcessSample(
            pid=601,
            ppid=600,
            pgid=600,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --protected",
            started_at_ns=601,
        ),
    }
    terminated: list[tuple[int, int]] = []

    def fake_kill(pid: int, sig: int) -> None:
        if sig == 0 and any(sent_pid == pid for sent_pid, _sig in terminated):
            raise ProcessLookupError
        if sig == memory_guard.signal.SIGTERM:
            terminated.append((pid, sig))

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        getpgrp=lambda: 999,
        kill=fake_kill,
    )

    cleaned = memory_guard.cleanup_repo_scoped_orphans_since_baseline(
        baseline_pgids=frozenset({500}),
        tracker=tracker,
        sampler=lambda: samples,
        grace=0.125,
    )

    assert cleaned.process_groups == (200, 300)
    assert [report.reason for report in cleaned.termination_reports] == [
        "repo_scoped_orphan_cleanup",
        "repo_scoped_orphan_cleanup",
    ]
    assert [report.root_pgid for report in cleaned.termination_reports] == [200, 300]
    assert [
        action.target_id
        for report in cleaned.termination_reports
        for action in report.actions
    ] == [200, 300]
    assert all(
        action.result == "completed_or_missing"
        for report in cleaned.termination_reports
        for action in report.actions
    )
    assert terminated == [
        (200, memory_guard.signal.SIGTERM),
        (300, memory_guard.signal.SIGTERM),
    ]


def test_cleanup_repo_scoped_orphans_revalidates_identity_before_signal(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    root = memory_guard.ROOT.as_posix()
    tracker = memory_guard.ProcessTreeTracker(100)
    tracker.update(
        {
            100: memory_guard.ProcessSample(
                pid=100,
                ppid=1,
                pgid=100,
                rss_kb=64,
                command=f"{root}/.venv/bin/python3 -m pytest tests/root.py",
                started_at_ns=100,
            ),
            200: memory_guard.ProcessSample(
                pid=200,
                ppid=100,
                pgid=200,
                rss_kb=64,
                command=f"{root}/target/dev-fast/molt-backend --owned",
                started_at_ns=200,
            ),
        }
    )
    owned_orphan = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=64,
            command=f"{root}/target/dev-fast/molt-backend --owned",
            started_at_ns=200,
        )
    }
    reused_pid = {
        200: memory_guard.ProcessSample(
            pid=200,
            ppid=1,
            pgid=200,
            rss_kb=64,
            command="/Applications/Claude.app/Contents/MacOS/Claude",
            started_at_ns=201,
        )
    }
    sampler_calls = 0

    def sampler():
        nonlocal sampler_calls
        sampler_calls += 1
        return owned_orphan if sampler_calls <= 2 else reused_pid

    terminated: list[tuple[int, float]] = []
    # Synthetic PIDs may name live host processes; a regressed identity gate
    # must fail here, never reach a real signal.
    signals: list[tuple[str, int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        getpgrp=lambda: 999,
        killpg=lambda pgid, sig: signals.append(("killpg", pgid, sig)),
        kill=lambda pid, sig: signals.append(("kill", pid, sig)),
    )
    monkeypatch.setattr(
        memory_guard,
        "_terminate_single_pid",
        lambda pid, *, grace: terminated.append((pid, grace)) or True,
    )

    cleaned = memory_guard.cleanup_repo_scoped_orphans_since_baseline(
        baseline_pgids=frozenset(),
        tracker=tracker,
        sampler=sampler,
        grace=0.125,
    )

    assert cleaned.process_groups == ()
    assert len(cleaned.termination_reports) == 1
    assert cleaned.termination_reports[0].actions[0].result == (
        "skipped_identity_mismatch"
    )
    assert terminated == []
    assert signals == []


def test_terminate_verified_pid_revalidates_identity_before_fallback(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    root = memory_guard.ROOT.as_posix()
    original = memory_guard.ProcessSample(
        pid=200,
        ppid=1,
        pgid=200,
        rss_kb=64,
        command=f"{root}/target/dev-fast/molt-backend --owned",
        started_at_ns=111,
    )
    reused_pid = memory_guard.ProcessSample(
        pid=200,
        ppid=1,
        pgid=200,
        rss_kb=64,
        command="/Applications/Claude.app/Contents/MacOS/Claude",
        started_at_ns=222,
    )
    sample_sets = iter([{200: original}, {200: reused_pid}])
    sent: list[tuple[int, int]] = []

    monkeypatch.setattr(
        memory_guard,
        "_pid_exited_or_unobservable",
        lambda pid, *, grace: False,
    )
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        getpgrp=lambda: 999,
        kill=lambda pid, sig: None if sig == 0 else sent.append((pid, sig)),
    )

    actions = memory_guard.terminate_verified_pid(
        200,
        memory_guard.process_identity(original),
        sampler=lambda: next(sample_sets),
        grace=0.125,
    )

    assert [action.result for action in actions] == [
        "still_live",
        "skipped_identity_mismatch",
    ]
    assert sent == [(200, memory_guard.signal.SIGTERM)]


def test_terminate_verified_pid_preserves_host_control_plane(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sample = memory_guard.ProcessSample(
        pid=300,
        ppid=1,
        pgid=300,
        rss_kb=64,
        command="codex exec --dangerously-skip-approvals",
        started_at_ns=333,
    )
    sent: list[tuple[int, int]] = []
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        getpgrp=lambda: 999,
        kill=lambda pid, sig: None if sig == 0 else sent.append((pid, sig)),
    )

    actions = memory_guard.terminate_verified_pid(
        300,
        memory_guard.process_identity(sample),
        sampler=lambda: {300: sample},
        grace=0.125,
    )

    assert [action.result for action in actions] == ["skipped_host_control_plane"]
    assert sent == []


def test_cleanup_tracked_orphans_sampler_failure_uses_remembered_watched(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tracker = process_custody.ProcessTreeTracker(100)
    assert tracker.known_pids is not None
    tracker.known_pids.add(200)
    remembered_samples = {
        200: process_custody.ProcessSample(
            pid=200,
            ppid=1,
            pgid=None,
            rss_kb=64,
            command="escaped worker",
        )
    }
    calls: list[dict[str, object]] = []

    def failing_sampler() -> Mapping[int, process_custody.ProcessSample]:
        raise RuntimeError("sampler unavailable")

    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def fake_terminate(
        root_pid: int, **kwargs: object
    ) -> process_custody.GuardTerminationReport:
        calls.append({"root_pid": root_pid, **kwargs})
        return report

    monkeypatch.setattr(process_custody, "terminate_watched_processes", fake_terminate)

    with pytest.raises(RuntimeError, match="sampler unavailable"):
        process_custody.cleanup_tracked_orphans(
            100,
            tracker=tracker,
            sampler=failing_sampler,
            remembered_samples=remembered_samples,
            remembered_watched={200},
        )

    assert calls and calls[0]["watched"] == {200}
    assert calls[0]["reason"] == "tracked_orphan_cleanup"


def test_windows_cleanup_sampler_failure_never_signals_remembered_pid(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    tracker = process_custody.ProcessTreeTracker(100)
    root = process_custody.ProcessSample(100, 1, 10, "guard.exe", started_at_ns=100)
    child = process_custody.ProcessSample(200, 100, 20, "worker.exe", started_at_ns=200)
    remembered = {100: root, 200: child}
    tracker.update(remembered)
    sent: list[tuple[int, int]] = []

    def failing_sampler() -> Mapping[int, process_custody.ProcessSample]:
        raise RuntimeError("live sampler unavailable")

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        kill=lambda pid, sig: sent.append((pid, sig)),
    )

    with pytest.raises(RuntimeError, match="live sampler unavailable"):
        process_custody.cleanup_tracked_orphans(
            100,
            tracker=tracker,
            sampler=failing_sampler,
            remembered_samples=remembered,
            remembered_watched={200},
        )

    assert sent == []


def test_pid_permission_error_is_live_unknown_not_completed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    sample = memory_guard.ProcessSample(
        pid=300,
        ppid=1,
        pgid=300,
        rss_kb=64,
        command="worker",
        started_at_ns=333,
    )
    sent: list[tuple[int, int]] = []
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)

    def permission_liveness(pid: int, sig: int) -> None:
        if sig == 0:
            raise PermissionError("EPERM")
        sent.append((pid, sig))

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        getpgrp=lambda: 999,
        kill=permission_liveness,
    )

    action = memory_guard._terminate_pid_if_identity_action(
        300,
        memory_guard.process_identity(sample),
        sampler=lambda: {300: sample},
        grace=0.01,
    )

    assert action.result == "still_live"
    assert sent == [(300, memory_guard.signal.SIGTERM)]


@pytest.mark.parametrize("exit_at", [0.0, 0.01, 0.02])
def test_process_group_exit_probe_observes_at_the_end_of_its_window(
    monkeypatch: pytest.MonkeyPatch, exit_at: float
) -> None:
    # An exit anywhere in the window, including at the deadline (the owned
    # child's reap racing the window), must be observed.
    grace = 0.02
    now = [0.0]
    monkeypatch.setattr(
        process_custody,
        "time",
        types.SimpleNamespace(
            monotonic=lambda: now[0],
            sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
        ),
    )
    probes: list[float] = []

    def killpg(pgid: int, sig: int) -> None:
        assert (pgid, sig) == (300, 0)
        probes.append(now[0])
        if now[0] >= exit_at:
            raise ProcessLookupError

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        killpg=killpg,
    )

    assert process_custody.process_group_exited_or_unobservable(300, grace=grace)
    assert probes[-1] >= exit_at and probes[-1] <= grace


def test_process_group_permission_error_is_live_unknown(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        killpg=lambda _pgid, _sig: (_ for _ in ()).throw(PermissionError("EPERM")),
    )

    assert not memory_guard._process_group_exited_or_unobservable(300, grace=0.01)


def test_process_group_probe_observes_through_refusal_until_absence(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """EPERM is presence, not an unknown that ends the window (HF-155).

    XNU refuses every group signal while all members exit or wait for their
    reap, and the group vanishes only at that reap, which a loaded host can
    delay. A probe that stops at the first EPERM reports a closed tree live.
    """
    now = [0.0]
    monkeypatch.setattr(
        process_custody,
        "time",
        types.SimpleNamespace(
            monotonic=lambda: now[0],
            sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
        ),
    )

    def killpg(pgid: int, sig: int) -> None:
        assert (pgid, sig) == (300, 0)
        if now[0] < 0.5:
            raise PermissionError(errno.EPERM, "Operation not permitted")
        raise ProcessLookupError

    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)
    install_module_view(monkeypatch, "os", os, process_custody, killpg=killpg)

    assert process_custody.process_group_exited_or_unobservable(300, grace=2.0)
    assert 0.5 <= now[0] <= 2.0


@pytest.mark.skipif(os.name != "posix", reason="POSIX process groups")
def test_process_group_probe_observes_an_exited_group_until_its_reap() -> None:
    """The kernel is the oracle (HF-155).

    The session leader exits at once and stays a zombie until a timer reaps
    it. Darwin answers the group probe with EPERM until then and Linux with
    success; either way the probe reports absence only after the reap, and
    within its window.
    """
    proc = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
    reaper = threading.Timer(0.5, proc.wait)
    try:
        time.sleep(0.3)  # Let the child exit; nothing reaps it yet.
        started = time.monotonic()
        reaper.start()
        closed = process_custody.process_group_exited_or_unobservable(
            proc.pid, grace=10.0
        )
        elapsed = time.monotonic() - started
    finally:
        reaper.join(15.0)
        proc.wait(timeout=15.0)
    assert closed
    assert elapsed >= 0.4


@pytest.mark.parametrize("group_vanishes", [True, False])
def test_refused_group_signal_resolves_only_by_an_observed_exit(
    monkeypatch: pytest.MonkeyPatch,
    group_vanishes: bool,
) -> None:
    """A group whose members all exit refuses every signal (XNU EPERM).

    The refusal is neither delivery nor failure (HF-155): the guard keeps
    observing, and only the group's observed absence completes the target.
    A refusal that outlasts every window stays incomplete.
    """
    now = [0.0]
    reaped_at = 0.3 if group_vanishes else float("inf")
    signals: list[tuple[int, int]] = []

    def killpg(pgid: int, signum: int) -> None:
        if signum:
            signals.append((pgid, signum))
        if now[0] >= reaped_at:
            raise ProcessLookupError
        raise PermissionError(errno.EPERM, "Operation not permitted")

    monkeypatch.setattr(
        process_custody,
        "os",
        types.SimpleNamespace(
            name="posix", getpid=lambda: 999, getpgrp=lambda: 999, killpg=killpg
        ),
    )
    monkeypatch.setattr(
        process_custody,
        "time",
        types.SimpleNamespace(
            monotonic=lambda: now[0],
            sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
        ),
    )
    custody_signal = _posix_custody_boundary(monkeypatch)
    root = process_custody.ProcessSample(
        101, 999, 10, "python root.py", pgid=101, started_at_ns=11
    )
    child = process_custody.ProcessSample(
        202, 101, 20, "python child.py", pgid=101, started_at_ns=22
    )
    tracker = process_custody.ProcessTreeTracker(101)
    tracker.update({101: root, 202: child})
    # The root is reaped. Its child is still listed while it exits.
    exiting = {202: dataclasses.replace(child, ppid=1)}

    def sample() -> dict[int, process_custody.ProcessSample]:
        return dict(exiting) if now[0] < reaped_at else {}

    cleanup = process_custody.cleanup_tracked_orphans(
        101, tracker=tracker, sampler=sample, grace=0.25, root_reaped=True
    )
    closed, evidence = memory_guard._temporary_artifact_descendant_closure(
        proc=types.SimpleNamespace(returncode=0),
        child_process=_guarded_child(),
        tracker=tracker,
        sampler=sample,
        windows_job_cleanup=None,
        windows_process_model=False,
        posix_process_model=True,
        cleanup_orphans=True,
        guard_interrupted=False,
        termination_wait_expired=False,
        sampling_telemetry=_complete_sampling_telemetry(),
        termination_reports=cleanup.termination_reports,
        probe_grace=0.25,
    )

    (report,) = cleanup.termination_reports
    group_results = [
        (action.signal_name, action.result)
        for action in report.actions
        if (action.target_kind, action.target_id) == ("process_group", 101)
    ]
    assert signals == [(101, signal.SIGTERM), (101, custody_signal.SIGKILL)]
    assert group_results[:2] == [("SIGTERM", "still_live"), ("SIGKILL", "refused")]
    assert closed is group_vanishes, evidence
    if group_vanishes:
        assert group_results[-1] == (None, "exited")
        assert report.remaining_pgids == ()
        assert cleanup.process_groups == (101,)
    else:
        assert group_results[-1] == (None, "still_live")
        assert report.remaining_pgids == (101,)
        assert cleanup.process_groups == ()


def test_tracker_admits_the_reserved_root_group_from_the_exit_census() -> None:
    """Census rules (HF-146): the root leads its group and is not yet reaped."""
    sample = memory_guard.ProcessSample
    tracker = memory_guard.ProcessTreeTracker(100)
    tracker.update({100: sample(100, 1, 10, "root", pgid=100, started_at_ns=100)})
    released = sample(105, 1, 1, "transferred", pgid=100, started_at_ns=170)
    assert tracker.released_identities is not None
    tracker.released_identities[105] = memory_guard.process_identity(released)
    orphan = sample(101, 1, 1, "orphaned child", pgid=100, started_at_ns=150)
    census = {
        100: sample(100, 1, 10, "root", pgid=100, started_at_ns=100),
        101: orphan,
        102: sample(102, 1, 1, "other group", pgid=102, started_at_ns=160),
        103: sample(103, 1, 1, "no birth", pgid=100, started_at_ns=None),
        104: sample(104, 1, 1, "born before the root", pgid=100, started_at_ns=50),
        105: released,
    }

    assert tracker.admit_reserved_group_members(census) == {101}
    assert tracker.admit_reserved_group_members(census) == set()
    # Parentage follows the admitted member as it does any tracked process.
    grandchild = sample(106, 101, 1, "grandchild", pgid=100, started_at_ns=180)
    assert tracker.update({101: orphan, 106: grandchild}) == {101, 106}


@pytest.mark.parametrize(
    ("probe_error", "samples_table"),
    [
        (None, True),
        (ProcessLookupError(errno.ESRCH, "No such process"), False),
        (PermissionError(errno.EPERM, "Operation not permitted"), False),
        (OSError(errno.EINVAL, "Invalid argument"), True),
    ],
)
def test_exit_census_reads_the_table_only_when_a_member_can_take_a_signal(
    monkeypatch: pytest.MonkeyPatch,
    probe_error: OSError | None,
    samples_table: bool,
) -> None:
    """A group the kernel finds absent, or whose members all refuse signals
    (the exiting root alone on XNU), holds no member the guard could end; the
    census skips the table read that every guarded command would pay."""
    probes: list[tuple[int, int]] = []

    def killpg(pgid: int, sig: int) -> None:
        probes.append((pgid, sig))
        if probe_error is not None:
            raise probe_error

    table = {7: memory_guard.ProcessSample(7, 1, 1, "member", pgid=100)}
    reads: list[bool] = []

    def sampler() -> Mapping[int, memory_guard.ProcessSample]:
        reads.append(True)
        return table

    install_module_view(monkeypatch, "os", os, process_custody, killpg=killpg)
    census = process_custody.reserved_group_exit_census(100, sampler)

    assert probes == [(100, 0)]
    assert reads == ([True] if samples_table else [])
    assert census == (table if samples_table else {})


def test_completed_process_group_does_not_emit_redundant_member_kill(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    custody = process_custody
    samples = {
        100: memory_guard.ProcessSample(
            100, 1, 10, "root", pgid=100, started_at_ns=100
        ),
        101: memory_guard.ProcessSample(
            101, 100, 20, "child", pgid=100, started_at_ns=101
        ),
    }
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: False)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        name="posix",
        getpid=lambda: 999,
    )
    monkeypatch.setattr(custody, "_safe_getpgrp", lambda: 999)
    monkeypatch.setattr(custody, "_safe_getpgid", lambda _pid: 100)
    monkeypatch.setattr(custody, "_safe_getsid", lambda _pid: 100)
    monkeypatch.setattr(
        custody,
        "_current_protected_process_group_ids",
        lambda _samples, **_kwargs: set(),
    )
    monkeypatch.setattr(
        custody,
        "_terminate_process_group_if_identities_match_action",
        lambda pgid, _identities, **_kwargs: memory_guard.GuardTerminationAction(
            target_kind="process_group",
            target_id=pgid,
            signal=memory_guard.signal.SIGTERM,
            signal_name="SIGTERM",
            result="completed_or_missing",
        ),
    )
    monkeypatch.setattr(
        custody,
        "_send_pid_signal_if_identity_action",
        lambda *_args, **_kwargs: pytest.fail(
            "completed group must not emit redundant member SIGKILL"
        ),
    )

    report = custody.terminate_watched_processes(
        100,
        samples=samples,
        watched=set(samples),
        expected_identities={
            pid: memory_guard.process_identity(sample)
            for pid, sample in samples.items()
        },
        sampler=lambda: samples,
        root_owned=True,
    )

    assert [action.result for action in report.actions] == ["completed_or_missing"]


def test_run_command_cleans_tracked_orphans_by_default(monkeypatch) -> None:
    calls: list[dict[str, object]] = []
    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def fake_cleanup(root_pid, **kwargs):
        calls.append({"root_pid": root_pid, **kwargs})
        return memory_guard.GuardOrphanCleanupResult(
            process_groups=(777,),
            termination_reports=(report,),
        )

    monkeypatch.setattr(memory_guard, "cleanup_tracked_orphans", fake_cleanup)
    # Exercise the explicit no-Job fallback; a live Windows Job is itself the
    # exact descendant cleanup authority and intentionally bypasses PID-table
    # orphan cleanup.
    monkeypatch.setattr(memory_guard._win_job, "create_kill_on_close_job", lambda: None)
    _patch_temporary_artifact_closure_closed(monkeypatch)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.orphaned_process_groups == (777,)
    assert result.termination_reports == (report,)
    assert len(calls) == 1
    assert calls[0]["root_reaped"] is True


def test_run_command_timeout_reports_post_baseline_repo_orphan_cleanup(
    monkeypatch,
) -> None:
    report = _guard_termination_report(
        reason="repo_scoped_orphan_cleanup",
        root_pid=222,
        root_pgid=222,
    )

    spawned: list[int] = []

    def child_only_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        # The host table, narrowed to the guarded child: the launch baseline
        # is exactly the child's own group, and the timeout terminates the
        # child through ordinary identity-checked custody.
        return {
            pid: sample
            for pid, sample in memory_guard.sample_processes().items()
            if pid in spawned
        }

    def fake_cleanup(**kwargs):
        assert spawned and kwargs["baseline_pgids"] == frozenset(spawned)
        return memory_guard.GuardOrphanCleanupResult(
            process_groups=(222,),
            termination_reports=(report,),
        )

    monkeypatch.setattr(
        memory_guard,
        "cleanup_repo_scoped_orphans_since_baseline",
        fake_cleanup,
    )
    monkeypatch.setattr(memory_guard._win_job, "create_kill_on_close_job", lambda: None)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=0.01,
        sampler=child_only_sampler,
        on_spawn=spawned.append,
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert result.orphaned_process_groups == (222,)
    assert report in result.termination_reports


def test_run_guarded_observes_child_exit_before_timeout_race(
    monkeypatch: pytest.MonkeyPatch,
    fake_popen_without_windows_job: None,
) -> None:
    # Model the race, not real scratch filesystem/setup latency. The clock
    # crosses the deadline only when fake wait publishes the child exit.
    clock = {"now": 100.0}
    guard_time = types.SimpleNamespace(**vars(time))
    guard_time.perf_counter = lambda: clock["now"]
    guard_time.perf_counter_ns = lambda: int(clock["now"] * 1_000_000_000)
    monkeypatch.setattr(memory_guard, "time", guard_time)

    def unexpected_termination(*args: object, **kwargs: object) -> None:
        raise AssertionError("observed child exit must prevent termination")

    monkeypatch.setattr(
        memory_guard, "terminate_watched_processes", unexpected_termination
    )

    class FakePopen:
        pid = 4242
        stdin = None
        returncode: int | None = None
        _handle = None

        def __init__(self, command: list[str], **_kwargs: object) -> None:
            self.command = command

        def poll(self) -> int | None:
            return self.returncode

        def wait(self, timeout: float | None = None) -> int:
            if self.returncode is None:
                if timeout is not None and timeout <= 0.02:
                    clock["now"] = 100.51
                    self.returncode = 0
                    raise subprocess.TimeoutExpired(self.command, timeout)
                self.returncode = 0
            return self.returncode

    _patch_guard_popen_without_windows_job(monkeypatch, FakePopen)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "pass"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=lambda: {},
        timeout=0.5,
        cleanup_orphans=False,
    )

    assert result.returncode == 0
    assert result.timed_out is False


def test_run_command_captures_large_stdout_without_pipe_deadlock() -> None:
    payload_size = 512 * 1024
    script = (
        "import sys; "
        f"sys.stdout.write('x' * {payload_size}); "
        "sys.stdout.flush(); "
        "sys.stderr.write('done\\n')"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=5.0,
    )

    assert result.returncode == 0
    assert len(result.stdout) == payload_size
    assert result.stderr == "done\n"


def test_run_command_feeds_stdin_under_guard() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import sys; print(sys.stdin.read().upper())"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        input="guarded stdin",
    )

    assert result.returncode == 0
    assert result.stdout == "GUARDED STDIN\n"


def test_run_command_elapsed_tracks_the_direct_guarded_command() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(0.03); print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=1.0,
        child_rlimit_kb=1_000_000,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.elapsed_s is not None
    assert result.elapsed_s >= 0.02
    nested_guard_budget = memory_guard.ACTIVE_ENV in os.environ
    elapsed_ceiling = 8.0 if nested_guard_budget else (2.0 if os.name == "nt" else 0.5)
    assert result.elapsed_s < elapsed_ceiling


def test_run_command_ignores_samples_without_root_pid() -> None:
    def sampler() -> dict[int, memory_guard.ProcessSample]:
        return {
            999_999: memory_guard.ProcessSample(999_999, 1, 1, "missing-root"),
        }

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('ok')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=sampler,
    )

    assert result.returncode == 0
    assert result.violation is None


def test_run_command_returns_guard_code_on_real_low_limit() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        max_rss_kb=1,
        poll_interval=0.01,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.rss_kb > 1


def test_run_command_fast_start_poll_catches_allocator_before_slow_poll() -> None:
    # The child holds the allocation until the guard kills it, so exit rusage
    # never reports it. Only a live sample taken in the fast-start window can
    # trip the limit before the slow poll interval elapses.
    script = "buf = bytearray(192 * 1024 * 1024)\n" + HUNG_CHILD_SOURCE

    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=96 * 1024,
        max_total_rss_kb=160 * 1024,
        poll_interval=5.0,
        child_rlimit_kb=None,
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.scope in {"process", "process_tree"}
    assert result.elapsed_s is not None
    assert result.elapsed_s < 5.0


def test_run_command_rusage_catches_short_lived_allocator_spike() -> None:
    if memory_guard.os.name != "posix" or not hasattr(memory_guard.os, "wait4"):
        pytest.skip("requires POSIX wait4 resource accounting")
    script = "import os\nbuf = bytearray(192 * 1024 * 1024)\nos._exit(0)"

    # A blind sampler models a spike between samples on every host: a fast
    # sampler (Linux /proc) can otherwise catch it live. Only the exit rusage
    # can then report the violation.
    result = memory_guard.run_guarded(
        [sys.executable, "-c", script],
        max_rss_kb=96 * 1024,
        max_total_rss_kb=160 * 1024,
        poll_interval=1.0,
        child_rlimit_kb=None,
        sampler=lambda: {},
    )

    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    assert result.violation is not None
    assert result.violation.scope == "process_rusage"


def test_run_command_returns_timeout_code_when_wall_clock_expires() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        timeout=0.01,
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert "timeout after" in result.stderr


def test_run_command_timeout_teardown_uses_bounded_wait(
    monkeypatch,
    fake_popen_without_windows_job: None,
) -> None:
    waits: list[float | None] = []
    sample_reads: list[bool] = []

    def custom_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        sample_reads.append(True)
        return {}

    def unexpected_default(*_args):
        raise AssertionError("timeout replaced the caller's sampler")

    class FakeProc:
        pid = 987654
        returncode: int | None = None
        stdin = None

        def __init__(self, command, **_kwargs):  # type: ignore[no-untyped-def]
            self.command = list(command)

        def wait(self, timeout=None):  # type: ignore[no-untyped-def]
            waits.append(timeout)
            if timeout is None:
                raise AssertionError("memory guard attempted an unbounded wait")
            raise subprocess.TimeoutExpired(self.command, timeout)

        def poll(self):  # type: ignore[no-untyped-def]
            return self.returncode

        def terminate(self) -> None:
            pass

        def kill(self) -> None:
            pass

    _patch_guard_popen_without_windows_job(monkeypatch, FakeProc)
    monkeypatch.setattr(memory_guard, "sample_processes", unexpected_default)
    monkeypatch.setattr(memory_guard, "sample_processes_custody", unexpected_default)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "import time; time.sleep(10)"],
        max_rss_kb=1_000_000,
        poll_interval=0.001,
        timeout=0.001,
        env={**guard_custody_env(), "MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC": "0.001"},
        sampler=custom_sampler,
    )

    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.timed_out is True
    assert "termination wait expired" in result.stderr
    assert waits
    assert None not in waits
    assert len(sample_reads) >= 2
    assert result.sampling_telemetry is not None
    assert result.sampling_telemetry.attempts >= 2
    assert result.sampling_telemetry.source == "custom"


def test_exit_signal_payload_classifies_direct_signal_status() -> None:
    assert memory_guard._exit_signal_payload(-15) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": False,
    }


def test_exit_signal_payload_names_posix_only_signal_numbers() -> None:
    assert memory_guard._exit_signal_payload(-9) == {
        "signal": 9,
        "name": "SIGKILL",
        "conventional_shell_status": False,
    }


def test_exit_signal_payload_classifies_shell_signal_status() -> None:
    assert memory_guard._exit_signal_payload(143) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }


def test_exit_signal_payload_classifies_windows_sigterm_status(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(process_model, "is_windows_process_model", lambda: True)
    assert memory_guard._exit_signal_payload(15) == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": False,
    }


def test_run_guarded_signal_exit_defers_without_owned_incremental_evidence(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    live_file = target / "debug" / "incremental" / "unit" / "work.o"
    live_file.parent.mkdir(parents=True, exist_ok=True)
    live_file.write_text("work", encoding="utf-8")

    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            "import os, signal; os.kill(os.getpid(), signal.SIGTERM)",
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={**guard_custody_env(), "CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )

    assert result.returncode == (15 if os.name == "nt" else -15)
    assert result.cargo_incremental_quarantine is None
    assert live_file.exists()

    fake_cargo = tmp_path / ("cargo.cmd" if os.name == "nt" else "cargo")
    if os.name == "nt":
        fake_cargo.write_text(
            f'@echo off\r\n"{sys.executable}" -c "import os, signal; '
            'os.kill(os.getpid(), signal.SIGTERM)"\r\n',
            encoding="utf-8",
        )
    else:
        fake_cargo.write_text(
            f"#!{sys.executable}\n"
            "import os, signal\n"
            "os.kill(os.getpid(), signal.SIGTERM)\n",
            encoding="utf-8",
        )
    fake_cargo.chmod(0o755)
    result = memory_guard.run_guarded(
        [str(fake_cargo)],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={**guard_custody_env(), "CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )

    assert result.returncode != 0
    assert result.cargo_incremental_quarantine is not None
    assert result.cargo_incremental_quarantine.ownership_status == "deferred"
    assert live_file.exists()


def _run_guarded_cargo_with_fake_orphan_cleanup(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    *,
    exit_code: int,
) -> tuple[
    memory_guard.GuardResult,
    list[dict[str, object]],
    memory_guard.GuardTerminationReport,
]:
    target = tmp_path / "target"
    calls: list[dict[str, object]] = []
    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def fake_cleanup(root_pid: int, **kwargs: object):
        return memory_guard.GuardOrphanCleanupResult(
            process_groups=(777,),
            termination_reports=(report,),
        )

    def fake_quarantine(**kwargs: object):
        calls.append(kwargs)
        return memory_guard.CargoIncrementalQuarantine(
            reason=str(kwargs["reason"]),
            recorded_at="2026-07-09T00:00:00Z",
            target_dir=str(target),
            quarantine_dir=None,
            command=tuple(kwargs["command"]),
            cwd=str(kwargs["cwd"]),
        )

    monkeypatch.setattr(memory_guard, "cleanup_tracked_orphans", fake_cleanup)
    monkeypatch.setattr(memory_guard._win_job, "create_kill_on_close_job", lambda: None)
    _patch_temporary_artifact_closure_closed(monkeypatch)
    monkeypatch.setattr(
        memory_guard,
        "_quarantine_cargo_incremental_state",
        fake_quarantine,
    )

    script = "print('ok')" if exit_code == 0 else f"import sys; sys.exit({exit_code})"
    result = memory_guard.run_guarded(
        [sys.executable, "-c", script, "cargo"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=tmp_path,
        env={**guard_custody_env(), "CARGO_TARGET_DIR": str(target)},
        sampler=lambda: {},
    )
    return result, calls, report


def _wait_for_pid_file(path: Path, *, timeout_s: float = 10.0) -> int:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        text = path.read_text(encoding="utf-8") if path.exists() else ""
        if text.strip():
            return int(text)
        time.sleep(0.01)
    raise AssertionError(f"no pid published at {path}")


@pytest.mark.skipif(os.name != "posix", reason="POSIX sampled orphan custody")
def test_run_guarded_terminates_a_new_session_grandchild_left_after_exit(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """The grandchild escapes the child's group and outlives the child.

    Only the guard's tracked custody can find it after the child exits: it is
    reparented, in its own session, and named by no group the guard created.
    The child exits only once a poll has sampled the grandchild under it; a
    fixed half-second wait let a starved guard miss it.
    """
    pid_file = tmp_path / "grandchild.pid"
    sampled = tmp_path / "grandchild-sampled"
    marker = hung_child_marker()
    custody_sample = memory_guard.sample_processes_custody

    def sample_then_release_the_child(interest):
        samples = custody_sample(interest)
        published = pid_file.read_text(encoding="utf-8") if pid_file.exists() else ""
        if published and int(published) in samples:
            sampled.touch()
        return samples

    monkeypatch.setattr(
        memory_guard, "sample_processes_custody", sample_then_release_the_child
    )
    script = (
        "import os, pathlib, subprocess, sys, time\n"
        "grandchild = subprocess.Popen(\n"
        f"    [sys.executable, '-c', {HUNG_CHILD_SOURCE!r}, {marker!r}],\n"
        "    start_new_session=True,\n"
        ")\n"
        f"partial = pathlib.Path({str(pid_file) + '.partial'!r})\n"
        "partial.write_text(str(grandchild.pid))\n"
        f"os.replace(partial, {str(pid_file)!r})\n"
        f"while not pathlib.Path({str(sampled)!r}).exists():\n"
        "    time.sleep(0.01)\n"
    )
    grandchild_pid: int | None = None
    try:
        result = memory_guard.run_guarded(
            [sys.executable, "-c", script],
            max_rss_kb=1_000_000,
            poll_interval=0.05,
        )
        grandchild_pid = _wait_for_pid_file(pid_file)

        assert result.returncode == 0
        assert grandchild_pid in result.orphaned_process_groups
        assert result.descendants_closed is True
        # Closure observed its exit: it runs nothing now.
        assert grandchild_pid not in processes_carrying(marker)
    finally:
        # Found by its marker: run_guarded may raise before the pid is known.
        stop_test_processes_carrying(marker)


@pytest.mark.skipif(os.name != "posix", reason="POSIX process-group census")
def test_run_guarded_terminates_a_group_member_orphaned_between_samples(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    """The child exits right after it starts a grandchild in its own group.

    The guard samples once at launch and then waits on the child's exit, so
    no sample sees the grandchild under a live parent: the kernel reparents
    it at the child's exit. Only the census of the child's group, taken
    before the reap while the group ID is still reserved, puts it under
    custody (HF-146). Without it the group keeps a live member and closure
    stays unproven.

    The child starts the grandchild only after that launch sample. A child
    that started it sooner, as a starved guard allowed at load 58-65, put it
    under parentage custody in the launch sample instead, and the census
    then had nothing new to admit (HF-368).
    """
    monkeypatch.setattr(memory_guard, "DEFAULT_FAST_START_POLL_INTERVAL_SEC", 60.0)
    launch_sampled = tmp_path / "launch-sampled"
    custody_sample = memory_guard.sample_processes_custody

    def sample_then_release_the_child(interest):
        samples = custody_sample(interest)
        launch_sampled.touch()
        return samples

    monkeypatch.setattr(
        memory_guard, "sample_processes_custody", sample_then_release_the_child
    )
    pid_file = tmp_path / "grandchild.pid"
    marker = hung_child_marker()
    script = (
        "import pathlib, subprocess, sys, time\n"
        f"while not pathlib.Path({str(launch_sampled)!r}).exists():\n"
        "    time.sleep(0.01)\n"
        "grandchild = subprocess.Popen(\n"
        f"    [sys.executable, '-c', {HUNG_CHILD_SOURCE!r}, {marker!r}],\n"
        ")\n"
        f"pathlib.Path({str(pid_file)!r}).write_text(str(grandchild.pid))\n"
    )
    grandchild_pid: int | None = None
    try:
        result = memory_guard.run_guarded(
            [sys.executable, "-c", script],
            max_rss_kb=1_000_000,
            poll_interval=60.0,
            timeout=120.0,
        )
        grandchild_pid = _wait_for_pid_file(pid_file)

        assert result.returncode == 0, result.stderr
        assert result.child_process is not None
        # Only the launch sample ran; it preceded the grandchild's birth.
        assert result.sampling_telemetry is not None
        assert result.sampling_telemetry.attempts == 1
        assert result.child_process.pid in result.orphaned_process_groups
        assert result.descendants_closed is True
        assert result.temporary_artifacts is not None
        census = result.temporary_artifacts["closure"]["root_exit_census"]
        assert census == {"admitted_pids": [grandchild_pid]}
        # Closure observed its exit: it runs nothing now.
        assert grandchild_pid not in processes_carrying(marker)
    finally:
        # Found by its marker: run_guarded may raise before the pid is known.
        stop_test_processes_carrying(marker)


@pytest.mark.skipif(os.name != "posix", reason="POSIX owned-child reap")
def test_timeout_closure_survives_a_late_reap_of_the_killed_child(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A loaded host can reap the killed child late (HF-155).

    Until the reap XNU refuses every signal and probe to the child's group
    with EPERM. Each observation window must keep watching through that
    refusal; ending it early left closure unproven on hosted macOS.
    """
    real_wait4 = os.wait4

    def late_wait4(pid: int, options: int):
        time.sleep(0.5)  # The reaper thread is starved for half a second.
        return real_wait4(pid, options)

    # The view confines the late reap to the clock under test.
    install_module_view(monkeypatch, "os", os, process_custody, wait4=late_wait4)
    result = memory_guard.run_guarded(
        [sys.executable, "-c", HUNG_CHILD_SOURCE],
        max_rss_kb=1_000_000,
        poll_interval=0.05,
        timeout=0.5,
    )

    assert result.timed_out is True
    assert result.descendants_closed is True, result.stderr
    assert result.infrastructure_failure is None
    (report,) = result.termination_reports
    assert report.reason == "timeout"
    assert report.remaining_pgids == report.remaining_pids == ()


@pytest.mark.skipif(os.name != "posix", reason="POSIX sampled process-tree RSS")
def test_run_guarded_enforces_tree_rss_across_a_new_session_grandchild(
    tmp_path: Path,
) -> None:
    """Neither process crosses the per-process limit; together they cross the
    tree limit. The grandchild's RSS counts only if the sampler keeps it in
    the tree after it leaves the child's session."""
    pid_file = tmp_path / "grandchild.pid"
    marker = hung_child_marker()
    allocate = (
        "buf = bytearray(120 * 1024 * 1024); buf[::4096] = b'x' * len(buf[::4096])"
    )
    held = f"{allocate}\n{HUNG_CHILD_SOURCE}"
    script = (
        "import pathlib, subprocess, sys\n"
        "grandchild = subprocess.Popen(\n"
        f"    [sys.executable, '-c', {held!r}, {marker!r}],\n"
        "    start_new_session=True,\n"
        ")\n"
        f"pathlib.Path({str(pid_file)!r}).write_text(str(grandchild.pid))\n"
        f"{held}"
    )
    grandchild_pid: int | None = None
    try:
        result = memory_guard.run_guarded(
            [sys.executable, "-c", script],
            max_rss_kb=1024 * 1024,
            max_total_rss_kb=180 * 1024,
            poll_interval=0.05,
            child_rlimit_kb=None,
            timeout=30.0,
        )
        grandchild_pid = _wait_for_pid_file(pid_file)

        assert result.returncode == memory_guard.GUARD_RETURN_CODE
        assert result.violation is not None
        assert result.violation.scope == "process_tree"
        assert result.violation.rss_kb > 180 * 1024
        # Closure observed its exit: it runs nothing now.
        assert grandchild_pid not in processes_carrying(marker)
    finally:
        # Found by its marker: run_guarded may raise before the pid is known.
        stop_test_processes_carrying(marker)


@pytest.mark.parametrize("tracked_cleanup_signalled", [False, True])
def test_run_guarded_reuses_its_post_exit_snapshot_until_a_signal(
    monkeypatch: pytest.MonkeyPatch,
    tracked_cleanup_signalled: bool,
) -> None:
    """One post-exit observation serves cleanup and closure; a signal retires it."""
    spawned: list[int] = []
    seen: dict[str, object] = {}
    report = _guard_termination_report(reason="tracked_orphan_cleanup")

    def child_only_sampler() -> Mapping[int, memory_guard.ProcessSample]:
        return {
            pid: sample
            for pid, sample in memory_guard.sample_processes().items()
            if pid in spawned
        }

    def tracked_cleanup(root_pid, **kwargs):
        seen["tracked"] = kwargs["samples"]
        return memory_guard.GuardOrphanCleanupResult(
            termination_reports=(report,) if tracked_cleanup_signalled else ()
        )

    def repo_cleanup(**kwargs):
        seen["repo"] = kwargs["samples"]
        return memory_guard.GuardOrphanCleanupResult()

    def closure(**kwargs):
        seen["closure"] = kwargs["final_samples"]
        return True, {"schema": "molt.guard-scratch-closure.v1", "closed": True}

    monkeypatch.setattr(memory_guard, "cleanup_tracked_orphans", tracked_cleanup)
    monkeypatch.setattr(
        memory_guard, "cleanup_repo_scoped_orphans_since_baseline", repo_cleanup
    )
    monkeypatch.setattr(memory_guard, "_temporary_artifact_descendant_closure", closure)
    monkeypatch.setattr(memory_guard._win_job, "create_kill_on_close_job", lambda: None)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "pass"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        sampler=child_only_sampler,
        on_spawn=spawned.append,
    )

    assert result.returncode == 0
    assert seen["tracked"] is not None
    if tracked_cleanup_signalled:
        assert seen["repo"] is None
        assert seen["closure"] is None
    else:
        assert seen["repo"] is seen["tracked"]
        assert seen["closure"] is seen["tracked"]


def test_successful_cargo_orphan_cleanup_does_not_quarantine_incremental(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    result, calls, report = _run_guarded_cargo_with_fake_orphan_cleanup(
        monkeypatch,
        tmp_path,
        exit_code=0,
    )

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert result.orphaned_process_groups == (777,)
    assert result.termination_reports == (report,)
    assert result.cargo_incremental_quarantine is None
    assert calls == []


def test_failed_cargo_orphan_cleanup_quarantines_incremental(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    result, calls, _report = _run_guarded_cargo_with_fake_orphan_cleanup(
        monkeypatch,
        tmp_path,
        exit_code=3,
    )

    assert result.returncode == 3
    assert result.orphaned_process_groups == (777,)
    assert result.cargo_incremental_quarantine is not None
    assert result.cargo_incremental_quarantine.reason == "orphaned_processes_cleaned"
    assert [call["reason"] for call in calls] == ["orphaned_processes_cleaned"]


def test_main_enforces_timeout_and_writes_summary(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    summary_path = tmp_path / "timeout-summary.json"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--timeout",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            HUNG_CHILD_SOURCE,
        ]
    )

    assert rc == memory_guard.TIMEOUT_RETURN_CODE
    assert "timeout after" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == memory_guard.TIMEOUT_RETURN_CODE
    assert payload["timed_out"] is True
    assert payload["violation"] is None
    assert payload["exit_signal"] is None
    assert payload["incident"]["reason"] == "timeout"
    assert payload["incident"]["cleanup"].startswith("terminated tracked process tree")


def test_main_writes_summary_when_guard_parent_receives_sigterm(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    summary_path = tmp_path / "guard-sigterm-summary.json"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--timeout",
            "5",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "import os, signal\nos.kill(os.getppid(), signal.SIGTERM)\n"
            + HUNG_CHILD_SOURCE,
        ]
    )

    assert rc == 143
    assert "guard parent received SIGTERM" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 143
    assert payload["timed_out"] is False
    assert payload["violation"] is None
    assert payload["exit_signal"] is None
    assert payload["guard_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "guard_interrupted"
    # HF-88: on hosted macOS this cleanup was once "process cleanup
    # incomplete" while the child was exiting; name the unverified action.
    assert payload["incident"]["cleanup"] == "terminated tracked process tree", (
        json.dumps(payload["incident"], indent=2, sort_keys=True)
    )
    assert payload["incident"]["signal"] == payload["guard_signal"]


def test_run_guarded_restores_signal_handlers_after_post_launch_exception() -> None:
    if memory_guard.os.name != "posix":
        pytest.skip("requires POSIX process custody")
    watched_signals = [
        sig
        for sig in (
            getattr(signal, "SIGTERM", None),
            getattr(signal, "SIGINT", None),
            getattr(signal, "SIGHUP", None),
        )
        if sig is not None
    ]
    previous_handlers = {sig: signal.getsignal(sig) for sig in watched_signals}
    sampler_calls = 0

    def failing_sampler():
        nonlocal sampler_calls
        sampler_calls += 1
        if sampler_calls == 1:
            return {}
        raise RuntimeError("injected sampler failure")

    with pytest.raises(RuntimeError, match="injected sampler failure"):
        memory_guard.run_guarded(
            [sys.executable, "-c", "import time; time.sleep(5)"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
            timeout=5,
            sampler=failing_sampler,
        )

    assert sampler_calls >= 2
    assert {sig: signal.getsignal(sig) for sig in watched_signals} == previous_handlers


@pytest.mark.parametrize("phase", ["running", "terminal"])
@pytest.mark.parametrize("defect", ["missing", "unexpected"])
def test_guard_report_context_rejects_missing_or_unknown_fields_before_write(
    tmp_path: Path, phase: str, defect: str
) -> None:
    path = tmp_path / "invalid-summary.json"
    context = {
        "command": ["subject"],
        "cwd": None,
        "environ": {},
        "max_rss_kb": 1024,
        "max_total_rss_kb": None,
        "max_global_rss_kb": None,
        "child_rlimit_kb": None,
        "timeout_s": None,
        "poll_interval_s": 0.1,
    }
    if defect == "missing":
        del context["max_rss_kb"]
    else:
        context["max_rs_kb"] = 1024
    with pytest.raises(TypeError, match="invalid guard report context"):
        if phase == "running":
            memory_guard._write_running_summary_json(str(path), **context)
        else:
            memory_guard._write_summary_json(
                str(path),
                result=memory_guard.GuardResult(
                    0,
                    None,
                    None,
                    None,
                    "",
                    "",
                    child_stderr="",
                ),
                **context,
            )
    assert not path.exists()


def test_summary_json_keeps_rss_incident_primary_when_guard_signal_is_secondary(
    tmp_path,
) -> None:
    summary_path = tmp_path / "rss-plus-guard-signal.json"
    violation = memory_guard.RssViolation(
        pid=123,
        rss_kb=2_000_000,
        command="python worker.py",
        scope="process",
    )

    memory_guard._write_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        result=memory_guard.GuardResult(
            returncode=memory_guard.GUARD_RETURN_CODE,
            violation=violation,
            peak=violation,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.1,
            guard_signal=signal.SIGTERM,
            child_stderr="",
        ),
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["exit_signal"] is None
    assert payload["guard_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "rss_limit_exceeded"
    assert payload["incident"]["guard_signal"] == payload["guard_signal"]


def test_summary_json_reports_incomplete_sampling_without_fabricating_incident(
    tmp_path: Path,
) -> None:
    summary_path = tmp_path / "sampling-gap.json"
    telemetry = memory_guard.GuardSamplingTelemetry(
        attempts=7,
        successes=6,
        transient_failures=1,
        first_transient_failure_at="2026-07-18T15:24:00Z",
        last_transient_failure_at="2026-07-18T15:24:00Z",
        last_transient_error="snapshot deadline",
    )

    memory_guard._write_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        result=memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=1.0,
            sampling_telemetry=telemetry,
            child_stderr="",
        ),
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["incident"] is None
    assert payload["sampling_telemetry"] == {
        "attempts": 7,
        "successes": 6,
        "transient_failures": 1,
        "enforcement_complete": False,
        "first_transient_failure_at": "2026-07-18T15:24:00Z",
        "last_transient_failure_at": "2026-07-18T15:24:00Z",
        "last_transient_error": "snapshot deadline",
        "source": "unknown",
        "wall_time_s": 0.0,
        "cpu_time_s": 0.0,
        "max_wall_time_s": 0.0,
        "max_cpu_time_s": 0.0,
        "process_rows": 0,
        "max_process_rows": 0,
        "observer_wall_time_s": 0.0,
        "observer_cpu_time_s": 0.0,
        "observer_cpu_duty_cycle": None,
    }


def test_summary_json_keeps_timeout_primary_when_guard_signal_is_secondary(
    tmp_path,
) -> None:
    summary_path = tmp_path / "timeout-plus-guard-signal.json"

    memory_guard._write_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        result=memory_guard.GuardResult(
            returncode=memory_guard.TIMEOUT_RETURN_CODE,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            timed_out=True,
            elapsed_s=5.0,
            guard_signal=signal.SIGTERM,
            child_stderr="",
        ),
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["exit_signal"] is None
    assert payload["incident"]["reason"] == "timeout"
    assert payload["incident"]["guard_signal"] == payload["guard_signal"]


def test_main_writes_running_summary_before_launch_result(
    tmp_path, monkeypatch
) -> None:
    summary_path = tmp_path / "running-summary.json"

    def fake_run_guarded(_command, **_kwargs):
        assert _kwargs["running_summary_json"] == str(summary_path)
        payload = json.loads(summary_path.read_text(encoding="utf-8"))
        assert payload["status"] == "running"
        assert payload["returncode"] is None
        assert payload["child_process"] is None
        assert payload["incident"]["reason"] == "guard_started"
        assert payload["repro"]["summary_json"] == str(summary_path)
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.1,
            child_stderr="",
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    final_payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert final_payload["returncode"] == 0
    assert "status" not in final_payload


def test_running_summary_refresh_records_spawned_child_process(tmp_path) -> None:
    summary_path = tmp_path / "running-summary-child.json"
    child = memory_guard.GuardedChildProcess(
        pid=1234,
        pgid=None,
        sid=None,
        command=(sys.executable, "-c", "pass"),
        started_at="2026-07-02T17:03:11Z",
    )

    memory_guard._write_running_summary_json(
        str(summary_path),
        command=[sys.executable, "-c", "pass"],
        cwd=None,
        environ={},
        max_rss_kb=1_000_000,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=5,
        poll_interval_s=0.01,
        child_process=child,
    )

    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["status"] == "running"
    assert payload["child_process"]["pid"] == 1234
    assert payload["child_process"]["command"] == [sys.executable, "-c", "pass"]
    assert payload["incident"]["reason"] == "child_running"
    assert payload["repro"]["summary_json"] == str(summary_path)


def test_main_reports_signal_status_without_guard_violation(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "signal-summary.json"

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=143,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
            child_stderr="",
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(143)",
        ]
    )

    assert rc == 143
    assert "SIGTERM status" in capsys.readouterr().err
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 143
    assert payload["child_rlimit_gb"] == pytest.approx(1.0)
    assert payload["timed_out"] is False
    assert payload["violation"] is None
    assert payload["exit_signal"] == {
        "signal": 15,
        "name": "SIGTERM",
        "conventional_shell_status": True,
    }
    assert payload["incident"]["reason"] == "signal_exit"
    assert payload["incident"]["elapsed_s"] == pytest.approx(0.3)


def test_main_reports_guard_signal_name_from_guard_signal_not_returncode(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "rss-plus-guard-signal-summary.json"
    violation = memory_guard.RssViolation(
        pid=123,
        rss_kb=2_000_000,
        command="python worker.py",
        scope="process",
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=137,
            violation=violation,
            peak=violation,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
            guard_signal=signal.SIGTERM,
            child_stderr="",
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "raise SystemExit(137)",
        ]
    )

    assert rc == 137
    stderr = capsys.readouterr().err
    assert "guard parent received SIGTERM" in stderr
    assert "guard parent received SIGKILL" not in stderr
    assert "not classified as an RSS limit trip" not in stderr
    assert "RSS limit incident remains the primary classification" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 137
    assert payload["guard_signal"]["name"] == "SIGTERM"
    assert payload["incident"]["reason"] == "rss_limit_exceeded"


def test_main_reports_cargo_incremental_quarantine_summary(
    tmp_path, capsys: pytest.CaptureFixture[str], monkeypatch
) -> None:
    summary_path = tmp_path / "signal-summary.json"
    target = tmp_path / "target"
    quarantine = target / ".molt_state" / "quarantine" / "cargo_incremental" / "q"
    receipt = memory_guard.CargoIncrementalQuarantine(
        reason="signal_exit",
        recorded_at="2026-06-12T00:00:00Z",
        target_dir=str(target),
        quarantine_dir=str(quarantine),
        command=("cargo", "test"),
        cwd=str(tmp_path),
        moved_paths=(
            memory_guard.CargoIncrementalQuarantineMove(
                original_path=str(target / "debug" / "incremental"),
                quarantined_path=str(quarantine / "debug" / "incremental"),
            ),
        ),
        receipt_path=str(quarantine / "receipt.json"),
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=143,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.3,
            cargo_incremental_quarantine=receipt,
            child_stderr="",
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            "cargo",
            "test",
        ]
    )

    assert rc == 143
    stderr = capsys.readouterr().err
    assert "quarantined Cargo incremental state after signal_exit" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["cargo_incremental_quarantine"]["reason"] == "signal_exit"
    assert payload["cargo_incremental_quarantine"]["target_dir"] == str(target)
    assert len(payload["cargo_incremental_quarantine"]["moved_paths"]) == 1
    assert payload["incident"]["cleanup"] == "quarantined Cargo incremental state"


def test_main_reports_incident_repro_context(
    tmp_path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch,
) -> None:
    summary_path = tmp_path / "rss-summary.json"
    current_root = tmp_path / "pytest-memory-guard"
    current_test_path = current_root / "pytest-current-test.json"
    current_root.mkdir(parents=True)
    current_test_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "nodeid": "tests/test_memory_guard_tool.py::live_unit",
                "phase": "call",
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    env = {
        "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
        "CARGO_BUILD_JOBS": "2",
        "CARGO_INCREMENTAL": "1",
        "PATH": "/usr/bin",
        "PYTEST_CURRENT_TEST": "tests/test_memory_guard_tool.py::unit (call)",
        "MOLT_PYTEST_CURRENT_TEST_FILE": str(current_test_path),
        "MOLT_SESSION_ID": "unit-session",
        "SECRET_TOKEN": "must-not-leak",
        "UV_LINK_MODE": "copy",
        "UV_PROJECT_ENVIRONMENT": str(tmp_path / "uv-project-env"),
    }

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=memory_guard.GUARD_RETURN_CODE,
            violation=memory_guard.RssViolation(
                pid=321,
                rss_kb=4 * 1024 * 1024,
                command="python hungry.py",
                scope="process_tree",
            ),
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=1.25,
            limit_at_violation=memory_guard.ResolvedMemoryLimits(
                max_process_rss_kb=2 * 1024 * 1024,
                max_total_rss_kb=3 * 1024 * 1024,
            ),
            child_stderr="",
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "2",
            "--max-total-rss-gb",
            "3",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "pass",
        ],
        environ=env,
    )

    assert rc == memory_guard.GUARD_RETURN_CODE
    stderr = capsys.readouterr().err
    assert "memory_guard: repro context:" in stderr
    assert "tests/test_memory_guard_tool.py::unit" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    repro = payload["repro"]
    assert repro["command"] == [sys.executable, "-c", "pass"]
    assert repro["pytest"]["current_test"] == env["PYTEST_CURRENT_TEST"]
    assert (
        repro["pytest"]["current_test_file"]["payload"]["nodeid"]
        == "tests/test_memory_guard_tool.py::live_unit"
    )
    assert repro["env"]["MOLT_SESSION_ID"] == "unit-session"
    assert repro["env"]["CARGO_BUILD_JOBS"] == "2"
    assert repro["env"]["CARGO_INCREMENTAL"] == "1"
    assert repro["env"]["UV_LINK_MODE"] == "copy"
    assert repro["env"]["UV_PROJECT_ENVIRONMENT"] == str(tmp_path / "uv-project-env")
    assert "SECRET_TOKEN" not in repro["env"]
    assert repro["limits"]["max_total_rss_gb"] == pytest.approx(3.0)

    env_delta = memory_guard._safe_repro_env_delta(
        env,
        baseline={
            "CARGO_BUILD_JOBS": "8",
            "CARGO_INCREMENTAL": "0",
            "UV_PROJECT_ENVIRONMENT": str(tmp_path / "old-uv-project-env"),
            "SECRET_TOKEN": "baseline-secret",
        },
    )
    assert env_delta["changed"]["CARGO_BUILD_JOBS"] == {"from": "8", "to": "2"}
    assert env_delta["changed"]["CARGO_INCREMENTAL"] == {"from": "0", "to": "1"}
    assert env_delta["changed"]["UV_PROJECT_ENVIRONMENT"] == {
        "from": str(tmp_path / "old-uv-project-env"),
        "to": str(tmp_path / "uv-project-env"),
    }
    assert "SECRET_TOKEN" not in env_delta["changed"]


def test_repro_context_platform_detail_does_not_spawn_subprocess(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    def forbidden_platform_detail() -> str:
        raise AssertionError("platform.platform must not run in summary emission")

    # A tripwire must catch this call from any module, so it stays process-wide.
    # Only this thread meets the fake; other threads reach the real function.
    monkeypatch.setattr(
        platform,
        "platform",
        current_thread_only(forbidden_platform_detail, platform.platform),
    )

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-c", "pass"],
        cwd=tmp_path,
        environ={},
    )

    assert repro["host"]["platform"] == sys.platform
    assert repro["host"]["platform_detail"]


@pytest.mark.parametrize(
    ("record_pid", "matches"),
    [(4321, True), ("4321", False), (4321.0, False), (True, False), (None, False)],
)
def test_repro_context_reads_xdist_worker_current_test_sidecars(
    tmp_path: Path,
    monkeypatch,
    record_pid: object,
    matches: bool,
) -> None:
    current_root = tmp_path / "pytest-memory-guard"
    aggregate_path = current_root / "pytest-current-test.json"
    worker_dir = aggregate_path.with_name(f"{aggregate_path.name}.d")
    worker_dir.mkdir(parents=True)
    worker_path = worker_dir / "gw0-4321_current-test.json"
    worker_path.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "pid": record_pid,
                "nodeid": "tests/test_xdist.py::test_memory",
                "phase": "call",
                "xdist_worker": "gw0",
            },
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    monkeypatch.setattr(
        memory_guard,
        "sample_processes",
        lambda: {
            4321: memory_guard.ProcessSample(
                pid=4321,
                ppid=100,
                rss_kb=1,
                command="pytest worker gw0",
            ),
            9876: memory_guard.ProcessSample(
                pid=9876,
                ppid=4321,
                rss_kb=4 * 1024 * 1024,
                command="python hungry.py",
            ),
        },
    )

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest", "-n", "2"],
        cwd=tmp_path,
        environ={
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
            "MOLT_PYTEST_CURRENT_TEST_FILE": str(aggregate_path),
            "PYTEST_XDIST_WORKER": "",
        },
        incident_pid=9876,
    )

    current_test = repro["pytest"]["current_test_file"]
    assert current_test["missing"] is True
    records = current_test["worker_records"]
    assert records[0].get("incident_match") == ("pid_lineage" if matches else None)
    assert records[0]["payload"]["nodeid"] == "tests/test_xdist.py::test_memory"


def test_repro_context_rejects_noncanonical_current_test_file(
    tmp_path: Path,
    monkeypatch,
) -> None:
    current_root = tmp_path / "pytest-memory-guard"
    outside_path = tmp_path / "outside" / "pytest-current-test.json"
    outside_path.parent.mkdir()
    outside_path.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {})

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={
            "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
            "MOLT_PYTEST_CURRENT_TEST_FILE": str(outside_path),
        },
    )

    current_test = repro["pytest"]["current_test_file"]
    assert current_test["rejected"] == "noncanonical"
    assert current_test["canonical_root"] == str(current_root)


def test_repro_context_includes_bounded_host_control_plane(
    monkeypatch, tmp_path: Path
) -> None:
    long_command = "/Applications/Codex.app/Contents/MacOS/Codex " + ("x" * 800)
    samples = {
        10: memory_guard.ProcessSample(
            pid=10,
            ppid=1,
            pgid=10,
            rss_kb=500_000,
            command=long_command,
        ),
        11: memory_guard.ProcessSample(
            pid=11,
            ppid=10,
            pgid=10,
            rss_kb=200_000,
            command="/Users/adpena/Projects/molt/target/release-fast/molt-backend",
        ),
        999: memory_guard.ProcessSample(
            pid=999,
            ppid=10,
            pgid=999,
            rss_kb=10,
            command="python tools/memory_guard.py",
        ),
    }
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpid=lambda: 999,
        getppid=lambda: 10,
    )
    monkeypatch.setattr(memory_guard, "_safe_getpgrp", lambda: 999)

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={},
    )

    host = repro["host_control_plane"]
    assert host["host_pgids"] == [10]
    assert 10 in host["protected_pgids"]
    assert host["samples"][0]["pid"] == 10
    assert host["samples"][0]["command"].endswith("...<truncated>")
    assert len(host["samples"][0]["command"]) < len(long_command)


def test_repro_census_binds_argv_only_for_the_guards_groups(
    monkeypatch, tmp_path: Path
) -> None:
    """HF-368: a report pays for the guard's groups, never the host's rows."""
    bound: list[int] = []

    def native(pid: int, ppid: int, pgid: int, command: str) -> object:
        def bind() -> tuple[str, tuple[str, ...], str]:
            bound.append(pid)
            return command, tuple(command.split()), "full"

        return process_model.native_process_sample(
            pid=pid,
            ppid=ppid,
            rss_kb=1,
            pgid=pgid,
            elapsed_sec=1,
            started_at_ns=pid,
            bind_command=bind,
        )

    codex = "/Applications/Codex.app/Contents/MacOS/Codex"
    samples = {
        10: native(10, 1, 10, codex),
        11: native(11, 10, 10, "/opt/molt/target/release-fast/molt-backend"),
        999: native(999, 10, 999, "python tools/memory_guard.py"),
        1000: native(1000, 999, 1000, "python -c pass"),
        # Unrelated host rows: another control-plane app and an ordinary
        # process. Neither lies on the guard's or the incident's ancestry.
        500: native(500, 1, 500, "/Applications/Claude.app/Contents/MacOS/Claude"),
        501: native(501, 1, 501, "/usr/sbin/unrelated-daemon"),
    }
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    _fake_guard(monkeypatch, "os", os, getpid=lambda: 999, getppid=lambda: 10)
    monkeypatch.setattr(memory_guard, "_safe_getpgrp", lambda: 999)

    repro = memory_guard.repro_context_payload(
        command=[sys.executable, "-m", "pytest"],
        cwd=tmp_path,
        environ={},
        incident_pid=1000,
    )

    host = repro["host_control_plane"]
    assert host["scope_pgids"] == [10, 999, 1000]
    assert host["host_pgids"] == [10]
    assert 500 not in host["protected_pgids"]
    assert [sample["pid"] for sample in host["samples"]] == [10, 11]
    assert not {500, 501} & set(bound)
    assert {10, 1000} <= set(bound)


def test_main_rejects_unsafe_threshold(capsys: pytest.CaptureFixture[str]) -> None:
    rc = memory_guard.main(["--max-rss-gb", "112", "--", sys.executable, "-c", "pass"])

    assert rc == 2
    assert "below 112" in capsys.readouterr().err


def test_main_rejects_unsafe_total_threshold(
    capsys: pytest.CaptureFixture[str],
) -> None:
    rc = memory_guard.main(
        ["--max-total-rss-gb", "112", "--", sys.executable, "-c", "pass"]
    )

    assert rc == 2
    assert "below 112" in capsys.readouterr().err


def test_parser_accepts_process_and_tree_rss_aliases() -> None:
    args = memory_guard._parser().parse_args(
        [
            "--max-process-rss-gb",
            "1.5",
            "--max-tree-rss-gb",
            "2.5",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )
    group_args = memory_guard._parser().parse_args(
        [
            "--max-group-rss-gb",
            "3.5",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )

    assert args.max_rss_gb == 1.5
    assert args.max_total_rss_gb == 2.5
    assert group_args.max_total_rss_gb == 3.5


def test_main_reexec_hides_guarded_command_from_guard_argv(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    marker = "molt-backend-marker"
    captured: dict[str, object] = {}
    stdio = {"stdin": "in", "stdout": "out", "stderr": "err"}

    def fake_execve(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(73)

    def fake_subprocess_run(argv, *, env, check, **kwargs):
        assert check is False
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        captured["run_kwargs"] = dict(kwargs)
        return subprocess.CompletedProcess(argv, 73)

    main_argv = [
        "--max-rss-gb",
        "1",
        "--poll-interval",
        "0.01",
        "--",
        sys.executable,
        "-c",
        f"print({marker!r})",
    ]
    if os.name == "nt":
        monkeypatch.setattr(memory_guard, "inherit_stdio_kwargs", lambda: stdio)
        _fake_guard(monkeypatch, "subprocess", subprocess, run=fake_subprocess_run)
        assert (
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
            == 73
        )
    else:
        with pytest.raises(SystemExit) as exc:
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
        assert exc.value.code == 73
    worker_argv = captured["argv"]
    assert isinstance(worker_argv, list)
    assert all(marker not in arg for arg in worker_argv)
    env = captured["env"]
    assert isinstance(env, dict)
    encoded = env[memory_guard.INTERNAL_COMMAND_ENV]
    assert json.loads(encoded) == [sys.executable, "-c", f"print({marker!r})"]
    assert env[memory_guard.INTERNAL_WORKER_ENV] == "1"
    if os.name == "nt":
        run_kwargs = captured["run_kwargs"]
        assert isinstance(run_kwargs, dict)
        assert run_kwargs["creationflags"] == (
            getattr(memory_guard.subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            | getattr(memory_guard.subprocess, "CREATE_NO_WINDOW", 0)
        )
        assert run_kwargs["stdin"] == "in"
        assert run_kwargs["stdout"] == "out"
        assert run_kwargs["stderr"] == "err"


@pytest.mark.parametrize("temproot_mode", ["managed", "explicit", "unset"])
def test_run_guarded_marks_child_environment_as_guarded(
    request, tmp_path, temproot_mode
) -> None:
    if run_in_fresh_guard_observer(request):
        return
    env = dict(os.environ)
    env["MOLT_GUARD_SCRATCH_ROOT"] = str(tmp_path / "outer-scratch")
    if temproot_mode == "managed":
        env["PYTEST_DEBUG_TEMPROOT"] = env["MOLT_GUARD_SCRATCH_ROOT"]
    elif temproot_mode == "explicit":
        env["PYTEST_DEBUG_TEMPROOT"] = str(tmp_path / "explicit")
    else:
        env.pop("PYTEST_DEBUG_TEMPROOT", None)
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import json, os, pathlib; "
                "marker = pathlib.Path(os.environ['MOLT_MEMORY_GUARD_MARKER']); "
                "payload = json.loads(marker.read_text()); "
                "print(os.environ.get('MOLT_MEMORY_GUARD_ACTIVE')); "
                "print(bool(os.environ.get('MOLT_MEMORY_GUARD_PID'))); "
                "print(bool(os.environ.get('MOLT_MEMORY_GUARD_TOKEN'))); "
                "print(bool(os.environ.get('MOLT_GUARD_SCRATCH_ROOT'))); "
                "print(marker.exists()); "
                "print(payload['pid'] == int(os.environ['MOLT_MEMORY_GUARD_PID'])); "
                "print(payload['token'] == os.environ['MOLT_MEMORY_GUARD_TOKEN']); "
                "print(json.dumps({name: os.environ.get(name) for name in "
                "('PYTEST_DEBUG_TEMPROOT', 'MOLT_GUARD_SCRATCH_ROOT')}))"
            ),
        ],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=None,
        env=env,
    )

    assert result.returncode == 0
    lines = result.stdout.splitlines()
    assert lines[:-1] == [
        "1",
        "True",
        "True",
        "True",
        "True",
        "True",
        "True",
    ]
    selected = json.loads(lines[-1])
    assert selected["MOLT_GUARD_SCRATCH_ROOT"] != env["MOLT_GUARD_SCRATCH_ROOT"]
    assert selected["PYTEST_DEBUG_TEMPROOT"] == (
        selected["MOLT_GUARD_SCRATCH_ROOT"]
        if temproot_mode == "managed"
        else env.get("PYTEST_DEBUG_TEMPROOT")
    )
    assert result.temporary_artifacts is not None
    assert result.temporary_artifacts["state"] == "reclaimed"
    finalize_elapsed = result.temporary_artifacts["finalize_elapsed_s"]
    assert isinstance(finalize_elapsed, float)
    assert finalize_elapsed >= 0.0
    assert result.elapsed_s is not None and result.elapsed_s >= finalize_elapsed


@pytest.mark.parametrize(
    ("child_returncode", "expected_returncode"),
    [(0, memory_guard.INFRASTRUCTURE_RETURN_CODE), (7, 7), (137, 137)],
)
def test_run_guarded_scratch_cleanup_failure_preserves_primary_result(
    request: pytest.FixtureRequest,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    child_returncode: int,
    expected_returncode: int,
) -> None:
    if run_in_fresh_guard_observer(request):
        return

    class FakeLease:
        target = tmp_path / "guard-scratch"
        generation = tmp_path / "guard-generation"
        release_calls = 0

        def release(self) -> None:
            self.release_calls += 1

    lease = FakeLease()
    lease.target.mkdir()
    lease.generation.mkdir()
    finish_error_receipt = lease.generation / "finish-error.json"
    finish_error_receipt.write_text("{}", encoding="utf-8")
    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "acquire_guard_scratch",
        lambda _root, _env: lease,
    )

    def fail_finish(*_args: object, **_kwargs: object) -> Mapping[str, object]:
        raise OSError("scratch cleanup failed")

    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "finish_guard_scratch",
        fail_finish,
    )
    monkeypatch.setattr(
        memory_guard,
        "_temporary_artifact_descendant_closure",
        lambda **_kwargs: (
            True,
            {
                "schema": "molt.guard-scratch-closure.v1",
                "authority": "test-closed",
                "closed": True,
            },
        ),
    )
    monkeypatch.setattr(
        memory_guard._win_job,
        "create_kill_on_close_job",
        lambda: None,
    )
    env = dict(os.environ)
    env["MOLT_MEMORY_GUARD_STATE_ROOT"] = str(tmp_path / "memory_guard")

    result = memory_guard.run_guarded(
        [sys.executable, "-c", f"raise SystemExit({child_returncode})"],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=None,
        cleanup_orphans=False,
        env=env,
    )

    assert result.returncode == expected_returncode
    assert result.child_returncode == child_returncode
    assert result.infrastructure_failure is not None
    assert result.infrastructure_failure.phase == "temporary_artifact_custody"
    assert result.temporary_artifacts is not None
    assert result.temporary_artifacts["state"] == "cleanup-error"
    assert result.temporary_artifacts["error"] == ("OSError: scratch cleanup failed")
    assert result.temporary_artifacts["receipt"] == str(finish_error_receipt)
    finalize_elapsed = result.temporary_artifacts["finalize_elapsed_s"]
    assert isinstance(finalize_elapsed, float)
    assert finalize_elapsed >= 0.0
    assert result.elapsed_s is not None and result.elapsed_s >= finalize_elapsed
    assert "temporary artifact custody incomplete" in result.stderr
    assert lease.release_calls == 1


@pytest.mark.parametrize(
    ("child_returncode", "retention_errors", "expected_returncode"),
    [
        (
            0,
            ["prior generation receipt unreadable"],
            memory_guard.INFRASTRUCTURE_RETURN_CODE,
        ),
        (7, ["prior generation receipt unreadable"], 7),
        (137, ["prior generation receipt unreadable"], 137),
        (0, [], 0),
    ],
)
def test_run_guarded_retention_sweep_health_preserves_primary_result(
    request: pytest.FixtureRequest,
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    child_returncode: int,
    retention_errors: list[str],
    expected_returncode: int,
) -> None:
    if run_in_fresh_guard_observer(request):
        return

    class FakeLease:
        target = tmp_path / "guard-scratch"
        release_calls = 0

        def release(self) -> None:
            self.release_calls += 1

    lease = FakeLease()
    lease.target.mkdir()
    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "acquire_guard_scratch",
        lambda _root, _env: lease,
    )
    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "finish_guard_scratch",
        lambda *_args, **_kwargs: {
            "state": "reclaimed",
            "receipt": str(tmp_path / "owner.json"),
            "retention": {
                "reclaimed": [],
                "retained_bytes": 0,
                "retained_count": 0,
                "protected_count": 3,
                "errors": retention_errors,
            },
        },
    )
    monkeypatch.setattr(
        memory_guard,
        "_temporary_artifact_descendant_closure",
        lambda **_kwargs: (
            True,
            {
                "schema": "molt.guard-scratch-closure.v1",
                "authority": "test-closed",
                "closed": True,
            },
        ),
    )
    monkeypatch.setattr(
        memory_guard._win_job,
        "create_kill_on_close_job",
        lambda: None,
    )
    env = dict(os.environ)
    env["MOLT_MEMORY_GUARD_STATE_ROOT"] = str(tmp_path / "memory_guard")

    result = memory_guard.run_guarded(
        [sys.executable, "-c", f"raise SystemExit({child_returncode})"],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=None,
        cleanup_orphans=False,
        env=env,
    )

    assert result.returncode == expected_returncode
    assert result.child_returncode == child_returncode
    assert result.temporary_artifacts is not None
    assert result.temporary_artifacts["state"] == "reclaimed"
    assert result.temporary_artifacts["retention"]["protected_count"] == 3
    if retention_errors:
        assert result.infrastructure_failure is not None
        assert result.infrastructure_failure.phase == "temporary_artifact_custody"
        incident = memory_guard._incident_payload(result)
        assert incident["reason"] == "infrastructure_error"
        assert incident["child_returncode"] == child_returncode
        assert "signal" not in incident
        assert "prior generation receipt unreadable" in result.stderr
        assert "retention sweep reported errors" in result.stderr
    else:
        assert result.infrastructure_failure is None
        assert memory_guard._incident_payload(result) is None
        assert "temporary artifact custody incomplete" not in result.stderr
    assert lease.release_calls == 1

    summary = tmp_path / "outcome.json"
    monkeypatch.setattr(memory_guard, "repro_context_payload", lambda **_: {})
    memory_guard._write_summary_json(
        str(summary),
        result=result,
        command=["fixture-child"],
        cwd=tmp_path,
        environ={},
        max_rss_kb=512 * 1024,
        max_total_rss_kb=None,
        max_global_rss_kb=None,
        child_rlimit_kb=None,
        timeout_s=None,
        poll_interval_s=0.01,
    )
    payload = json.loads(summary.read_text(encoding="utf-8"))
    assert payload["returncode"] == expected_returncode
    assert payload["child_returncode"] == child_returncode
    assert payload[
        "infrastructure_failure"
    ] == memory_guard.infrastructure_failure_payload(result.infrastructure_failure)
    assert payload["exit_signal"] == memory_guard.exit_signal_payload(child_returncode)
    diagnostic = io.StringIO()
    memory_guard._reporting.emit_terminal_report(
        result,
        timeout_s=None,
        max_rss_gb=0.5,
        max_total_rss_gb=1.0,
        repro_payload=None,
        signal_payload=memory_guard.exit_signal_payload,
        stderr=diagnostic,
    )
    assert ("infrastructure_error" in diagnostic.getvalue()) is bool(retention_errors)
    assert ("SIGKILL" in diagnostic.getvalue()) is (child_returncode == 137)


def test_run_guarded_exception_releases_lease_and_updates_marker(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FakeLease:
        target = tmp_path / "guard-scratch"
        release_calls = 0

        def release(self) -> None:
            self.release_calls += 1

    lease = FakeLease()
    lease.target.mkdir()
    finish_calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "acquire_guard_scratch",
        lambda _root, _env: lease,
    )

    def finish_scratch(
        _lease: object,
        *,
        closed: bool,
        success: bool,
        evidence: Mapping[str, object],
    ) -> Mapping[str, object]:
        finish_calls.append(
            {
                "closed": closed,
                "success": success,
                "evidence": dict(evidence),
            }
        )
        return {"state": "retained", "receipt": str(tmp_path / "owner.json")}

    monkeypatch.setattr(
        memory_guard._temporary_artifacts,
        "finish_guard_scratch",
        finish_scratch,
    )
    monkeypatch.setattr(
        memory_guard,
        "_guarded_launch",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(RuntimeError("launch failed")),
    )
    env = dict(os.environ)
    state_root = tmp_path / "memory_guard"
    env["MOLT_MEMORY_GUARD_STATE_ROOT"] = str(state_root)

    with pytest.raises(RuntimeError, match="launch failed"):
        memory_guard.run_guarded(
            [sys.executable, "-c", "pass"],
            max_rss_kb=512 * 1024,
            max_total_rss_kb=1024 * 1024,
            poll_interval=0.01,
            child_rlimit_kb=None,
            env=env,
        )

    assert finish_calls == [
        {
            "closed": True,
            "success": False,
            "evidence": {
                "schema": "molt.guard-scratch-closure.v1",
                "authority": "no-child-launched",
                "closed": True,
                "direct_child_reaped": True,
                "exception_type": "RuntimeError",
            },
        }
    ]
    assert lease.release_calls == 1
    markers = list((state_root / "active").glob("*.json"))
    assert len(markers) == 1
    marker = json.loads(markers[0].read_text(encoding="utf-8"))
    assert marker["status"] == "guard_exception"
    assert marker["temporary_artifacts"]["state"] == "retained"
    assert marker["temporary_artifacts"]["closure"]["closed"] is True
    assert marker["temporary_artifacts"]["finalize_elapsed_s"] >= 0.0


@pytest.mark.parametrize("raw_available", [None, "7.5"])
def test_run_guarded_exports_backend_memory_contract(
    request, raw_available, monkeypatch
) -> None:
    if run_in_fresh_guard_observer(request):
        return
    if raw_available is None:
        monkeypatch.delenv("MOLT_BACKEND_MEMORY_AVAILABLE_GB", raising=False)
    else:
        monkeypatch.setenv("MOLT_BACKEND_MEMORY_AVAILABLE_GB", raw_available)
    result = memory_guard.run_guarded(
        [
            sys.executable,
            "-c",
            (
                "import os; "
                "print(os.environ.get('MOLT_BACKEND_MEMORY_AVAILABLE_GB')); "
                "print(os.environ.get('MOLT_BACKEND_MAX_PROCESS_RSS_GB'))"
            ),
        ],
        max_rss_kb=512 * 1024,
        max_total_rss_kb=1024 * 1024,
        poll_interval=0.01,
        child_rlimit_kb=768 * 1024,
    )

    assert result.returncode == 0
    assert result.stdout.splitlines() == [str(raw_available), "0.500000"]


def test_main_reexec_preserves_stream_and_sample_rotation_options(
    monkeypatch, tmp_path
) -> None:
    captured: dict[str, object] = {}
    samples_path = tmp_path / "samples.jsonl"
    stdio = {"stdin": "in", "stdout": "out", "stderr": "err"}

    def fake_execve(path, argv, env):
        captured["path"] = path
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        raise SystemExit(74)

    def fake_subprocess_run(argv, *, env, check, **kwargs):
        assert check is False
        captured["argv"] = list(argv)
        captured["env"] = dict(env)
        captured["creationflags"] = kwargs.get("creationflags", 0)
        captured["run_kwargs"] = dict(kwargs)
        return subprocess.CompletedProcess(argv, 74)

    main_argv = [
        "--max-rss-gb",
        "1",
        "--poll-interval",
        "0.01",
        "--samples-jsonl",
        str(samples_path),
        "--samples-max-mb",
        "0.5",
        "--stream",
        "json-stderr",
        "--child-rlimit-gb",
        "0.75",
        "--",
        sys.executable,
        "-c",
        "print('ok')",
    ]
    if os.name == "nt":
        monkeypatch.setattr(memory_guard, "inherit_stdio_kwargs", lambda: stdio)
        _fake_guard(monkeypatch, "subprocess", subprocess, run=fake_subprocess_run)
        assert (
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
            == 74
        )
    else:
        with pytest.raises(SystemExit) as exc:
            memory_guard.main(
                main_argv,
                hide_command_argv=True,
                execve=fake_execve,
            )
        assert exc.value.code == 74
    worker_argv = captured["argv"]
    assert isinstance(worker_argv, list)
    assert worker_argv[:4] == [
        sys.executable,
        "-I",
        "-S",
        str(Path(memory_guard.__file__).resolve()),
    ]
    assert "--samples-jsonl" in worker_argv
    assert str(samples_path) in worker_argv
    assert "--samples-max-mb" in worker_argv
    assert "0.5" in worker_argv
    assert "--stream" in worker_argv
    assert "json-stderr" in worker_argv
    assert "--child-rlimit-gb" in worker_argv
    assert "0.75" in worker_argv
    if os.name == "nt":
        assert captured["creationflags"] == (
            getattr(memory_guard.subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
            | getattr(memory_guard.subprocess, "CREATE_NO_WINDOW", 0)
        )
        run_kwargs = captured["run_kwargs"]
        assert isinstance(run_kwargs, dict)
        assert run_kwargs["stdin"] == "in"
        assert run_kwargs["stdout"] == "out"
        assert run_kwargs["stderr"] == "err"


def test_internal_worker_loads_command_and_strips_internal_env(monkeypatch) -> None:
    command = [sys.executable, "-c", "print('worker')"]
    observed: dict[str, object] = {}

    def fake_run_guarded(seen_command, **kwargs):
        observed["command"] = list(seen_command)
        observed["env"] = dict(kwargs["env"])
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            child_stderr="",
        )

    monkeypatch.setenv(memory_guard.INTERNAL_WORKER_ENV, "1")
    monkeypatch.setenv(memory_guard.INTERNAL_COMMAND_ENV, json.dumps(command))
    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
        ],
        hide_command_argv=True,
    )

    assert rc == 0
    assert observed["command"] == command
    child_env = observed["env"]
    assert isinstance(child_env, dict)
    assert memory_guard.INTERNAL_COMMAND_ENV not in child_env
    assert memory_guard.INTERNAL_WORKER_ENV not in child_env


def test_resolve_relative_executable_leaves_absolute_and_bare_names() -> None:
    # Absolute paths and bare program names (no separator) are untouched so
    # PATH lookup still works and an explicit absolute command is preserved.
    absolute = [sys.executable, "-c", "print('x')"]
    assert memory_guard._resolve_relative_executable(absolute) == absolute
    bare = ["python3", "-c", "print('x')"]
    assert memory_guard._resolve_relative_executable(bare) == bare
    assert memory_guard._resolve_relative_executable([]) == []


def test_resolve_relative_executable_resolves_against_parent_cwd(
    monkeypatch, tmp_path
) -> None:
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp_name = "python.exe" if os.name == "nt" else "python3"
    rel_interp = rel_dir / rel_interp_name
    if os.name == "nt":
        shutil.copy2(Path(sys.executable).resolve(), rel_interp)
    else:
        rel_interp.symlink_to(Path(sys.executable).resolve())
    monkeypatch.chdir(tmp_path)

    resolved = memory_guard._resolve_relative_executable(
        [f"relbin/{rel_interp_name}", "-c", "print('x')"]
    )

    assert resolved[0] == str(rel_interp.absolute())
    assert resolved[1:] == ["-c", "print('x')"]


def test_resolve_relative_executable_skips_nonexistent_relative_path(
    monkeypatch, tmp_path
) -> None:
    # A relative path that does not exist under the parent cwd is left as-is so
    # an intentionally child-relative command is never clobbered.
    monkeypatch.chdir(tmp_path)
    command = ["does/not/exist", "arg"]
    assert memory_guard._resolve_relative_executable(command) == command


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="relative venv interpreter symlink chain is a POSIX concern",
)
def test_run_guarded_execs_relative_interpreter_with_other_cwd(
    monkeypatch, tmp_path
) -> None:
    rel_dir = tmp_path / "relbin"
    rel_dir.mkdir()
    rel_interp = rel_dir / "python3"
    rel_interp.symlink_to(Path(sys.executable).resolve())
    other_cwd = tmp_path / "elsewhere"
    other_cwd.mkdir()
    monkeypatch.chdir(tmp_path)

    result = memory_guard.run_guarded(
        ["relbin/python3", "-c", "print('relrun')"],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=str(other_cwd),
        child_rlimit_kb=1_000_000,
    )

    assert result.returncode == 0
    assert result.stdout == "relrun\n"


@pytest.mark.parametrize(
    "selection",
    ["relative", "canonical-relative", "canonical-absolute", "canonical-path"],
)
def test_guarded_interpreter_preserves_virtual_environment(
    selection, monkeypatch, tmp_path
):
    import venv
    from tools import harness_memory_guard

    environment = tmp_path / "selected-environment"
    venv.EnvBuilder(with_pip=False, symlinks=os.name != "nt").create(environment)
    if os.name == "nt":
        executable = environment / "Scripts" / "python.exe"
        packages = environment / "Lib" / "site-packages"
    else:
        executable = environment / "bin" / "python3"
        packages = (
            environment
            / "lib"
            / f"python{sys.version_info.major}.{sys.version_info.minor}"
            / "site-packages"
        )
    packages.mkdir(parents=True, exist_ok=True)
    (packages / "selected_environment_witness.py").write_text(
        "VALUE = 'selected environment'\n", encoding="utf-8"
    )
    child_cwd = tmp_path / "child"
    child_cwd.mkdir()
    monkeypatch.chdir(tmp_path)
    command = str(executable.relative_to(tmp_path))
    if selection == "canonical-absolute":
        command = str(executable)
    elif selection == "canonical-path":
        monkeypatch.setenv(
            "PATH", str(executable.parent) + os.pathsep + os.environ.get("PATH", "")
        )
        command = executable.name
    if selection.startswith("canonical-"):
        command = harness_memory_guard.canonical_interpreter(command)
    result = memory_guard.run_guarded(
        [
            command,
            "-c",
            "import json,sys,selected_environment_witness as witness;print(json.dumps([sys.prefix,witness.VALUE]))",
        ],
        max_rss_kb=1_000_000,
        poll_interval=0.01,
        cwd=str(child_cwd),
        child_rlimit_kb=1_000_000,
    )
    assert result.returncode == 0, result.stderr
    prefix, witness = json.loads(result.stdout)
    assert Path(prefix).resolve() == environment.resolve()
    assert witness == "selected environment"


def test_guarded_launch_applies_resource_limit_before_exec_on_posix() -> None:
    command = [sys.executable, "-c", "print('child')"]
    launch = memory_guard._guarded_launch(
        command,
        {"KEEP": "1"},
        child_rlimit_kb=12345,
    )

    if memory_guard.os.name == "posix":
        assert launch.command == command
        assert launch.env == {"KEEP": "1"}
        assert launch.preexec_fn is not None
        assert launch.started_read_fd is not None
        assert launch.pass_fds == launch.close_fds
    else:
        assert launch.command == command
        assert launch.env == {"KEEP": "1"}
        assert launch.pass_fds == ()
        assert launch.close_fds == ()
        assert launch.started_read_fd is None
    memory_guard._close_fds((*launch.close_fds, launch.started_read_fd))


def test_child_started_timestamp_read_preserves_single_close_authority() -> None:
    read_fd, write_fd = os.pipe()
    try:
        os.write(write_fd, b"1234567890\n")
    finally:
        os.close(write_fd)

    try:
        assert memory_guard._read_child_started_at(read_fd) == 1.23456789
        # The GuardedLaunch finalizer, not the read helper, owns the descriptor.
        # Keeping it live also prevents its number from being reused and then
        # spuriously closed by the finalizer (the Linux ABA failure seen in CI).
        assert os.fstat(read_fd).st_ino >= 0
    finally:
        os.close(read_fd)


def test_spawn_failure_preserves_single_started_fd_close_authority(
    monkeypatch,
) -> None:
    started_fd = 123_456
    close_calls: list[tuple[int | None, ...]] = []

    def fail_spawn(*_args, **_kwargs):  # type: ignore[no-untyped-def]
        raise OSError("spawn failed")

    monkeypatch.setattr(
        memory_guard,
        "_guarded_launch",
        lambda *_args, **_kwargs: memory_guard.GuardedLaunch(
            command=["missing"],
            env={},
            started_read_fd=started_fd,
        ),
    )
    _fake_guard(
        monkeypatch,
        "subprocess",
        subprocess,
        Popen=fail_spawn,
    )
    monkeypatch.setattr(
        memory_guard,
        "_close_fds",
        lambda fds: close_calls.append(tuple(fds)),
    )

    with pytest.raises(OSError, match="spawn failed"):
        memory_guard.run_guarded(
            ["missing"],
            max_rss_kb=1_000_000,
            poll_interval=0.01,
        )

    assert sum(call.count(started_fd) for call in close_calls) == 1


def test_main_writes_summary_json(tmp_path) -> None:
    summary_path = tmp_path / "summary.json"
    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["returncode"] == 0
    assert payload["violation"] is None
    assert payload["peak"]["rss_kb"] > 0
    expected_peak_scopes = {"process", "process_rusage"}
    expected_total_scopes = {"process_tree", "process_tree_rusage"}
    if os.name == "nt":
        expected_peak_scopes.add("process_handle")
        expected_total_scopes.add("process_tree_handle")
    assert payload["peak"]["scope"] in expected_peak_scopes
    assert payload["peak_total"]["rss_kb"] >= payload["peak"]["rss_kb"]
    assert payload["peak_total"]["scope"] in expected_total_scopes
    assert payload["max_total_rss_gb"] == pytest.approx(18.0)
    assert payload["child_rlimit_gb"] is None
    assert payload["orphaned_process_groups"] == []
    assert payload["incident"] is None


def test_run_guarded_keeps_windows_handle_peak_when_sampler_misses_child(
    monkeypatch,
) -> None:
    if memory_guard.os.name != "nt":
        pytest.skip("requires Windows process-handle accounting")
    monkeypatch.setattr(
        memory_guard,
        "windows_process_handle_rss_kb",
        lambda _handle: 12_345,
    )
    monkeypatch.setattr(memory_guard._win_job, "create_kill_on_close_job", lambda: None)
    _patch_temporary_artifact_closure_closed(monkeypatch)

    result = memory_guard.run_guarded(
        [sys.executable, "-c", "pass"],
        max_rss_kb=1_000_000,
        max_total_rss_kb=18 * 1024 * 1024,
        poll_interval=0.01,
        sampler=lambda: {},
        timeout=5.0,
    )

    assert result.returncode == 0
    assert result.peak is not None
    assert result.peak.rss_kb == 12_345
    assert result.peak.scope == "process_handle"
    assert result.peak_total is not None
    assert result.peak_total.rss_kb == 12_345
    assert result.peak_total.scope == "process_tree_handle"


def test_main_reports_orphan_cleanup_with_operator_signal(
    tmp_path,
    capsys: pytest.CaptureFixture[str],
    monkeypatch,
) -> None:
    summary_path = tmp_path / "orphan-summary.json"
    report = _guard_termination_report(
        reason="repo_scoped_orphan_cleanup",
        root_pid=44,
        root_pgid=44,
    )

    def fake_run_guarded(_command, **_kwargs):
        return memory_guard.GuardResult(
            returncode=0,
            violation=None,
            peak=None,
            peak_total=None,
            stdout="",
            stderr="",
            elapsed_s=0.4,
            orphaned_process_groups=(44,),
            termination_reports=(report,),
            child_stderr="",
        )

    monkeypatch.setattr(memory_guard, "run_guarded", fake_run_guarded)

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--max-total-rss-gb",
            "18",
            "--poll-interval",
            "0.01",
            "--summary-json",
            str(summary_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    stderr = capsys.readouterr().err
    assert "orphaned child processes detected after command exit" in stderr
    assert "elapsed=0.40s" in stderr
    assert "pgids=44" in stderr
    assert "next action: inspect child process lifecycle and logs" in stderr
    payload = json.loads(summary_path.read_text(encoding="utf-8"))
    assert payload["orphaned_process_groups"] == [44]
    assert payload["incident"]["reason"] == "orphaned_processes_cleaned"
    assert payload["incident"]["elapsed_s"] == pytest.approx(0.4)
    assert payload["incident"]["process_groups"] == [44]
    assert payload["termination_reports"][0]["reason"] == "repo_scoped_orphan_cleanup"
    assert payload["incident"]["termination_reports"][0]["root_pgid"] == 44


def test_incident_reports_incomplete_orphan_cleanup_without_false_success() -> None:
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=100,
                signal=None,
                signal_name=None,
                result="skipped_missing_identity",
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=1,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        elapsed_s=1.0,
        # Even if another group was fully cleaned, this incomplete group must
        # dominate the incident classification and quarantine authority.
        orphaned_process_groups=(777,),
        termination_reports=(report,),
        child_stderr="",
    )

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["reason"] == "orphan_cleanup_incomplete"
    assert incident["candidate_pids"] == [100, 200]
    assert "reported as cleaned" in str(incident["cleanup"])
    assert incident["termination_reports"][0]["actions"][1]["result"] == "failed"


@pytest.mark.parametrize(
    ("result_overrides", "expected_reason", "report_reason"),
    [
        (
            {"guard_signal": int(signal.SIGTERM)},
            "guard_interrupted",
            "guard_signal",
        ),
        ({"timed_out": True}, "timeout", "timeout"),
        (
            {
                "violation": memory_guard.RssViolation(
                    pid=200,
                    rss_kb=10,
                    command="worker",
                )
            },
            "rss_limit_exceeded",
            "rss_limit",
        ),
    ],
)
@pytest.mark.parametrize(
    "history",
    ["failed", "failed_then_completed", "skipped_then_exited", "remaining_only"],
)
def test_primary_incidents_preserve_incomplete_cleanup_truth(
    result_overrides: dict[str, object],
    expected_reason: str,
    report_reason: str,
    history: str,
) -> None:
    report = _guard_termination_report(
        reason=report_reason,
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    if history == "failed_then_completed":
        report = dataclasses.replace(
            report,
            actions=report.actions
            + (
                dataclasses.replace(
                    report.actions[0], result="completed_or_missing", error=None
                ),
            ),
        )
    elif history == "skipped_then_exited":
        report = dataclasses.replace(
            report,
            actions=(
                dataclasses.replace(
                    report.actions[0], result="skipped_identity_mismatch", error=None
                ),
                dataclasses.replace(
                    report.actions[0],
                    result="exited",
                    signal=None,
                    signal_name=None,
                    error=None,
                ),
            ),
        )
    elif history == "remaining_only":
        report = dataclasses.replace(report, actions=(), remaining_pids=(200,))
    kwargs: dict[str, object] = {
        "returncode": 1,
        "violation": None,
        "peak": None,
        "peak_total": None,
        "stdout": "",
        "stderr": "",
        "elapsed_s": 1.0,
        "termination_reports": (report,),
    }
    kwargs.update(result_overrides)
    result = memory_guard.GuardResult(
        **kwargs,
        child_stderr=kwargs["stderr"],
    )  # type: ignore[arg-type]

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["reason"] == expected_reason
    assert "cleanup incomplete" in str(incident["cleanup"])
    assert incident["process_tree_cleanup_status"] == "incomplete"
    assert incident["process_tree_cleanup_candidate_pids"] == [200]


@pytest.mark.parametrize(
    "root_result", ["still_live", "failed", "skipped_identity_mismatch"]
)
def test_owned_child_handle_success_reconciles_only_direct_child_liveness(
    root_result: str,
) -> None:
    primary = _guard_termination_report(
        reason="timeout",
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=100,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result=root_result,
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="failed",
                error="access denied",
            ),
        ),
    )
    handle = _guard_termination_report(
        reason="post_loop_unreaped_child_direct_child_handle",
        root_pid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="owned_child_handle",
                target_id=100,
                signal=memory_guard.fallback_kill_signal(),
                signal_name=memory_guard._signal_name(
                    memory_guard.fallback_kill_signal()
                ),
                result="completed_or_missing",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=memory_guard.TIMEOUT_RETURN_CODE,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        timed_out=True,
        termination_reports=(primary, handle),
        child_stderr="",
    )

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["process_tree_cleanup_status"] == "incomplete"
    assert incident["process_tree_cleanup_candidate_pids"] == (
        [200] if root_result == "still_live" else [100, 200]
    )

    fully_reaped = dataclasses.replace(
        result,
        termination_reports=(
            dataclasses.replace(primary, actions=(primary.actions[0],)),
            handle,
        ),
    )
    fully_reaped_incident = memory_guard._incident_payload(fully_reaped)
    assert fully_reaped_incident is not None
    if root_result == "still_live":
        assert "process_tree_cleanup_status" not in fully_reaped_incident
        assert fully_reaped_incident["cleanup"] == "terminated tracked process tree"
    else:
        assert fully_reaped_incident["process_tree_cleanup_status"] == "incomplete"
        assert fully_reaped_incident["process_tree_cleanup_candidate_pids"] == [100]
    corroborating_reap = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        root_pid=100,
        actions=(
            dataclasses.replace(
                primary.actions[0], result="reaped", signal=None, signal_name=None
            ),
        ),
    )
    corroborated = memory_guard._incident_payload(
        dataclasses.replace(
            fully_reaped,
            termination_reports=fully_reaped.termination_reports
            + (corroborating_reap,),
        )
    )
    assert corroborated is not None
    assert (corroborated.get("process_tree_cleanup_status") == "incomplete") is (
        root_result != "still_live"
    )


def test_completed_group_outcome_preserves_protected_root_group_skip() -> None:
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        root_pid=100,
        root_pgid=100,
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process_group",
                target_id=100,
                signal=None,
                signal_name=None,
                result="skipped_protected_root_group",
            ),
            memory_guard.GuardTerminationAction(
                target_kind="process_group",
                target_id=100,
                signal=memory_guard.signal.SIGTERM,
                signal_name="SIGTERM",
                result="completed_or_missing",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=0,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        orphaned_process_groups=(100,),
        termination_reports=(report,),
        child_stderr="",
    )

    incident = memory_guard._incident_payload(result)

    assert incident is not None
    assert incident["reason"] == "orphan_cleanup_incomplete"
    assert incident["orphan_cleanup_status"] == "incomplete"


@pytest.mark.parametrize("job_completed", [False, True])
@pytest.mark.parametrize("timed_out", [False, True])
def test_incident_cleanup_preserves_exact_windows_job_authority(
    job_completed: bool, timed_out: bool
) -> None:
    report = _guard_termination_report(
        reason="tracked_orphan_cleanup",
        actions=(
            memory_guard.GuardTerminationAction(
                target_kind="process",
                target_id=200,
                signal=None,
                signal_name=None,
                result="skipped_identity_mismatch",
            ),
        ),
    )
    result = memory_guard.GuardResult(
        returncode=memory_guard.TIMEOUT_RETURN_CODE if timed_out else 0,
        violation=None,
        peak=None,
        peak_total=None,
        stdout="",
        stderr="",
        timed_out=timed_out,
        orphaned_process_groups=(200,),
        termination_reports=(report,),
        windows_job_cleanup=_windows_job_cleanup(
            active_processes=0 if job_completed else 1
        ),
        child_stderr="",
    )
    incident = memory_guard._incident_payload(result)
    assert incident is not None
    assert incident["reason"] == (
        "timeout"
        if timed_out
        else "orphaned_processes_cleaned"
        if job_completed
        else "orphan_cleanup_incomplete"
    )
    assert (incident.get("orphan_cleanup_status") == "incomplete") is (
        not job_completed
    )
    if timed_out:
        assert ("cleanup incomplete" in str(incident["cleanup"])) is (not job_completed)


def test_main_writes_samples_jsonl(tmp_path) -> None:
    samples_path = tmp_path / "samples.jsonl"
    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--samples-jsonl",
            str(samples_path),
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    )

    assert rc == 0
    lines = samples_path.read_text(encoding="utf-8").splitlines()
    assert lines
    payload = json.loads(lines[-1])
    assert payload["root_pid"] > 0
    assert "peak" in payload
    assert "total" in payload


def test_sample_jsonl_rotation_bounds_artifacts(tmp_path) -> None:
    samples_path = tmp_path / "samples.jsonl"
    peak = memory_guard.RssViolation(pid=100, rss_kb=10, command="root")

    for _ in range(8):
        memory_guard._record_sample(
            root_pid=100,
            peak=peak,
            total=peak,
            violation=None,
            samples_jsonl=str(samples_path),
            samples_jsonl_max_bytes=1024,
            stream="",
        )

    assert samples_path.exists()
    assert samples_path.with_name("samples.jsonl.1").exists()
    assert samples_path.stat().st_size <= 1024
    assert samples_path.with_name("samples.jsonl.1").stat().st_size <= 1024


def test_main_streams_samples_without_sample_artifact(
    tmp_path, capsys: pytest.CaptureFixture[str]
) -> None:
    samples_path = tmp_path / "samples.jsonl"

    rc = memory_guard.main(
        [
            "--max-rss-gb",
            "1",
            "--poll-interval",
            "0.01",
            "--child-rlimit-gb",
            "0",
            "--stream",
            "stderr",
            "--",
            sys.executable,
            "-c",
            "pass",
        ]
    )

    captured = capsys.readouterr()
    assert rc == 0
    assert "memory_guard sample:" in captured.err
    assert not samples_path.exists()


@pytest.mark.skipif(os.name != "posix", reason="POSIX launch group identity")
@pytest.mark.parametrize("child_rlimit_kb", [None, 2 * 1024 * 1024])
def test_posix_launch_identity_survives_reap_before_clock_returns(
    monkeypatch, child_rlimit_kb
):
    real_clock = process_custody.ChildExecutionClock

    def reap_before_return(proc, started, **kwargs):
        clock = real_clock(proc, started, **kwargs)
        # Force the adversarial schedule through the actual sole reaper.
        # No timing race or repeated fast-command loop is needed.
        assert proc.wait(timeout=5) == 0
        assert clock.done.is_set()
        return clock

    monkeypatch.setattr(process_custody, "ChildExecutionClock", reap_before_return)
    result = memory_guard.run_guarded(
        [getattr(sys, "_base_executable", sys.executable), "-I", "-S", "-c", "pass"],
        max_rss_kb=1024 * 1024,
        max_total_rss_kb=2 * 1024 * 1024,
        capture_output=True,
        text=False,
        poll_interval=0.01,
        timeout=10,
        child_rlimit_kb=child_rlimit_kb,
    )
    assert result.child_returncode == 0
    assert result.returncode == 0, result.stderr
    assert result.infrastructure_failure is None
    assert result.child_process is not None
    assert result.child_process.pgid == result.child_process.pid
    assert result.child_process.sid == result.child_process.pid
    assert result.descendants_closed
    assert result.temporary_artifacts is not None
    closure = result.temporary_artifacts["closure"]
    assert closure["authority"] == "posix-sampled-process-group"
    assert closure["root_process_group_closed"] is True
    assert closure["remaining_tracked_pids"] == []
    assert closure["root_process_group_members"] == []


@pytest.mark.skipif(os.name != "posix", reason="POSIX session-leader launch identity")
def test_launch_identity_survives_a_child_the_kernel_no_longer_reports(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Under load a fast child can exit before the guard records its launch
    # identity, and macOS answers getpgid/getsid for an unreaped zombie with
    # ESRCH. Model that kernel answer for every queried PID: the identity must
    # come from the session-leader launch, and closure must still complete.
    def esrch(pid: int) -> int:
        raise ProcessLookupError(errno.ESRCH, "No such process")

    _fake_guard(
        monkeypatch,
        "os",
        os,
        getpgid=esrch,
        getsid=esrch,
    )

    result = memory_guard.run_guarded(
        [getattr(sys, "_base_executable", sys.executable), "-I", "-S", "-c", "pass"],
        max_rss_kb=1024 * 1024,
        max_total_rss_kb=2 * 1024 * 1024,
        capture_output=True,
        poll_interval=0.01,
        child_rlimit_kb=None,
    )

    assert result.returncode == 0, result.stderr
    assert result.infrastructure_failure is None
    assert result.child_process is not None
    assert result.child_process.pgid == result.child_process.pid
    assert result.child_process.sid == result.child_process.pid
    assert result.descendants_closed


@pytest.mark.parametrize("delayed_boundary", ["sampler", "scratch"])
def test_child_clock_is_independent_of_guard_setup_and_sampler(
    monkeypatch, delayed_boundary
):
    def run(sampler):
        return memory_guard.run_guarded(
            [
                getattr(sys, "_base_executable", sys.executable),
                "-I",
                "-S",
                "-c",
                "pass",
            ],
            max_rss_kb=1024 * 1024,
            max_total_rss_kb=2 * 1024 * 1024,
            capture_output=True,
            text=True,
            poll_interval=0.01,
            sampler=sampler,
            child_rlimit_kb=None,
        )

    # A sampler delay hides inside a child that outlives it, so no clock could
    # tell them apart. Size the delay from this child's own lifetime under the
    # current load (a loaded Windows runner takes 0.48 s to run `pass`).
    baseline = run(memory_guard.sample_processes)
    assert baseline.returncode == 0
    assert baseline.child_elapsed_s is not None
    delay_s = max(0.3, 5 * baseline.child_elapsed_s)
    if delayed_boundary == "scratch":
        original = memory_guard._temporary_artifacts.acquire_guard_scratch

        def delayed_scratch(*args, **kwargs):
            time.sleep(delay_s)
            return original(*args, **kwargs)

        monkeypatch.setattr(
            memory_guard._temporary_artifacts, "acquire_guard_scratch", delayed_scratch
        )
        sampler = memory_guard.sample_processes
    else:
        # One slow observation suffices: the child exits while the guard's
        # first sample is still running, so any clock that included sampler
        # latency would report at least the delay. Later samples are fast.
        original = memory_guard.sample_processes
        sampler_delays = [delay_s]

        def sampler():
            if sampler_delays:
                time.sleep(sampler_delays.pop())
            return original()

        # Windows uses its job-owned sampler; delaying that kernel-owned query
        # must not influence the process-handle clock either.
        if os.name == "nt":
            original_memory = memory_guard._win_job.process_memory
            memory_delays = [delay_s]

            def slow_memory(*args, **kwargs):
                if memory_delays:
                    time.sleep(memory_delays.pop())
                return original_memory(*args, **kwargs)

            monkeypatch.setattr(memory_guard._win_job, "process_memory", slow_memory)
    result = run(sampler)
    assert result.returncode == 0
    assert result.child_elapsed_s is not None
    # The child clock reports the child's own lifetime, well under the delay;
    # the guard clock includes the delay.
    assert result.child_elapsed_s < 0.6 * delay_s
    assert result.elapsed_s >= delay_s
    assert result.elapsed_s == result.child_elapsed_s + result.cleanup_elapsed_s


def test_posix_child_clock_has_one_reaper_independent_of_sampling(monkeypatch):
    import threading

    calls = []

    def wait4(pid, flags):
        calls.append((pid, flags))
        return pid, 0, types.SimpleNamespace(ru_maxrss=64)

    # The clock's own reaper thread calls wait4 and waitid, so these fakes
    # answer every thread that reads process_custody's os.
    install_module_view(
        monkeypatch,
        "os",
        os,
        process_custody,
        wait4=wait4,
        waitid=lambda *args: None,
        name="posix",
        WNOWAIT=1,
        WEXITED=2,
        P_PID=3,
        WNOHANG=4,
    )
    proc = types.SimpleNamespace(
        pid=123456,
        args=["owned-mock"],
        returncode=None,
        wait=lambda: None,
        _waitpid_lock=threading.Lock(),
    )
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    assert proc.wait(timeout=1) == 0
    exited = clock.finished
    assert proc.poll() == 0
    assert process_custody._take_child_exit_usage(proc).max_rss_kb > 0
    assert process_custody._take_child_exit_usage(proc) is None
    assert clock.finished == exited
    assert calls == [(123456, process_custody.os.WNOHANG)]


@pytest.mark.skipif(os.name != "nt", reason="Windows suspended job custody")
def test_child_clock_excludes_suspended_job_assignment_delay(monkeypatch):
    original_resume = memory_guard._win_job._resume_process
    delay_s = 0.3

    def delayed_resume(*args, **kwargs):
        time.sleep(delay_s)
        return original_resume(*args, **kwargs)

    monkeypatch.setattr(memory_guard._win_job, "_resume_process", delayed_resume)
    result = memory_guard.run_guarded(
        [getattr(sys, "_base_executable", sys.executable), "-I", "-S", "-c", "pass"],
        max_rss_kb=1024 * 1024,
        max_total_rss_kb=2 * 1024 * 1024,
        capture_output=True,
        text=True,
        poll_interval=0.01,
    )
    assert result.returncode == 0
    assert result.child_elapsed_s is not None
    # The guard clock covers the suspended assignment delay and the child clock
    # excludes it, however long the child itself takes on a loaded host (a
    # hosted Windows run took 0.41 s for `pass`).
    assert result.elapsed_s - result.child_elapsed_s >= delay_s * 0.95


@pytest.mark.skipif(
    os.name != "nt", reason="Windows Popen publishes status before waiter returns"
)
def test_windows_child_clock_publishes_exit_only_after_timestamp():
    import threading

    status_published = threading.Event()
    release_wait = threading.Event()
    proc = types.SimpleNamespace(
        pid=123456, args=["mock-owned-handle"], returncode=None
    )

    def wait():
        proc.returncode = 0
        status_published.set()
        assert release_wait.wait(1.0)
        return 0

    proc.wait = wait
    clock = process_custody.ChildExecutionClock(proc, time.perf_counter())
    try:
        assert status_published.wait(1.0)
        assert proc.returncode == 0
        assert proc.poll() is None
        assert clock.finished is None
    finally:
        release_wait.set()
    assert proc.wait(timeout=1.0) == 0
    assert clock.finished is not None
    assert proc.poll() == 0


@pytest.mark.parametrize(
    "parent_birth,child_birth,admitted",
    [
        (100, 100, True),
        (100, 200, True),
        (200, 100, False),
        (None, 200, False),
        (100, None, False),
        (0, 200, False),
        (100, 0, False),
        (True, 200, False),
        (100, True, False),
    ],
)
def test_host_protection_exemptions_require_birth_fenced_current_ancestry(
    parent_birth,
    child_birth,
    admitted,
):
    sample = memory_guard.ProcessSample
    samples = {
        50: sample(50, 1, 1, "codex", pgid=50),
        100: sample(
            100,
            50,
            1,
            "python tools/memory_guard.py",
            pgid=100,
            started_at_ns=parent_birth,
        ),
        200: sample(200, 100, 1, "molt-backend", pgid=200, started_at_ns=child_birth),
        201: sample(201, 200, 1, "worker", pgid=201, started_at_ns=300),
        300: sample(300, 100, 1, "node node_repl.js", pgid=300, started_at_ns=300),
    }
    protected = process_model.protected_process_group_ids(
        samples,
        self_pid=100,
        owned_pids={200, 201, 300},
    )
    assert {50, 100, 300} <= protected
    assert (200 not in protected) is admitted
    assert (201 not in protected) is admitted
    assert process_model.has_external_host_control_plane_lineage(
        samples,
        200,
        current_pid=100,
        owned_pids={200},
    ) is (not admitted)
    assert process_model.has_external_host_control_plane_lineage(
        samples,
        300,
        current_pid=100,
        owned_pids={300},
    )
    # Protection must keep possible host lineage even when its birth is unknown.
    assert process_model.host_control_plane_ancestor_pids(samples, 201) == {50}


def test_untracked_watch_cannot_admit_children_of_an_absent_root():
    sample = memory_guard.ProcessSample
    samples = {200: sample(200, 100, 1, "stale parent pid", started_at_ns=200)}
    assert memory_guard.watched_pids(samples, 100) == set()


def test_sigkilled_target_gets_time_to_leave_under_zero_grace(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Zero grace means "kill now", not "vanish within 20 ms" (HF-F141).

    The escaped descendant leaves the process table 0.2 s after SIGKILL, which
    a loaded host can take. A probe window shorter than that sees it live and
    the guard reports incomplete cleanup.
    """
    monkeypatch.setattr(
        process_custody,
        "os",
        types.SimpleNamespace(name="posix", getpid=lambda: 999, getpgrp=lambda: 999),
    )
    custody_signal = _posix_custody_boundary(monkeypatch)
    root = process_custody.ProcessSample(
        101, 999, 10, "python root.py", pgid=101, started_at_ns=11
    )
    child = process_custody.ProcessSample(
        202, 101, 20, "python child.py", pgid=101, started_at_ns=22
    )
    tracker = process_custody.ProcessTreeTracker(101)
    tracker.update({101: root, 202: child})
    live = {202: dataclasses.replace(child, ppid=1, pgid=202)}
    killed: list[int] = []
    teardown_s = 0.2

    def send(pid: int, signum: int):
        if signum == custody_signal.SIGKILL:
            killed.append(pid)
        return process_custody._termination_action(
            target_kind="process", target_id=pid, signum=signum, result="sent"
        )

    def probe(pid: int, *, grace: float) -> bool:
        if not killed:
            return False  # The TERM grace expires with the target alive.
        if grace < teardown_s:
            return False  # The window closed before the kernel finished.
        live.clear()
        return True

    monkeypatch.setattr(process_custody, "_send_pid_signal_action", send)
    monkeypatch.setattr(process_custody, "_pid_exited_or_unobservable", probe)
    monkeypatch.setattr(process_custody, "_process_group_exited_or_unobservable", probe)

    cleanup = process_custody.cleanup_tracked_orphans(
        101, tracker=tracker, sampler=lambda: dict(live), grace=0.0, root_reaped=True
    )

    (report,) = cleanup.termination_reports
    assert killed == [202]
    assert report.actions[-1].result == "exited"
    assert report.remaining_pids == ()


@pytest.mark.parametrize(
    "data,error",
    [
        (
            b'{"schema_version":1,"nodeid":"old","phase":"call"}' + b" " * (16 * 1024),
            "current_test_record_exceeds_byte_limit",
        ),
        (
            b'{"schema_version":1,"nodeid":"partial',
            "current_test_incomplete_or_invalid_json",
        ),
        (
            b'\0{"nodeid":"uncommitted","phase":"call"}',
            "current_test_incomplete_or_invalid_json",
        ),
        (b"\xff", "current_test_invalid_encoding"),
        (b"[]", "current_test_invalid_record"),
        (
            b'{"schema_version":true,"nodeid":"wrong","phase":"call"}',
            "current_test_invalid_record",
        ),
        (
            b'{"schema_version":1,"nodeid":"wrong","phase":[]}',
            "current_test_invalid_record",
        ),
    ],
)
def test_current_test_reader_refuses_partial_oversized_or_invalid_identity(
    tmp_path, data, error
):
    path = tmp_path / "current-test.json"
    path.write_bytes(data)
    record = memory_guard._read_pytest_current_test_json(path)
    assert record["error"] == error
    assert "payload" not in record


def test_current_test_diagnostic_reader_never_creates_missing_storage(tmp_path):
    path = tmp_path / "missing" / "current-test.json"
    assert memory_guard._read_pytest_current_test_json(path)["missing"] is True
    assert not path.parent.exists()


@pytest.mark.parametrize("link_kind", ["directory", "record"])
def test_xdist_current_records_reject_symlinks_outside_canonical_custody(
    tmp_path, monkeypatch, link_kind
):
    current_root = tmp_path / "pytest-memory-guard"
    current_root.mkdir()
    aggregate = current_root / "current-test.json"
    worker_dir = aggregate.with_name(aggregate.name + ".d")
    outside = tmp_path / "outside"
    outside.mkdir()
    escaped = outside / "gw0.json"
    escaped.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "nodeid": "must-not-read-outside",
                "phase": "call",
            }
        ),
        encoding="utf-8",
    )
    try:
        if link_kind == "directory":
            worker_dir.symlink_to(outside, target_is_directory=True)
        else:
            worker_dir.mkdir()
            (worker_dir / "gw0.json").symlink_to(escaped)
    except OSError as error:
        pytest.skip(f"host lacks symlink capability: {error}")
    from molt import file_locks

    acquire = file_locks._try_acquire_file_lock

    def admitted_only(candidate, **kwargs):
        assert candidate == aggregate, "unadmitted worker path reached file open"
        return acquire(candidate, **kwargs)

    with monkeypatch.context() as probe:
        probe.setattr(file_locks, "_try_acquire_file_lock", admitted_only)
        record = memory_guard._pytest_current_test_file_payload(
            {
                "MOLT_MEMORY_GUARD_STATE_ROOT": str(tmp_path / "memory_guard"),
                "MOLT_PYTEST_CURRENT_TEST_FILE": str(aggregate),
            },
            samples={},
        )
    assert record["worker_records"][0]["rejected"] == "noncanonical"
    assert "payload" not in record["worker_records"][0]
    assert "must-not-read-outside" not in json.dumps(record)


@pytest.mark.parametrize("unavailable", [False, True])
def test_current_test_raw_short_reads_never_admit_an_oversized_valid_prefix(
    tmp_path, monkeypatch, unavailable
):
    from molt import file_locks

    path = tmp_path / "current-test.json"
    prefix = b'{"schema_version":1,"nodeid":"prefix","phase":"call"}'
    path.write_bytes(prefix + b" " * (16 * 1024))
    acquire = file_locks._try_acquire_file_lock
    bytes_read = []

    class Reads:
        def __init__(self, file):
            self.file = file

        def __getattr__(self, name):
            return getattr(self.file, name)

        def read(self, amount):
            if unavailable:
                return None
            chunk = self.file.read(min(amount, len(prefix)))
            bytes_read.append(len(chunk))
            return chunk

    def partial(candidate, **kwargs):
        handle = acquire(candidate, **kwargs)
        if handle is not None and candidate == path:
            handle.file = Reads(handle.file)
        return handle

    with monkeypatch.context() as fault:
        fault.setattr(file_locks, "_try_acquire_file_lock", partial)
        record = memory_guard._read_pytest_current_test_json(path)
    assert "payload" not in record
    if unavailable:
        assert record["error"] == "current_test_read_incomplete"
    else:
        assert record["truncated"] is True
        assert record["error"] == "current_test_record_exceeds_byte_limit"
        assert bytes_read[0] == len(prefix)
        assert sum(bytes_read) == memory_guard._PYTEST_CURRENT_TEST_FILE_MAX_BYTES + 1


def _managed_guard_use_environment(tmp_path):
    from molt.dx import checkout_component
    from molt.build_state_layout import build_state_root

    checkout = tmp_path / "project"
    checkout.mkdir()
    artifact = tmp_path / "artifacts"
    parent = artifact / "target" / "checkouts" / checkout_component(checkout)
    session = parent / "sessions" / "reader"
    environment = {
        **os.environ,
        "CARGO_TARGET_DIR": str(session),
        "MOLT_EXT_ROOT": str(artifact),
    }
    control = build_state_root(
        project_root=checkout, cargo_target=session, environment=environment
    )
    return environment, (parent, session, control)


def _assert_guard_use_retirement(roots, available):
    from molt.checkout_state import checkout_state_retirement

    for root in roots:
        with checkout_state_retirement(root) as admitted:
            assert admitted is available, root


@pytest.mark.parametrize("group", [False, True])
@pytest.mark.parametrize(
    "observation", ["live", "exited", "unknown", "denied", "lookup", "deadline", "zero"]
)
def test_windows_exit_observation_queries_without_signaling(
    monkeypatch, group, observation
):
    from tests.process_guard_common import install_thread_module_view

    now = [0.0]
    queries = []

    def query(pid):
        assert pid == 300
        queries.append(now[0])
        if observation == "unknown":
            raise OSError("native query unavailable")
        if observation == "denied":
            raise PermissionError("native query denied")
        if observation == "lookup":
            # A native-query exception is unknown even if it has this Python
            # subclass; only the API's actual False result proves exit.
            raise ProcessLookupError("unclassified native query error")
        if observation == "exited":
            return False
        if observation == "deadline":
            return now[0] < 0.04
        return True

    def forbidden_signal(*args, **kwargs):
        pytest.fail("Windows liveness observation must never call os.kill/killpg")

    install_thread_module_view(
        monkeypatch,
        "_process_model",
        process_model,
        process_custody,
        is_windows_process_model=lambda: True,
        pid_alive=query,
    )
    install_thread_module_view(
        monkeypatch,
        "os",
        os,
        process_custody,
        kill=forbidden_signal,
        killpg=forbidden_signal,
    )
    install_thread_module_view(
        monkeypatch,
        "time",
        time,
        process_custody,
        monotonic=lambda: now[0],
        sleep=lambda seconds: now.__setitem__(0, now[0] + seconds),
    )
    probe = (
        process_custody.process_group_exited_or_unobservable
        if group
        else process_custody._pid_exited_or_unobservable
    )
    closed = probe(300, grace=0.0 if observation == "zero" else 0.04)
    assert closed is (observation in {"exited", "deadline"})
    if observation == "zero":
        assert queries == []
    elif observation == "deadline":
        assert queries[-1] == 0.04
    else:
        assert queries


# Enforcement does not wait on observation (HF-417). On 2026-10-21 a guard
# blocked 38 minutes in a host-wide read and signalled its 300 s deadline at
# 2,270 s. These tests stall each observation that stood on the enforcement
# path and require the child's own process group to be signalled anyway.

# A child that ends only when its process group is signalled.
_ENFORCED_CHILD = "import threading\nthreading.Event().wait()\n"


def _wait_until(predicate: Callable[[], bool], bound_s: float) -> bool:
    """Poll ``predicate``; ``bound_s`` is a hang guard, never the oracle."""
    deadline = time.monotonic() + bound_s
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


class _GuardThread:
    """Run ``run_guarded`` on a thread; optionally stall its own samples."""

    def __init__(
        self, command: list[str], *, stall_samples: bool = False, **kwargs: Any
    ) -> None:
        self.release = threading.Event()
        self.guard_samples = 0
        self.spawned: list[int] = []
        self.outcome: dict[str, Any] = {}
        if stall_samples:
            kwargs["sampler"] = self._stalled_sampler
        self.thread = threading.Thread(
            target=self._run, args=(command,), kwargs=kwargs, name="hf417-guard"
        )

    def _stalled_sampler(self) -> Mapping[int, memory_guard.ProcessSample]:
        # The guard's first sample is its launch baseline; each later one on
        # the guard thread stalls until the test releases it. The reaper's
        # exit census runs on its own thread and is not stalled.
        if threading.current_thread() is self.thread:
            self.guard_samples += 1
            if self.guard_samples > 1:
                self.release.wait(600)
        return memory_guard.sample_processes()

    def _run(self, command: list[str], **kwargs: Any) -> None:
        try:
            self.outcome["result"] = memory_guard.run_guarded(
                command, on_spawn=self.spawned.append, **kwargs
            )
        except BaseException as exc:
            self.outcome["error"] = exc

    def start(self) -> int:
        self.thread.start()
        assert _wait_until(lambda: bool(self.spawned), 120)
        return self.spawned[0]

    def finish(self) -> memory_guard.GuardResult:
        self.release.set()
        self.thread.join(600)
        assert not self.thread.is_alive()
        if "error" in self.outcome:
            raise self.outcome["error"]
        return self.outcome["result"]


def _root_signalled(pid: int) -> Callable[[], bool]:
    # The reaper reaps the signalled root; a reaped or zombie child is absent.
    return lambda: pid not in memory_guard.sample_processes()


def _enforcer_reports(
    result: memory_guard.GuardResult, reason: str
) -> list[memory_guard.GuardTerminationReport]:
    # The enforcer observes nothing, so its reports watch no sampled pid.
    return [
        report
        for report in result.termination_reports
        if report.reason == reason and report.watched_pids == ()
    ]


@pytest.mark.skipif(
    os.name != "posix", reason="the POSIX enforcer signals the child's own group"
)
def test_deadline_holds_while_the_guard_thread_is_blocked_sampling() -> None:
    guard = _GuardThread(
        [sys.executable, "-c", _ENFORCED_CHILD],
        max_rss_kb=4_000_000,
        poll_interval=0.02,
        timeout=0.5,
        stall_samples=True,
    )
    try:
        pid = guard.start()
        bound = 0.5 + memory_guard.ENFORCEMENT_SLACK_SEC + 120
        assert _wait_until(_root_signalled(pid), bound)
        # The guard thread was still blocked in its second sample.
        assert guard.guard_samples >= 2 and not guard.release.is_set()
    finally:
        result = guard.finish()
    assert result.timed_out
    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    # The run loop deferred to the enforcer's strike: one report, no stale
    # second signal.
    assert [report.reason for report in result.termination_reports] == ["timeout"]
    (report,) = _enforcer_reports(result, "timeout")
    actions = [
        (action.target_kind, action.target_id, action.signal_name, action.result)
        for action in report.actions
    ]
    # SIGTERM to the root's own group; SIGKILL follows only a root that
    # outlived the grace; the run loop observed the group's exit afterwards.
    assert actions[0] == ("process_group", pid, "SIGTERM", "sent")
    assert actions[-1] == ("process_group", pid, None, "exited")
    assert {action[2] for action in actions[1:-1]} <= {"SIGKILL"}
    assert result.descendants_closed


@pytest.mark.skipif(
    os.name != "posix", reason="the POSIX enforcer signals the child's own group"
)
def test_owner_cancellation_holds_while_the_guard_thread_is_blocked_sampling() -> None:
    cancel = threading.Event()
    guard = _GuardThread(
        [sys.executable, "-c", _ENFORCED_CHILD],
        max_rss_kb=4_000_000,
        poll_interval=0.02,
        cancellation_requested=cancel.is_set,
        stall_samples=True,
    )
    try:
        pid = guard.start()
        assert _wait_until(lambda: guard.guard_samples >= 2, 120)
        cancel.set()
        bound = memory_guard.ENFORCEMENT_SLACK_SEC + 120
        assert _wait_until(_root_signalled(pid), bound)
        assert not guard.release.is_set()
    finally:
        result = guard.finish()
    assert result.cancelled
    assert not result.timed_out
    assert [report.reason for report in result.termination_reports] == [
        "owner_cancellation"
    ]
    assert _enforcer_reports(result, "owner_cancellation")
    assert result.descendants_closed


@pytest.mark.skipif(
    os.name != "posix", reason="the POSIX enforcer signals the child's own group"
)
def test_memory_trip_holds_while_the_kill_waits_on_the_host_table(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    if not process_model.posix_custody_reads_partial_rows():
        pytest.skip("this host's polls read the whole table; no separate kill read")
    holder: dict[str, Any] = {}
    entered = threading.Event()
    real_table = memory_guard.sample_processes

    def stalled_table() -> Mapping[int, memory_guard.ProcessSample]:
        # Polls read custody rows; the kill's protection decision reads the
        # host table. Stall that read on the guard thread.
        guard = holder.get("guard")
        if guard is not None and threading.current_thread() is guard.thread:
            entered.set()
            guard.release.wait(600)
        return real_table()

    # The guard's host sampler is this function, so its polls stay custody
    # reads and only its whole-table reads stall.
    monkeypatch.setattr(memory_guard, "sample_processes", stalled_table)
    guard = _GuardThread(
        [sys.executable, "-c", _ENFORCED_CHILD],
        max_rss_kb=1,
        poll_interval=0.02,
        sampler=stalled_table,
    )
    holder["guard"] = guard
    try:
        pid = guard.start()
        assert entered.wait(120)
        bound = memory_guard.ENFORCEMENT_SLACK_SEC + 120
        assert _wait_until(_root_signalled(pid), bound)
        assert not guard.release.is_set()
    finally:
        result = guard.finish()
    assert result.violation is not None
    assert result.returncode == memory_guard.GUARD_RETURN_CODE
    # The run loop deferred to the enforcer's strike.
    assert [report.reason for report in result.termination_reports] == ["rss_limit"]
    assert _enforcer_reports(result, "rss_limit")
    assert result.descendants_closed


@pytest.mark.skipif(
    os.name != "posix", reason="the POSIX enforcer signals the child's own group"
)
def test_deadline_holds_while_the_running_summary_reads_the_host(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    guard = _GuardThread(
        [sys.executable, "-c", _ENFORCED_CHILD],
        max_rss_kb=4_000_000,
        poll_interval=0.02,
        timeout=0.5,
        running_summary_json=str(tmp_path / "summary.json"),
    )
    entered = threading.Event()
    real_repro = memory_guard.repro_context_payload

    def stalled_repro(**kwargs: Any) -> dict[str, object]:
        # Every process's command line on the host: the read that stalled.
        entered.set()
        guard.release.wait(600)
        return real_repro(**kwargs)

    monkeypatch.setattr(memory_guard, "repro_context_payload", stalled_repro)
    try:
        pid = guard.start()
        assert entered.wait(120)
        assert _wait_until(_root_signalled(pid), 120)
        assert not guard.release.is_set()
    finally:
        result = guard.finish()
    assert result.timed_out
    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE


def test_enforcer_signals_only_what_the_run_loop_has_not_delivered() -> None:
    calls: list[tuple[str, float]] = []
    signalled = threading.Event()

    def signal_root(reason: str, grace: float):
        calls.append((reason, grace))
        signalled.set()
        return True, None

    # A delivered reason is never enforced; the long slack rules out a race.
    idle = process_custody.OwnedTreeEnforcer(
        signal_root, slack=600.0, deadline=time.perf_counter(), deadline_grace=0.25
    )
    idle.request("rss_limit", grace=0.25)
    idle.delivered("rss_limit")
    idle.delivered("timeout")
    assert idle.stop() == ((), ())
    assert calls == []

    # An undelivered request is enforced once, no sooner than the slack.
    busy = process_custody.OwnedTreeEnforcer(signal_root, slack=0.05)
    requested_at = time.perf_counter()
    busy.request("owner_cancellation", grace=0.0)
    busy.request("owner_cancellation", grace=0.0)
    assert signalled.wait(120)
    assert time.perf_counter() - requested_at >= 0.05
    assert busy.stop() == (("owner_cancellation",), ())
    assert calls == [("owner_cancellation", 0.0)]

    # A run loop about to signal defers to a strike already made, and a
    # strike not yet made waits another slack behind the loop's own signal.
    calls.clear()
    signalled.clear()
    struck = process_custody.OwnedTreeEnforcer(signal_root, slack=0.05)
    struck.request("rss_limit", grace=0.25)
    assert _wait_until(lambda: calls == [("rss_limit", 0.25)], 120)
    assert struck.begin("rss_limit") is False
    assert struck.stop() == (("rss_limit",), ())
    calls.clear()
    signalled.clear()
    behind = process_custody.OwnedTreeEnforcer(signal_root, slack=600.0)
    behind.request("timeout", grace=0.25)
    assert behind.begin("timeout") is True
    behind.delivered("timeout")
    assert behind.stop() == ((), ())
    assert calls == []

    # The deadline is due at deadline + slack, with its own grace.
    calls.clear()
    signalled.clear()
    deadline = process_custody.OwnedTreeEnforcer(
        signal_root,
        slack=0.05,
        deadline=time.perf_counter() + 0.05,
        deadline_grace=0.25,
    )
    assert signalled.wait(120)
    assert deadline.stop() == (("timeout",), ())
    assert calls == [("timeout", 0.25)]

    # A root already reaped is not signalled, so nothing counts as enforced.
    attempted = threading.Event()

    def reaped_root(_reason: str, _grace: float):
        attempted.set()
        return False, None

    reaped = process_custody.OwnedTreeEnforcer(reaped_root, slack=0.0)
    reaped.request("rss_limit", grace=0.25)
    assert attempted.wait(120)
    assert reaped.stop() == ((), ())

    # Without an identity-safe signal the enforcer is inert.
    inert = process_custody.OwnedTreeEnforcer(
        None, slack=0.0, deadline=time.perf_counter()
    )
    inert.request("rss_limit", grace=0.25)
    assert inert.stop() == ((), ())


def test_run_guarded_ends_a_command_whose_output_stops() -> None:
    result = memory_guard.run_guarded(
        [sys.executable, "-c", "print('started', flush=True)\n" + HUNG_CHILD_SOURCE],
        max_rss_kb=1_000_000,
        poll_interval=0.02,
        no_output_timeout=0.3,
        timeout=120,  # A hang guard only; its message differs.
    )

    assert result.timed_out is True
    assert result.returncode == memory_guard.TIMEOUT_RETURN_CODE
    assert result.stdout == "started\n"
    assert "memory_guard: no output for 0.30s" in result.stderr
    assert [report.reason for report in result.termination_reports] == [
        "output_stalled"
    ]
    assert result.descendants_closed is True


def test_run_guarded_output_bound_restarts_with_each_new_byte(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """Moving output keeps a command alive however long it runs.

    Silence accrues by the guard's planned poll wait, 0.02 s here. The child
    prints a line every three polls, and each poll waits until the child has
    printed the line due before it, so at most five polls (0.1 s) pass
    between two observed lines, under the 0.15 s bound. The run holds about
    two silent polls per line, 24 in all (about 0.5 s), so a count that is
    not reset by new output passes the bound, however loaded the host.
    """
    polls = tmp_path / "polls"
    lines = tmp_path / "lines"
    done = tmp_path / "done"
    bound = 0.15
    real_custody = memory_guard.sample_processes_custody
    count = [0]

    def published(path: Path) -> int:
        try:
            return int(path.read_text(encoding="utf-8") or 0)
        except FileNotFoundError:
            return 0

    def publish(path: Path, value: int) -> None:
        partial = path.with_suffix(".partial")
        partial.write_text(str(value), encoding="utf-8")
        os.replace(partial, path)

    def paced_custody(*args, **kwargs):
        # Poll k follows the child's line k // 3 - 1.
        while published(lines) < count[0] // 3 and not done.exists():
            time.sleep(0.005)
        samples = real_custody(*args, **kwargs)
        count[0] += 1
        publish(polls, count[0])
        return samples

    monkeypatch.setattr(memory_guard, "sample_processes_custody", paced_custody)
    # The handshake inside the sampler is not sampling cost to pace by.
    monkeypatch.setattr(memory_guard, "SAMPLE_COST_PACING_FACTOR", 0.0)
    child = (
        "import os, pathlib, sys, time\n"
        "polls, lines, done = map(pathlib.Path, sys.argv[1:4])\n"
        "def published(path):\n"
        "    try:\n"
        "        return int(path.read_text() or 0)\n"
        "    except FileNotFoundError:\n"
        "        return 0\n"
        "for line in range(12):\n"
        "    while published(polls) < 3 * line + 1:\n"
        "        time.sleep(0.005)\n"
        "    print(line, flush=True)\n"
        "    lines.with_suffix('.partial').write_text(str(line + 1))\n"
        "    os.replace(lines.with_suffix('.partial'), lines)\n"
        "done.write_text('done')\n"
    )

    result = memory_guard.run_guarded(
        [sys.executable, "-c", child, str(polls), str(lines), str(done)],
        max_rss_kb=1_000_000,
        poll_interval=0.02,
        no_output_timeout=bound,
    )

    assert result.timed_out is False, result.stderr
    assert result.returncode == 0
    assert result.stdout.split() == [str(line) for line in range(12)]


def test_run_guarded_output_bound_ignores_time_the_guard_did_not_watch(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    """A host freeze stops the guard and its child alike: it is no silence.

    The child prints, then waits while the host "freezes": the guard's clock
    leaps ten bounds between two polls and no output arrives. Once the guard
    has looked across the leap, the child resumes and prints again. A bound
    that counts the leap ends the command (exit 124); one that counts only
    the time the guard watched lets it finish. Events order every step.
    """
    frozen = tmp_path / "frozen"
    resume = tmp_path / "resume"
    bound = 30.0
    leap = [0.0]
    polls = [0]
    polled = threading.Condition()
    finished = threading.Event()
    real_custody = memory_guard.sample_processes_custody

    def counted_custody(*args, **kwargs):
        samples = real_custody(*args, **kwargs)
        with polled:
            polls[0] += 1
            polled.notify_all()
        return samples

    def after_polls(count: int) -> bool:
        with polled:
            target = polls[0] + count
            while polls[0] < target:
                if finished.is_set():
                    return False
                polled.wait(0.05)
        return True

    def freeze_the_host() -> None:
        while not frozen.exists():
            if finished.wait(0.005):
                return
        # The guard has seen the first line; then its clock leaps while the
        # child is silent, and it looks across the leap before the thaw.
        if after_polls(2):
            leap[0] += 10 * bound
            if after_polls(2):
                resume.touch()

    monkeypatch.setattr(memory_guard, "sample_processes_custody", counted_custody)
    install_module_view(
        monkeypatch,
        "time",
        time,
        memory_guard,
        perf_counter=lambda: time.perf_counter() + leap[0],
    )
    child = (
        "import pathlib, sys, time\n"
        "print('before', flush=True)\n"
        "pathlib.Path(sys.argv[1]).touch()\n"
        "while not pathlib.Path(sys.argv[2]).exists():\n"
        "    time.sleep(0.005)\n"
        "print('after', flush=True)\n"
    )
    thaw = threading.Thread(target=freeze_the_host)
    thaw.start()
    try:
        result = memory_guard.run_guarded(
            [sys.executable, "-c", child, str(frozen), str(resume)],
            max_rss_kb=1_000_000,
            poll_interval=0.02,
            no_output_timeout=bound,
        )
    finally:
        finished.set()
        resume.touch()  # A child left waiting ends.
        thaw.join(120)  # A hang guard only.

    assert result.timed_out is False, result.stderr
    assert result.returncode == 0
    assert result.stdout.split() == ["before", "after"]

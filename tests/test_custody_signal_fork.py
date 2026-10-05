from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from molt import file_locks as locks
from tools import memory_guard, process_sentinel as sentinel
from tools.memory_guard_core import process_custody as core


@pytest.mark.parametrize("replacement_leader", [False, True])
def test_dead_or_reused_sentinel_leader_never_grants_group_signal(
    monkeypatch, replacement_leader
):
    group = 90001
    member = memory_guard.ProcessSample(90002, 1, 64, "molt-backend", group, None, 200)
    unknown = memory_guard.ProcessSample(
        90003, 1, 64, "unknown worker", group, None, 300
    )
    samples = {member.pid: member, unknown.pid: unknown}
    expected = {member.pid: memory_guard.process_identity(member)}
    if replacement_leader:
        samples[group] = memory_guard.ProcessSample(
            group, 1, 64, "foreign replacement", group, None, 999
        )
        expected[group] = memory_guard.ProcessIdentity(100)
    monkeypatch.setattr(sentinel, "sample_processes_for_sentinel", lambda: samples)
    monkeypatch.setattr(sentinel, "_safe_getpgrp", lambda: 91000)
    monkeypatch.setattr(sentinel, "_is_windows_process_model", lambda: False)
    monkeypatch.setattr(
        sentinel, "protected_process_group_ids", lambda *_a, **_k: set()
    )
    monkeypatch.setattr(sentinel, "report_only_enabled", lambda: False)
    signals = []

    def signal_pid(pid, signum):
        signals.append(pid)
        return core._termination_action(
            target_kind="pid", target_id=pid, signum=signum, result="sent"
        )

    monkeypatch.setattr(core, "_send_pid_signal_action", signal_pid)
    monkeypatch.setattr(
        memory_guard,
        "_send_process_group_signal_if_identities_match_action",
        lambda *_a, **_k: pytest.fail("dead/reused leader authorised killpg"),
    )
    sentinel.terminate_group(
        group, grace=0.0, root=Path.cwd(), expected_identities=expected
    )
    assert signals == [member.pid, member.pid]


def test_posix_popen_audit_rejects_only_atomic_custody_scope(monkeypatch):
    monkeypatch.setattr(locks, "os", SimpleNamespace(name="posix"))
    locks._file_lock_fork_audit("subprocess.Popen", ())
    with locks._file_lock_atomic_mutation("descriptor publication"):
        with pytest.raises(RuntimeError, match="must be deferred"):
            locks._file_lock_fork_audit("subprocess.Popen", ())
    locks._file_lock_fork_audit("subprocess.Popen", ())


def test_windows_popen_remains_allowed_inside_atomic_scope():
    if os.name != "nt":
        pytest.skip("Windows spawn policy witness")
    with locks._file_lock_atomic_mutation("descriptor publication"):
        locks._file_lock_fork_audit("subprocess.Popen", ())


def test_failed_before_callback_cannot_release_unowned_parent_gate():
    condition = locks._FILE_LOCK_LIFECYCLE_CONDITION
    condition.acquire()
    try:
        with locks._file_lock_atomic_mutation("pending descriptor birth"):
            with pytest.raises(RuntimeError, match="atomic custody"):
                locks._before_file_lock_fork()
            locks._after_file_lock_fork_parent()
        assert condition._is_owned()
        assert getattr(locks._FILE_LOCK_ATOMIC_LOCAL, "fork_protocol_depth", 0) == 0
    finally:
        condition.release()


def test_quiescent_owned_lock_allows_normal_compiler_subprocess(tmp_path):
    handle = locks._try_acquire_file_lock(tmp_path / "compiler.lock")
    assert handle is not None
    try:
        result = subprocess.run(
            [sys.executable, "-c", "print('quiescent')"],
            capture_output=True,
            text=True,
            timeout=15,
        )
        assert result.returncode == 0 and result.stdout.strip() == "quiescent"
    finally:
        locks._release_file_lock(handle)


@pytest.mark.skipif(
    os.name != "posix", reason="actual CPython POSIX preexec fork witness"
)
def test_actual_posix_preexec_spawn_rejected_before_atfork_callbacks():
    from tests.process_guard_common import run_custody_subject_process

    code = r"""import subprocess,sys
from molt import file_locks as locks
with locks._file_lock_atomic_mutation("actual preexec boundary"):
    try:
        subprocess.Popen([sys.executable,"-c","pass"],preexec_fn=lambda:None)
    except RuntimeError as exc:
        assert "must be deferred" in str(exc)
    else:
        raise AssertionError("preexec spawn escaped atomic admission")
assert getattr(locks._FILE_LOCK_ATOMIC_LOCAL,"fork_protocol_depth",0)==0
print("preexec rejected")
"""
    result = run_custody_subject_process(
        [sys.executable, "-c", code], timeout=15, capture_output=True, text=True
    )
    assert result.returncode == 0 and "preexec rejected" in result.stdout


def test_birth_fenced_descendants_follow_moved_ancestors_and_reject_pid_reuse():
    from tools.memory_guard_core.process_model import birth_fenced_descendants

    parent = memory_guard.ProcessSample(90010, 1, 1, "moved", 90010, None, 100)
    child = memory_guard.ProcessSample(90011, 90010, 1, "child", 90011, None, 200)
    grandchild = memory_guard.ProcessSample(
        90012, 90011, 1, "grandchild", 90012, None, 300
    )
    unknown = memory_guard.ProcessSample(90013, 90011, 1, "unknown", 90013)
    impossible = memory_guard.ProcessSample(90014, 90011, 1, "older", 90014, None, 150)
    samples = {p.pid: p for p in (grandchild, unknown, impossible, child, parent)}
    owned, unresolved = birth_fenced_descendants(samples, {90010: 100})
    assert set(owned) == {90010, 90011, 90012}
    assert unresolved == {90013, 90014}
    assert birth_fenced_descendants(samples, {90010: 99}) == ({}, set())
    del samples[90010]
    assert birth_fenced_descendants(samples, {90010: 100}) == ({}, set())


@pytest.mark.parametrize("child_birth", [None, 0, -1, True, 200.0, "200", 99])
def test_birth_fenced_descendants_leave_invalid_edges_unresolved(child_birth):
    from tools.memory_guard_core.process_model import birth_fenced_descendants

    sample = memory_guard.ProcessSample
    samples = {
        300: sample(300, 200, 1, "behind invalid edge", started_at_ns=300),
        200: sample(200, 100, 1, "invalid birth", started_at_ns=child_birth),
        101: sample(101, 100, 1, "same clock tick", started_at_ns=100),
        100: sample(100, 1, 1, "observed parent", started_at_ns=100),
    }
    owned, unresolved = birth_fenced_descendants(samples, {100: 100})
    assert set(owned) == {100, 101}
    assert unresolved == {200}


def test_durable_transfer_does_not_reenter_command_tracker_and_allows_new_pid_birth():
    tracker = memory_guard.ProcessTreeTracker(90020)
    root = memory_guard.ProcessSample(90020, 1, 1, "command", 90020, None, 100)
    daemon = memory_guard.ProcessSample(90021, 90020, 1, "daemon", 90021, None, 200)
    moved = memory_guard.ProcessSample(90022, 90021, 1, "moved", 90022, None, 300)
    samples = {p.pid: p for p in (root, daemon, moved)}
    assert tracker.update(samples) == set(samples)
    expected = {p.pid: memory_guard.process_identity(p) for p in (daemon, moved)}
    assert tracker.transfer_process_group(
        daemon.pid, samples=samples, identities=expected
    )
    assert tracker.update(samples) == {root.pid}
    samples[daemon.pid] = memory_guard.ProcessSample(
        daemon.pid, root.pid, 1, "degraded", daemon.pid
    )
    assert tracker.update(samples) == {root.pid}
    samples[daemon.pid] = memory_guard.ProcessSample(
        daemon.pid, root.pid, 1, "new", daemon.pid, None, 400
    )
    assert tracker.update(samples) == {root.pid, daemon.pid}


def test_transfer_never_releases_command_root_as_a_moved_member():
    tracker = memory_guard.ProcessTreeTracker(90030)
    root = memory_guard.ProcessSample(90030, 1, 1, "command", 90030, None, 100)
    daemon = memory_guard.ProcessSample(90031, 90030, 1, "daemon", 90031, None, 200)
    samples = {root.pid: root, daemon.pid: daemon}
    expected = {p.pid: memory_guard.process_identity(p) for p in (root, daemon)}
    assert not tracker.transfer_process_group(
        daemon.pid, samples=samples, identities=expected
    )
    assert tracker.update(samples) == set(samples)

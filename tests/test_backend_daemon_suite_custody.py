from __future__ import annotations

from dataclasses import replace
import json
import os
from pathlib import Path
import sys

import pytest

from molt import backend_daemon_custody as daemon
from molt import backend_daemon_suite_custody as suite
from molt.exact_json import write_exact
from molt.file_locks import _try_acquire_file_lock, _release_file_lock
from tools import memory_guard

from tests.process_guard_common import run_custody_subject_process


def sample(pid, parent, group, born, command="owned worker"):
    return memory_guard.ProcessSample(
        pid=pid, ppid=parent, pgid=group, rss_kb=64, command=command, started_at_ns=born
    )


def deterministic_drain_clock(monkeypatch):
    # This checks metadata authority, not filesystem throughput. Real seal and
    # durable publication costs are measured separately; keep this deadline
    # deterministic so corruption cannot vacuously skip known-scope cleanup.
    from types import SimpleNamespace

    elapsed = 0.0

    def clock():
        nonlocal elapsed
        elapsed += 0.001
        return elapsed

    def sleep(delay):
        nonlocal elapsed
        elapsed += delay

    monkeypatch.setattr(suite, "time", SimpleNamespace(perf_counter=clock, sleep=sleep))


@pytest.fixture
def lease(tmp_path, monkeypatch):
    root = tmp_path / "daemon"
    path = root / "suite-leases" / ("a" * 32) / "lease.json"
    path.parent.mkdir(parents=True)
    record = {
        "schema": suite.SCHEMA,
        "token": "a" * 32,
        "state": "active",
        "owner_pid": 10,
        "owner_started_at_ns": 100,
        "guardian_pid": os.getpid(),
        "guardian_started_at_ns": 200,
        "project_root": str(tmp_path),
        "daemon_root": str(root),
        "source_digest": suite._source_digest(),
        "interpreter": sys.executable,
        "interpreter_sha256": "b" * 64,
    }
    write_exact(path.parent / "adoption-index.json", suite._new_adoption_index(record))
    write_exact(path, record)
    monkeypatch.setattr(
        daemon,
        "process_started_at_ns",
        lambda pid: {10: 100, os.getpid(): 200}.get(pid),
    )
    handle = _try_acquire_file_lock(path.parent / "owner.lock")
    assert handle is not None
    yield path, record, handle
    _release_file_lock(handle)


def lease_samples(record):
    return {
        10: sample(10, 1, 10, 100),
        record["guardian_pid"]: sample(
            record["guardian_pid"], 10, record["guardian_pid"], 200
        ),
    }


def identity(tmp_path, lease_path):
    binary = tmp_path / "molt-backend"
    return daemon.BackendDaemonIdentity(
        pid=30,
        socket_path=tmp_path / "daemon.sock",
        project_root=tmp_path,
        cargo_profile="dev-fast",
        config_digest="c" * 64,
        backend_bin=binary,
        created_at=1.0,
        started_at_ns=300,
        backend_sha256="d" * 64,
        suite_lease=str(lease_path),
        command=f"{binary} --daemon --socket {tmp_path / 'daemon.sock'}",
    )


def test_live_suite_lease_requires_both_births_and_os_lock(lease, tmp_path):
    path, record, _handle = lease
    samples = lease_samples(record)
    assert suite.live_lease(path, project_root=tmp_path, samples=samples) == record
    for pid in samples:
        reused = dict(samples)
        reused[pid] = replace(
            samples[pid], started_at_ns=samples[pid].started_at_ns + 1
        )
        assert suite.live_lease(path, project_root=tmp_path, samples=reused) is None
    assert (
        suite.live_lease(path, project_root=tmp_path / "other", samples=samples) is None
    )


def test_unheld_or_closed_lease_cannot_adopt(tmp_path):
    path = tmp_path / "daemon" / "suite-leases" / ("a" * 32) / "lease.json"
    path.parent.mkdir(parents=True)
    record = {
        "schema": suite.SCHEMA,
        "token": "a" * 32,
        "state": "active",
        "owner_pid": 10,
        "owner_started_at_ns": 100,
        "guardian_pid": os.getpid(),
        "guardian_started_at_ns": 200,
        "project_root": str(tmp_path),
        "daemon_root": str(path.parents[2]),
        "source_digest": suite._source_digest(),
        "interpreter": sys.executable,
        "interpreter_sha256": "b" * 64,
    }
    write_exact(path.parent / "adoption-index.json", suite._new_adoption_index(record))
    write_exact(path, record)
    handle = _try_acquire_file_lock(path.parent / "owner.lock")
    assert handle is not None
    _release_file_lock(handle)
    assert (
        suite.live_lease(path, project_root=tmp_path, samples=lease_samples(record))
        is None
    )


def test_explicit_transfer_preserves_daemon_and_workers_but_keeps_unrelated_child(
    lease, tmp_path
):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "molt-backend.test.identity.json", owned
    )
    before = {
        # Births must be ordered parent <= child or ancestry is refused.
        1000: sample(1000, 10, 1000, 250),
        30: sample(30, 1000, 30, 300, owned.command),
        31: sample(31, 30, 30, 310),
        40: sample(40, 1000, 40, 400),
    }
    tracker = memory_guard.ProcessTreeTracker(1000)
    tracker.update(before)
    after = {
        **lease_samples(record),
        30: replace(before[30], ppid=1),
        31: before[31],
        40: replace(before[40], ppid=1),
    }
    groups = suite.transferable_groups(
        {suite.LEASE_ENV: str(path)}, project_root=tmp_path, samples=after
    )
    assert len(groups) == 1
    _lease, receipt, members = groups[0]
    assert receipt.started_at_ns == 300
    assert tracker.transfer_process_group(
        30,
        samples=after,
        identities={
            pid: memory_guard.process_identity(value) for pid, value in members.items()
        },
    )
    # A subsequent cleanup sample still watches the unregistered leftover.
    assert tracker.update(after) == {40}
    assert not tracker.transfer_process_group(
        1000,
        samples=before,
        identities={1000: memory_guard.process_identity(before[1000])},
    )


@pytest.mark.parametrize(
    "field,value",
    [
        ("started_at_ns", 301),
        ("config_digest", ""),
        ("backend_sha256", None),
        ("suite_lease", "foreign"),
    ],
)
def test_daemon_adoption_rejects_reused_or_unbound_identity(
    lease, tmp_path, field, value
):
    path, record, _handle = lease
    valid = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "molt-backend.test.identity.json",
        replace(valid, **{field: value}),
    )
    samples = {**lease_samples(record), 30: sample(30, 1, 30, 300, valid.command)}
    assert (
        suite.transferable_groups(
            {suite.LEASE_ENV: str(path)}, project_root=tmp_path, samples=samples
        )
        == ()
    )


def test_persistent_daemon_cannot_use_command_scratch(tmp_path):
    scratch = tmp_path / "command"
    external = tmp_path / "suite-temp"
    env = {
        "MOLT_GUARD_SCRATCH_ROOT": str(scratch),
        "MOLT_MEMORY_GUARD_TOKEN": "token",
        "MOLT_MEMORY_GUARD_MARKER": "marker",
        "PYTEST_DEBUG_TEMPROOT": str(scratch),
        "TMPDIR": str(scratch / "nested"),
        "TEMP": str(external),
        suite.LEASE_ENV: "owner",
        "MOLT_BACKEND_MAX_PROCESS_RSS_GB": "1",
    }
    clean = suite.persistent_daemon_env(env)
    assert clean == {
        "TEMP": str(external),
        suite.LEASE_ENV: "owner",
        "MOLT_BACKEND_MAX_PROCESS_RSS_GB": "1",
    }
    assert env["MOLT_GUARD_SCRATCH_ROOT"] == str(scratch)


def test_eof_guardian_drains_only_its_registered_births(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "molt-backend.test.identity.json", owned
    )
    foreign = replace(owned, pid=50, started_at_ns=500, suite_lease="other")
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "molt-backend.foreign.identity.json", foreign
    )
    samples = {
        os.getpid(): sample(os.getpid(), 10, os.getpid(), 200),
        30: sample(30, 1, 30, 300, owned.command),
        31: sample(31, 30, 30, 310),
        50: sample(50, 1, 50, 500, foreign.command),
    }
    monkeypatch.setattr(daemon, "process_started_at_ns", lambda pid: 200)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: dict(samples))
    signaled = []

    def terminate(pid, **kwargs):
        signaled.extend(sorted(kwargs["watched"]))
        assert kwargs["expected_identities"] == {
            member: memory_guard.process_identity(samples[member])
            for member in [30, 31]
        }
        for member in kwargs["watched"]:
            samples.pop(member)

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", terminate)
    assert suite.drain_lease(path)
    assert signaled == [30, 31]
    assert 50 in samples
    assert (
        json.loads((path.parent / "drain.json").read_text(encoding="utf-8"))["closed"]
        is True
    )


def test_birth_bound_daemon_reuse_never_signals_replacement(tmp_path, monkeypatch):
    owned = identity(tmp_path, "suite")
    monkeypatch.setattr(daemon, "process_started_at_ns", lambda pid: 301)
    monkeypatch.setattr(daemon, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(daemon, "_process_command", lambda pid: owned.command)
    assert not daemon.backend_daemon_identity_is_verified(
        owned, allow_health_probe=False
    )
    monkeypatch.setattr(
        memory_guard,
        "sample_processes",
        lambda: {30: sample(30, 1, 30, 301, owned.command)},
    )
    monkeypatch.setattr(
        memory_guard,
        "terminate_verified_pid",
        lambda *_a, **_k: pytest.fail("reused PID was signaled"),
    )
    assert not daemon.terminate_backend_daemon_identity(owned)


@pytest.mark.skipif(
    os.name != "posix", reason="POSIX backend daemon and pass_fds custody"
)
def test_owned_suite_guardian_closes_on_eof_without_backend_build(tmp_path):
    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(tmp_path / "target")
    env["PYTHONPATH"] = str(root / "src") + os.pathsep + str(root)
    active = suite.SuiteDaemonLease.start(project_root=root, environ=env)
    assert active is not None
    assert not os.get_inheritable(active.write_fd)
    active.close()
    assert active.guardian.poll() == 0
    closed_record = suite.read_lease(active.path)
    assert closed_record is not None
    assert closed_record["state"] == "closed"
    assert (
        json.loads((active.path.parent / "drain.json").read_text(encoding="utf-8"))[
            "closed"
        ]
        is True
    )


def test_fork_descriptor_cleanup_closes_both_pipe_ends_without_unlock(monkeypatch):
    import threading

    closed = []
    monkeypatch.setattr(suite, "_LEASE_PIPE_FDS", {17, 19})
    monkeypatch.setattr(suite, "_LEASE_DESCRIPTOR_MUTEX", threading.Lock())
    monkeypatch.setattr(suite.os, "close", closed.append)
    monkeypatch.setattr(
        suite, "_release_file_lock", lambda _lock: pytest.fail("parent lease unlocked")
    )
    suite._close_inherited_lease_descriptors()
    assert set(closed) == {17, 19}
    assert suite._LEASE_PIPE_FDS == set()


def test_inherited_python_lease_cannot_teardown_parent(tmp_path, monkeypatch):
    import subprocess
    from typing import cast

    active = suite.SuiteDaemonLease(
        tmp_path / "lease.json",
        {"owner_pid": os.getpid() + 1},
        None,
        17,
        cast(subprocess.Popen, object()),
    )
    monkeypatch.setattr(
        suite.os, "close", lambda _fd: pytest.fail("parent pipe closed")
    )
    active.close()
    assert active.write_fd == 17


def test_guardian_failure_still_runs_bounded_suite_fallback(tmp_path, monkeypatch):
    monkeypatch.setenv(suite.LEASE_ENV, "isolated-test-lease")
    from types import SimpleNamespace
    from tools import harness_memory_guard

    sentinel = harness_memory_guard.repo_process_sentinel(
        repo_root=tmp_path,
        artifact_root=tmp_path,
        label="guardian-failure",
        limits=harness_memory_guard.HarnessMemoryLimits(
            enabled=True,
            max_process_rss_gb=1,
            max_total_rss_gb=2,
            max_global_rss_gb=3,
            poll_interval=0.01,
        ),
        drain_max_runtime_sec=5.0,
        suppress_auto_guard=False,
    )
    calls = []

    def fail(**kwargs):
        raise RuntimeError("guardian failed")

    sentinel._daemon_suite_lease = SimpleNamespace(close=fail)
    monkeypatch.setattr(
        sentinel,
        "drain_new_processes",
        lambda: calls.append("bounded receiver drain") or 0,
    )
    with pytest.raises(RuntimeError, match="guardian failed"):
        sentinel.__exit__(None, None, None)
    assert calls == ["bounded receiver drain"]


@pytest.mark.skipif(os.name != "posix", reason="real fork ownership requires POSIX")
def test_fork_child_does_not_keep_suite_eof_alive(tmp_path):
    import signal
    import time

    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(tmp_path / "target")
    env["PYTHONPATH"] = str(root / "src") + os.pathsep + str(root)
    active = suite.SuiteDaemonLease.start(project_root=root, environ=env)
    assert active is not None
    child = getattr(os, "fork")()
    if child == 0:
        # Close inherited Python object too: this must be inert, never unlock.
        active.close()
        time.sleep(10)
        os._exit(0)
    try:
        active.close(timeout=3)
        assert active.guardian.returncode == 0
        assert os.waitpid(child, getattr(os, "WNOHANG")) == (0, 0)
    finally:
        os.kill(child, signal.SIGTERM)  # This test owns exactly this child.
        os.waitpid(child, 0)


@pytest.mark.parametrize(
    "field,value",
    [
        ("unexpected", True),
        ("source_digest", "invalid"),
        ("interpreter_sha256", "bad"),
        ("owner_pid", True),
    ],
)
def test_lease_metadata_is_fail_closed(lease, field, value):
    path, record, _ = lease
    write_exact(path, {**record, field: value})
    assert suite.read_lease(path) is None


def test_live_leader_drains_birth_verified_reparented_worker(
    lease, tmp_path, monkeypatch
):
    path, record, _ = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "owned.identity.json", owned
    )
    samples = {
        os.getpid(): sample(os.getpid(), 10, os.getpid(), 200),
        30: sample(30, 1, 30, 300, owned.command),
        31: sample(31, 1, 30, 310),
        50: sample(50, 1, 50, 500),
    }
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: dict(samples))
    signaled = []

    def terminate(pid, **kwargs):
        assert kwargs["root_owned"] is True
        assert kwargs["expected_identities"] == {
            member: memory_guard.process_identity(samples[member])
            for member in [30, 31]
        }
        for member in kwargs["watched"]:
            signaled.append(member)
            samples.pop(member)

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", terminate)
    assert suite.drain_lease(path)
    assert set(signaled) == {30, 31}
    assert 50 in samples


def test_transient_snapshot_error_retries_and_writes_receipt(
    lease, tmp_path, monkeypatch
):
    path, record, _ = lease
    samples = {os.getpid(): sample(os.getpid(), 10, os.getpid(), 200)}
    import threading

    calls = []
    drain_thread = threading.get_ident()

    def sampler():
        # A composed harness may sample concurrently; inject the fault into
        # this drain, rather than letting an unrelated observer consume it.
        if threading.get_ident() == drain_thread:
            calls.append(True)
            if len(calls) == 1:
                raise memory_guard.ProcessSnapshotError("transient snapshot")
        return samples

    monkeypatch.setattr(memory_guard, "sample_processes", sampler)
    assert suite.drain_lease(path)
    receipt = json.loads((path.parent / "drain.json").read_text(encoding="utf-8"))
    assert receipt["closed"] is True
    assert receipt["sampling_errors"] == ["ProcessSnapshotError: transient snapshot"]


def test_persistent_snapshot_failure_is_bounded_and_fail_closed(lease, monkeypatch):
    path, _, _ = lease
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 0.2)

    def sampler():
        raise memory_guard.ProcessSnapshotError("unavailable snapshot")

    monkeypatch.setattr(memory_guard, "sample_processes", sampler)
    assert not suite.drain_lease(path)
    receipt = json.loads((path.parent / "drain.json").read_text(encoding="utf-8"))
    assert receipt["closed"] is False
    assert receipt["sampling_errors"]


def test_dead_leader_never_claims_unobserved_worker_custody(
    lease, tmp_path, monkeypatch
):
    path, record, _ = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "owned.identity.json", owned
    )
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 0.2)
    samples = {
        os.getpid(): sample(os.getpid(), 10, os.getpid(), 200),
        31: sample(31, 1, 30, 310),
    }
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        memory_guard,
        "terminate_watched_processes",
        lambda *_a, **_k: pytest.fail("unproven orphan signaled"),
    )
    assert not suite.drain_lease(path)
    assert json.loads((path.parent / "drain.json").read_text(encoding="utf-8"))[
        "unresolved_pgids"
    ] == [30]


def test_birth_probe_snapshot_error_is_unavailable(monkeypatch):
    monkeypatch.setattr(daemon, "_load_memory_guard_module", lambda: memory_guard)

    def fail():
        raise memory_guard.ProcessSnapshotError("snapshot unavailable")

    monkeypatch.setattr(memory_guard, "sample_processes", fail)
    assert daemon.process_started_at_ns(30) is None


@pytest.mark.skipif(
    os.name != "posix", reason="real guarded daemon-scope transfer requires POSIX"
)
@pytest.mark.parametrize("exit_code", [0, 7])
def test_guarded_protocol_daemon_transfer_and_failure_cleanup(tmp_path, exit_code):
    """Exercise actual guard/guardian protocol; this is not a compiler benchmark."""
    import textwrap

    root = Path(__file__).resolve().parents[1]
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(tmp_path / "target")
    env["PYTHONPATH"] = str(root / "src") + os.pathsep + str(root)
    active = suite.SuiteDaemonLease.start(project_root=root, environ=env)
    assert active is not None
    env[suite.LEASE_ENV] = str(active.path)
    env["OWNED_PROTOCOL_PID_FILE"] = str(tmp_path / "daemon.pid")
    env["OWNED_PROTOCOL_EXIT"] = str(exit_code)
    script = textwrap.dedent("""
        import os, subprocess, sys, time
        from pathlib import Path
        from molt import backend_daemon_custody as custody
        from molt.backend_daemon_suite_custody import (
            acknowledge_started_daemon, persistent_daemon_env,
        )
        root=Path.cwd()
        socket=Path(os.environ['OWNED_PROTOCOL_PID_FILE']).with_suffix('.sock')
        proc=subprocess.Popen([sys.executable,'-I','-S','-c','import time; time.sleep(30)','--daemon','--socket',str(socket)],
            start_new_session=True, env=persistent_daemon_env(os.environ),
            stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
        identity=custody.backend_daemon_identity_for_pid(proc.pid,socket_path=socket,
            project_root=root,cargo_profile='dev-fast',config_digest='c'*64,backend_bin=Path(sys.executable))
        assert identity.started_at_ns is not None
        custody.write_backend_daemon_identity(custody.backend_daemon_root_from_env(os.environ,project_root=root)/'protocol.identity.json',identity)
        if int(os.environ['OWNED_PROTOCOL_EXIT']) == 0:
            assert acknowledge_started_daemon(os.environ, project_root=root, daemon_pid=proc.pid)
        Path(os.environ['OWNED_PROTOCOL_PID_FILE']).write_text(str(proc.pid))
        time.sleep(0.4)
        sys.exit(int(os.environ['OWNED_PROTOCOL_EXIT']))
    """)
    try:
        result = memory_guard.run_guarded(
            [sys.executable, "-c", script],
            env=env,
            cwd=root,
            max_rss_kb=512_000,
            max_total_rss_kb=1_024_000,
            poll_interval=0.02,
            timeout=10,
        )
        assert result.returncode == exit_code, result.stderr
        pid = int((tmp_path / "daemon.pid").read_text(encoding="utf-8"))
        assert result.temporary_artifacts is not None
        closure = result.temporary_artifacts["closure"]
        assert isinstance(closure, dict)
        transfers = closure.get("suite_custody_transfers", [])
        if exit_code == 0:
            assert [item["pgid"] for item in transfers] == [pid]
            assert pid in memory_guard.sample_processes()
        else:
            assert transfers == []
            assert pid not in memory_guard.sample_processes()
    finally:
        active.close()
    assert (
        json.loads((active.path.parent / "drain.json").read_text(encoding="utf-8"))[
            "closed"
        ]
        is True
    )


def acknowledge(lease, tmp_path, *, daemon_root=None):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    operational = (
        daemon_root or Path(record["daemon_root"])
    ) / "molt-backend.test.identity.json"
    daemon.write_backend_daemon_identity(operational, owned)
    samples = {
        **lease_samples(record),
        30: sample(30, 1, 30, 300, owned.command),
        31: sample(31, 30, 30, 310),
    }
    assert (
        len(
            suite.registered_groups(
                path, lease=record, samples=samples, daemon_root=daemon_root
            )
        )
        == 1
    )
    return owned, operational, samples


def mocked_drain(monkeypatch, samples):
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: dict(samples))
    calls = []

    def terminate(pid, **kwargs):
        calls.append((pid, kwargs))
        for member in kwargs["watched"]:
            assert (
                samples[member].started_at_ns
                == kwargs["expected_identities"][member].started_at_ns
            )
            samples.pop(member)

    def terminate_individual(pid, marker, **kwargs):
        assert samples[pid].started_at_ns == marker.started_at_ns
        calls.append(
            (
                pid,
                {
                    "watched": {pid},
                    "root_owned": False,
                    "expected_identities": {pid: marker},
                },
            )
        )
        samples.pop(pid)
        return ()

    monkeypatch.setattr(memory_guard, "terminate_watched_processes", terminate)
    monkeypatch.setattr(memory_guard, "terminate_verified_pid", terminate_individual)
    return calls


def test_adoption_survives_operational_receipt_deletion(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert calls[0][1]["watched"] == {30, 31}
    assert calls[0][1]["root_owned"] is True
    assert suite._adoption_path(path, _owned).exists()


def test_dead_root_only_receipted_births_are_signaled(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples.pop(30)
    samples[31] = replace(samples[31], ppid=1, pgid=71)
    samples[50] = sample(50, 1, 50, 500)
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert calls[0][1]["watched"] == {31}
    assert calls[0][1]["root_owned"] is False
    assert 50 in samples


def test_dead_root_unknown_members_remain_unresolved(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples.pop(30)
    samples[32] = sample(32, 1, 30, 320)
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 0.2)
    deterministic_drain_clock(monkeypatch)
    calls = mocked_drain(monkeypatch, samples)
    assert not suite.drain_lease(path)
    assert calls[0][1]["watched"] == {31}
    assert 32 in samples
    closure = json.loads((path.parent / "drain.json").read_text(encoding="utf-8"))
    assert closure["unresolved_pgids"] == [30]


def test_adopted_worker_pid_reuse_does_not_authorize_signal(
    lease, tmp_path, monkeypatch
):
    path, record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples.pop(30)
    samples[31] = replace(samples[31], started_at_ns=999, pgid=31)
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert calls == []
    assert samples[31].started_at_ns == 999


@pytest.mark.parametrize(
    "damage",
    [
        "receipt-delete",
        "index-delete",
        "token",
        "source",
        "project",
        "member",
        "digest",
        "index-schema",
    ],
)
def test_damaged_acknowledgement_fails_export_and_closure(
    lease, tmp_path, monkeypatch, damage
):
    path, record, _handle = lease
    owned, operational, samples = acknowledge(lease, tmp_path)
    receipt = suite._adoption_path(path, owned)
    index = path.parent / "adoption-index.json"
    if damage == "receipt-delete":
        receipt.unlink()
    elif damage == "index-delete":
        index.unlink()
    elif damage in {"digest", "index-schema"}:
        payload = json.loads(index.read_text(encoding="utf-8"))
        if damage == "digest":
            payload["receipts"][receipt.name] = "0" * 64
        else:
            payload["schema"] = "wrong"
        write_exact(index, payload)
    else:
        payload = json.loads(receipt.read_text(encoding="utf-8"))
        if damage == "token":
            payload["lease_token"] = "f" * 32
        elif damage == "source":
            payload["source_digest"] = "e" * 64
        elif damage == "project":
            payload["project_root"] = str(tmp_path / "foreign")
        else:
            payload["custody_members"]["31"] = True
        write_exact(receipt, payload)
    assert suite.registered_groups(path, lease=record, samples=samples) == ()
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 0.2)
    deterministic_drain_clock(monkeypatch)
    calls = mocked_drain(monkeypatch, samples)
    assert not suite.drain_lease(path)
    assert calls and all(call[1]["watched"] <= {30, 31} for call in calls)
    assert 30 not in samples and 31 not in samples


def test_command_root_divergence_uses_receiving_ledger(lease, tmp_path, monkeypatch):
    path, _record, _handle = lease
    _owned, operational, samples = acknowledge(
        lease, tmp_path, daemon_root=tmp_path / "canonical-command-root"
    )
    operational.unlink()
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert calls[0][1]["watched"] == {30, 31}


def test_interrupted_index_publication_never_exports(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "molt-backend.test.identity.json", owned
    )
    samples = {**lease_samples(record), 30: sample(30, 1, 30, 300, owned.command)}
    actual_write = suite.write_exact

    def fail_index(target, payload, **kwargs):
        if target.name == "adoption-index.json":
            raise OSError("injected interrupted index publication")
        actual_write(target, payload, **kwargs)

    monkeypatch.setattr(suite, "write_exact", fail_index)
    assert suite.registered_groups(path, lease=record, samples=samples) == ()
    assert not suite._adoption_path(path, owned).exists()
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 0.2)
    calls = mocked_drain(monkeypatch, samples)
    assert not suite.drain_lease(path)
    assert calls and all(call[1]["watched"] == {30} for call in calls)
    assert 30 not in samples


@pytest.mark.parametrize("event", ["os.fork", "os.forkpty"])
def test_suite_descriptor_mutation_rejects_audited_fork_before_callbacks(event):
    with suite._lease_descriptor_mutation():
        with pytest.raises(RuntimeError, match="atomic custody operation"):
            sys.audit(event)
        sys.audit("molt.test.harmless")
    sys.audit(event)  # Manual event: no OS process is created; quiescent fork allowed.


@pytest.mark.parametrize("event", ["os.fork", "os.forkpty"])
def test_suite_fork_protocol_rejects_nested_audit_then_restores(event):
    suite._before_lease_fork()
    try:
        with pytest.raises(RuntimeError, match="fork lifecycle callback"):
            sys.audit(event)
    finally:
        suite._after_parent_lease_fork()
    sys.audit(event)


def test_child_pipe_cleanup_marks_protocol_and_restores(monkeypatch):
    import threading

    monkeypatch.setattr(suite, "_LEASE_PIPE_FDS", {17, 19})
    monkeypatch.setattr(suite, "_LEASE_DESCRIPTOR_MUTEX", threading.Lock())
    closed = []

    def close(fd):
        with pytest.raises(RuntimeError, match="fork lifecycle callback"):
            sys.audit("os.fork")
        closed.append(fd)

    monkeypatch.setattr(suite.os, "close", close)
    suite._close_inherited_lease_descriptors()
    assert sorted(closed) == [17, 19]
    assert suite._LEASE_PIPE_FDS == set()
    sys.audit("os.fork")


@pytest.mark.parametrize("error_type", [RuntimeError, KeyboardInterrupt])
def test_child_pipe_exception_attempts_all_copies_and_fails_custody_closed(
    lease, monkeypatch, error_type
):
    import threading

    path, _record, _handle = lease
    monkeypatch.setattr(suite, "_LEASE_PIPE_FDS", {17, 19})
    inherited_mutex = threading.Lock()
    inherited_mutex.acquire()
    monkeypatch.setattr(suite, "_LEASE_DESCRIPTOR_MUTEX", inherited_mutex)
    monkeypatch.setattr(suite, "_LEASE_CHILD_CUSTODY_ERROR", None)
    attempts = []

    def close(fd):
        attempts.append(fd)
        if fd == 17:
            raise error_type("injected inherited pipe close failure")

    monkeypatch.setattr(suite.os, "close", close)
    suite._close_inherited_lease_descriptors()
    assert sorted(attempts) == [17, 19]
    assert suite._LEASE_PIPE_FDS == set()
    assert not suite._LEASE_DESCRIPTOR_MUTEX.locked()
    assert suite.read_lease(path) is None
    with pytest.raises(RuntimeError, match="child custody is unavailable"):
        with suite._lease_descriptor_mutation():
            pytest.fail("failed child must not acquire suite descriptor custody")
    sys.audit("os.fork")
    prior = suite._LEASE_CHILD_CUSTODY_ERROR
    suite._close_inherited_lease_descriptors()
    assert suite._LEASE_CHILD_CUSTODY_ERROR == prior


def test_live_leader_drains_receipted_worker_outside_current_group(
    lease, tmp_path, monkeypatch
):
    path, _record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples[31] = replace(samples[31], ppid=1, pgid=71)
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert [call[1]["watched"] for call in calls] == [{30}, {31}]
    assert calls[1][1]["root_owned"] is False
    assert 31 not in samples


def test_reused_leader_group_is_foreign_not_unresolved(lease, tmp_path, monkeypatch):
    path, _record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples.pop(31)
    samples[30] = sample(30, 1, 30, 999)
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert calls == []
    assert samples[30].started_at_ns == 999


def test_dead_leader_uses_real_individual_birth_gate_never_group_signal(
    lease, tmp_path, monkeypatch
):
    from tools.memory_guard_core import process_custody

    path, _record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples.pop(30)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: dict(samples))
    monkeypatch.setattr(
        memory_guard,
        "terminate_watched_processes",
        lambda *_a, **_kw: pytest.fail("dead leader group API used"),
    )
    signals = []

    def kill(pid, sig):
        assert pid == 31
        if sig == 0:
            if pid not in samples:
                raise ProcessLookupError(pid)
            return
        signals.append((pid, sig))
        samples.pop(pid)

    monkeypatch.setattr(process_custody.os, "kill", kill)
    monkeypatch.setattr(
        process_custody.os,
        "killpg",
        lambda *_args: pytest.fail("dead-root killpg used"),
        raising=False,
    )
    assert suite.drain_lease(path)
    assert len(signals) == 1 and signals[0][0] == 31


def test_adoption_after_completed_drain_is_revoked(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    owned, _operational, samples = acknowledge(lease, tmp_path)
    mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    samples[30] = sample(30, 1, 30, 300, owned.command)
    assert not suite._record_adoption(
        path, lease=record, identity=owned, members={30: samples[30]}
    )
    assert (
        json.loads((path.parent / "adoption-index.json").read_text(encoding="utf-8"))[
            "state"
        ]
        == "closed"
    )


@pytest.mark.parametrize("state", ["closing", "closed"])
def test_valid_lease_state_revokes_admission(lease, tmp_path, state):
    path, record, _handle = lease
    record["state"] = state
    write_exact(path, record)
    assert suite.read_lease(path) is not None
    assert (
        suite.live_lease(path, project_root=tmp_path, samples=lease_samples(record))
        is None
    )


def test_valid_lease_without_held_owner_lock_is_rejected(lease, tmp_path):
    path, record, handle = lease
    _release_file_lock(handle)
    assert suite.read_lease(path) == record
    assert (
        suite.live_lease(path, project_root=tmp_path, samples=lease_samples(record))
        is None
    )


def test_adoption_cap_uses_actual_formatted_publication_bytes(
    lease, tmp_path, monkeypatch
):
    from molt.exact_json import canonical_json_bytes, encode_exact

    path, record, _handle = lease
    owned = identity(tmp_path, path)
    members = {
        30: sample(30, 1, 30, 300, owned.command),
        **{pid: sample(pid, 30, 30, pid * 10) for pid in range(40, 75)},
    }
    payload = {
        **daemon.backend_daemon_identity_payload(owned),
        "adoption_schema": suite.ADOPTION_SCHEMA,
        "lease_token": record["token"],
        "source_digest": record["source_digest"],
        "custody_members": {
            str(pid): item.started_at_ns for pid, item in members.items()
        },
    }
    compact, published = len(canonical_json_bytes(payload)), len(encode_exact(payload))
    assert compact < published
    monkeypatch.setattr(suite, "_ADOPTION_BYTES", (compact + published) // 2)
    assert not suite._record_adoption(
        path, lease=record, identity=owned, members=members
    )
    assert not suite._adoption_path(path, owned).exists()


@pytest.mark.parametrize(
    "field,value", [("backend_sha256", None), ("config_digest", "")]
)
def test_adoption_writer_rejects_reader_invalid_identity(lease, tmp_path, field, value):
    path, record, _handle = lease
    owned = replace(identity(tmp_path, path), **{field: value})
    assert not suite._record_adoption(
        path,
        lease=record,
        identity=owned,
        members={30: sample(30, 1, 30, 300, owned.command)},
    )
    assert not suite._adoption_path(path, owned).exists()


@pytest.mark.parametrize(
    "damage",
    ["lease_token", "source_digest", "project_root", "member-bool", "leader-missing"],
)
def test_adoption_inner_schema_rejects_even_matching_content_digest(
    lease, tmp_path, damage
):
    import hashlib
    from molt.exact_json import canonical_json_bytes

    path, record, _handle = lease
    owned, _operational, samples = acknowledge(lease, tmp_path)
    receipt = suite._adoption_path(path, owned)
    payload = json.loads(receipt.read_text(encoding="utf-8"))
    if damage == "project_root":
        payload[damage] = str(tmp_path / "foreign")
    elif damage == "member-bool":
        payload["custody_members"]["31"] = True
    elif damage == "leader-missing":
        payload["custody_members"].pop("30")
    else:
        payload[damage] = "f" * (32 if damage == "lease_token" else 64)
    write_exact(receipt, payload)
    indexpath = path.parent / "adoption-index.json"
    index = json.loads(indexpath.read_text(encoding="utf-8"))
    index["receipts"][receipt.name] = hashlib.sha256(
        canonical_json_bytes(payload)
    ).hexdigest()
    write_exact(indexpath, index)
    assert suite.registered_groups(path, lease=record, samples=samples) == ()


def test_adoption_waits_bounded_transaction_contention(lease, tmp_path):
    import threading

    path, record, _handle = lease
    _owned, _operational, samples = acknowledge(lease, tmp_path)
    held = threading.Event()
    release = threading.Event()
    errors = []

    def hold():
        lock = _try_acquire_file_lock(path.parent / "adoption-transaction.lock")
        try:
            assert lock is not None
            held.set()
            assert release.wait(5)
        except BaseException as exc:
            errors.append(exc)
        finally:
            if lock is not None:
                _release_file_lock(lock)

    thread = threading.Thread(target=hold)
    thread.start()
    assert held.wait(5)
    timer = threading.Timer(0.05, release.set)
    timer.start()
    try:
        assert len(suite.registered_groups(path, lease=record, samples=samples)) == 1
    finally:
        release.set()
        thread.join(5)
        timer.join(5)
    assert not errors and not thread.is_alive()


def test_staged_receipt_recovers_after_final_commit_interruption(
    lease, tmp_path, monkeypatch
):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    operational = Path(record["daemon_root"]) / "owned.identity.json"
    daemon.write_backend_daemon_identity(operational, owned)
    samples = {
        **lease_samples(record),
        30: sample(30, 1, 30, 300, owned.command),
        31: sample(31, 30, 30, 310),
    }
    original = suite.write_exact
    count = 0

    def interrupt_final(target, payload, **kwargs):
        nonlocal count
        if target.name == "adoption-index.json":
            count += 1
            if count == 2:
                raise OSError("interrupted final commit")
        original(target, payload, **kwargs)

    monkeypatch.setattr(suite, "write_exact", interrupt_final)
    assert suite.registered_groups(path, lease=record, samples=samples) == ()
    receipt = suite._adoption_path(path, owned)
    assert receipt.exists()
    staged = json.loads(
        (path.parent / "adoption-index.json").read_text(encoding="utf-8")
    )
    assert receipt.name in staged["pending"] and receipt.name not in staged["receipts"]
    monkeypatch.setattr(suite, "write_exact", original)
    operational.unlink()
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    committed = json.loads(
        (path.parent / "adoption-index.json").read_text(encoding="utf-8")
    )
    assert committed["pending"] == {} and receipt.name in committed["receipts"]
    assert calls[0][1]["watched"] == {30, 31}


def test_staged_update_recovers_old_committed_generation(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    owned, _operational, samples = acknowledge(lease, tmp_path)
    before = suite._adoption_path(path, owned).read_bytes()
    samples[32] = sample(32, 30, 30, 320)
    original = suite.write_exact

    def interrupt_receipt(target, payload, **kwargs):
        if target.name.endswith(".identity.json"):
            raise OSError("interrupted receipt update")
        original(target, payload, **kwargs)

    monkeypatch.setattr(suite, "write_exact", interrupt_receipt)
    assert suite.registered_groups(path, lease=record, samples=samples) == ()
    assert suite._adoption_path(path, owned).read_bytes() == before
    monkeypatch.setattr(suite, "write_exact", original)
    records, errors = suite._custody_records(path, lease=record)
    assert errors == [] and len(records) == 1
    assert records[0][1] == {30: 300, 31: 310}
    assert (
        json.loads((path.parent / "adoption-index.json").read_text(encoding="utf-8"))[
            "pending"
        ]
        == {}
    )


def test_reader_waits_for_receipt_index_transaction(lease, tmp_path, monkeypatch):
    import threading

    path, record, _handle = lease
    _owned, _operational, samples = acknowledge(lease, tmp_path)
    samples[32] = sample(32, 30, 30, 320)
    published = threading.Event()
    proceed = threading.Event()
    read_started = threading.Event()
    read_done = threading.Event()
    original = suite.write_exact
    errors = []
    results = []

    def pause_receipt(target, payload, **kwargs):
        original(target, payload, **kwargs)
        if target.name.endswith(".identity.json"):
            published.set()
            assert proceed.wait(5)

    monkeypatch.setattr(suite, "write_exact", pause_receipt)

    def write():
        try:
            results.append(suite.registered_groups(path, lease=record, samples=samples))
        except BaseException as exc:
            errors.append(exc)

    def read():
        read_started.set()
        try:
            results.append(suite._custody_records(path, lease=record))
        except BaseException as exc:
            errors.append(exc)
        finally:
            read_done.set()

    writer = threading.Thread(target=write)
    reader = threading.Thread(target=read)
    writer.start()
    assert published.wait(5)
    reader.start()
    assert read_started.wait(5)
    try:
        assert not read_done.wait(0.05)
    finally:
        proceed.set()
        writer.join(5)
        reader.join(5)
    assert not writer.is_alive() and not reader.is_alive() and errors == []
    record_result = next(
        result for result in results if len(result) == 2 and isinstance(result[1], list)
    )
    assert record_result[1] == [] and record_result[0][0][1] == {
        30: 300,
        31: 310,
        32: 320,
    }


def test_transferred_groups_use_actual_command_root_projection(
    lease, tmp_path, monkeypatch
):
    path, record, _handle = lease
    command_root = tmp_path / "actual-command-root"
    owned = identity(tmp_path, path)
    operational = command_root / "owned.identity.json"
    daemon.write_backend_daemon_identity(operational, owned)
    samples = {**lease_samples(record), 30: sample(30, 1, 30, 300, owned.command)}
    observed = []

    def project(environ, *, project_root):
        observed.append(dict(environ))
        return command_root

    monkeypatch.setattr(daemon, "backend_daemon_root_from_env", project)
    environ = {
        suite.LEASE_ENV: str(path),
        "CARGO_TARGET_DIR": str(tmp_path / "command-target"),
    }
    assert (
        len(suite.transferable_groups(environ, project_root=tmp_path, samples=samples))
        == 1
    )
    assert observed == [environ]


def test_owner_lock_contention_is_cross_process_os_proof(lease):

    path, _record, _handle = lease
    code = """import sys
from pathlib import Path
from molt.file_locks import _try_acquire_file_lock,_release_file_lock
handle=_try_acquire_file_lock(Path(sys.argv[1]))
if handle is not None:
    _release_file_lock(handle)
    raise SystemExit(2)
print("contended")
"""
    result = run_custody_subject_process(
        [sys.executable, "-c", code, str(path.parent / "owner.lock")],
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert result.returncode == 0 and "contended" in result.stdout


@pytest.mark.parametrize("probe", ["_source_digest", "_try_acquire_file_lock"])
def test_live_lease_probe_errors_fail_closed(lease, monkeypatch, tmp_path, probe):
    path, record, _handle = lease

    def fail(*args, **kwargs):
        raise OSError("injected custody probe failure")

    monkeypatch.setattr(suite, probe, fail)
    assert (
        suite.live_lease(path, project_root=tmp_path, samples=lease_samples(record))
        is None
    )


def test_seal_actual_encoded_byte_cap_preserves_readable_index(lease, monkeypatch):
    path, record, _handle = lease
    index_path = path.parent / "adoption-index.json"
    original = index_path.read_bytes()
    monkeypatch.setattr(suite, "_ADOPTION_BYTES", len(original))
    errors = suite._seal_adoption_index(path, lease=record, state="closing")
    assert errors and "byte limit" in errors[0]
    assert index_path.read_bytes() == original
    assert suite._read_adoption_index(path, lease=record)["state"] == "active"


@pytest.mark.parametrize(
    "field", ["project_root", "backend_bin", "socket_path", "operational", "lease"]
)
def test_command_scratch_dependencies_cannot_receive_adoption(
    lease, tmp_path, monkeypatch, field
):
    path, record, _handle = lease
    scratch = tmp_path / "command scratch"
    scratch.mkdir()
    owned = identity(tmp_path, path)
    operational = Path(record["daemon_root"]) / "owned.identity.json"
    if field in {"project_root", "backend_bin", "socket_path"}:
        owned = replace(owned, **{field: scratch / field})
    if field == "operational":
        operational = scratch / "owned.identity.json"
    daemon.write_backend_daemon_identity(operational, owned)
    samples = {30: sample(30, 1, 30, 300, owned.command)}
    donor = {
        "MOLT_GUARD_SCRATCH_ROOT": str(path.parent if field == "lease" else scratch)
    }
    assert (
        suite.registered_groups(
            path,
            lease=record,
            samples=samples,
            daemon_root=operational.parent,
            donor_environ=donor,
        )
        == ()
    )
    assert suite._read_adoption_index(path, lease=record)["receipts"] == {}


def test_stable_suite_paths_preserve_optimized_adoption(lease, tmp_path):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "owned.identity.json", owned
    )
    samples = {30: sample(30, 1, 30, 300, owned.command)}
    assert (
        len(
            suite.registered_groups(
                path,
                lease=record,
                samples=samples,
                donor_environ={
                    "MOLT_GUARD_SCRATCH_ROOT": str(tmp_path / "unrelated scratch")
                },
            )
        )
        == 1
    )


def test_persistent_path_probe_errors_do_not_grant_transfer(tmp_path, monkeypatch):
    class FailedPath:
        def resolve(self):
            raise OSError("injected native path probe failure")

    monkeypatch.setattr(suite, "Path", lambda *_args: FailedPath())
    assert not suite.persistent_daemon_paths_allowed(
        {"MOLT_GUARD_SCRATCH_ROOT": str(tmp_path)}, [tmp_path / "dependency"]
    )


def test_failed_new_receipt_publication_can_retry_unexported_stage(
    lease, tmp_path, monkeypatch
):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "owned.identity.json", owned
    )
    samples = {30: sample(30, 1, 30, 300, owned.command)}
    original = suite.write_exact

    def fail_receipt(target, payload, **kwargs):
        if target.name.endswith(".identity.json"):
            raise OSError("injected receipt publication failure")
        return original(target, payload, **kwargs)

    with monkeypatch.context() as fault:
        fault.setattr(suite, "write_exact", fail_receipt)
        assert suite.registered_groups(path, lease=record, samples=samples) == ()
    assert len(suite.registered_groups(path, lease=record, samples=samples)) == 1
    index = suite._read_adoption_index(path, lease=record)
    assert index["pending"] == {} and len(index["receipts"]) == 1


def test_transient_closing_seal_failure_retries_within_drain_budget(lease, monkeypatch):
    path, record, _handle = lease
    original = suite._seal_adoption_index
    calls = []

    def seal(*args, **kwargs):
        calls.append(kwargs["state"])
        if len(calls) == 1:
            return ["adoption seal: OSError: injected transient seal timeout"]
        return original(*args, **kwargs)

    monkeypatch.setattr(suite, "_seal_adoption_index", seal)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: lease_samples(record))
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 0.5)
    assert suite.drain_lease(path)
    assert calls[:2] == ["closing", "closing"] and calls[-1] == "closed"


def test_read_only_scope_scan_never_grants_new_export(lease, tmp_path, monkeypatch):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "owned.identity.json", owned
    )
    samples = {30: sample(30, 1, 30, 300, owned.command)}
    assert (
        suite.registered_groups(path, lease=record, samples=samples, acknowledge=False)
        == ()
    )
    assert len(suite.registered_groups(path, lease=record, samples=samples)) == 1

    def forbidden(*args, **kwargs):
        pytest.fail("read-only sentinel attempted export acknowledgement")

    monkeypatch.setattr(suite, "_record_adoption", forbidden)
    assert (
        len(
            suite.registered_groups(
                path, lease=record, samples=samples, acknowledge=False
            )
        )
        == 1
    )


def test_read_only_scope_retains_exact_worker_birth_after_leader_reuse(lease, tmp_path):
    path, record, _handle = lease
    owned = identity(tmp_path, path)
    daemon.write_backend_daemon_identity(
        Path(record["daemon_root"]) / "owned.identity.json", owned
    )
    samples = {30: sample(30, 1, 30, 300, owned.command), 31: sample(31, 30, 30, 310)}
    assert len(suite.registered_groups(path, lease=record, samples=samples)) == 1
    samples[30] = replace(samples[30], started_at_ns=301)
    groups = suite.registered_groups(
        path, lease=record, samples=samples, acknowledge=False
    )
    assert len(groups) == 1 and set(groups[0][2]) == {31}


def test_transitive_moved_members_are_journaled_and_drained(
    lease, tmp_path, monkeypatch
):
    path, record, _handle = lease
    owned, operational, samples = acknowledge(lease, tmp_path)
    samples[31] = sample(31, 30, 31, 310)
    samples[32] = sample(32, 31, 32, 320)
    groups = suite.registered_groups(path, lease=record, samples=samples)
    assert set(groups[0][2]) == {30, 31, 32}
    receipt = json.loads(suite._adoption_path(path, owned).read_text(encoding="utf-8"))
    assert receipt["custody_members"] == {"30": 300, "31": 310, "32": 320}
    operational.unlink()
    samples.pop(30)
    samples[33] = sample(33, 32, 33, 330)
    calls = mocked_drain(monkeypatch, samples)
    assert suite.drain_lease(path)
    assert {call[0] for call in calls} == {31, 32, 33}
    assert all(not call[1]["root_owned"] for call in calls)


def test_unknown_moved_descendant_remains_unresolved_after_parent_drains(
    lease, tmp_path, monkeypatch
):
    path, _record, _handle = lease
    _owned, operational, samples = acknowledge(lease, tmp_path)
    operational.unlink()
    samples.pop(30)
    samples[31] = sample(31, 1, 31, 310)
    samples[32] = sample(32, 31, 32, None)
    monkeypatch.setattr(suite, "_DRAIN_BUDGET_S", 1.0)
    calls = mocked_drain(monkeypatch, samples)
    assert not suite.drain_lease(path)
    assert {call[0] for call in calls} == {31}
    assert 32 in samples
    result = json.loads((path.parent / "drain.json").read_text(encoding="utf-8"))
    assert result["closed"] is False


@pytest.mark.parametrize("ready", [False, True])
@pytest.mark.parametrize("accepted", [False, True])
def test_verified_busy_or_ready_reuse_requires_receiver_ack(
    lease, tmp_path, monkeypatch, ready, accepted
):
    from molt.cli import backend_execution as execution

    lease_path, _record, _handle = lease
    owned = identity(tmp_path, lease_path)
    owned.socket_path.touch()
    identity_path = tmp_path / "identity.json"
    monkeypatch.setattr(execution, "_unix_socket_path_exceeds_limit", lambda *_a: False)
    monkeypatch.setattr(
        execution, "_backend_daemon_identity_path", lambda *_a, **_k: identity_path
    )
    monkeypatch.setattr(
        execution, "_backend_daemon_log_path", lambda *_a, **_k: tmp_path / "daemon.log"
    )
    monkeypatch.setattr(
        execution, "_sweep_orphaned_backend_daemon_locks_once", lambda *_a: None
    )
    monkeypatch.setattr(execution, "_read_backend_daemon_identity", lambda *_a: owned)
    monkeypatch.setattr(
        execution, "_backend_daemon_identity_matches_context", lambda *_a, **_k: True
    )
    monkeypatch.setattr(
        execution, "_backend_daemon_identity_is_verified", lambda *_a, **_k: True
    )
    monkeypatch.setattr(
        execution, "_backend_daemon_wait_until_ready", lambda *_a, **_k: (ready, {})
    )
    observed = []

    def acknowledge(environ, **kwargs):
        observed.append((dict(environ), kwargs))
        return accepted

    monkeypatch.setattr(suite, "acknowledge_started_daemon", acknowledge)
    env = {suite.LEASE_ENV: str(lease_path)}
    assert (
        execution._start_backend_daemon(
            owned.backend_bin,
            owned.socket_path,
            cargo_profile="dev-fast",
            project_root=tmp_path,
            config_digest=owned.config_digest,
            startup_timeout=0.1,
            json_output=True,
            warnings=[],
            backend_env=env,
        )
        is accepted
    )
    assert observed == [(env, {"project_root": tmp_path, "daemon_pid": owned.pid})]

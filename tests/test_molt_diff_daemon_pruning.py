from __future__ import annotations

import dataclasses
import pytest

import importlib.util
from pathlib import Path
from types import SimpleNamespace
from tests.process_guard_common import install_module_os_view, install_module_view
import sys

from molt import backend_daemon_custody as custody
from molt.cli.backend_daemon_paths import _backend_daemon_paths
import os

# These tests fake process data the session sentinel also reads.
pytestmark = pytest.mark.usefixtures("session_sentinel_paused")


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tests" / "molt_diff.py"


def _load_diff_module():
    spec = importlib.util.spec_from_file_location(
        "molt_diff_daemon_pruning_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _daemon_process(module, *, pid, socket_path, backend_bin, rss_kb=64, age=None):
    sample = module.memory_guard.ProcessSample(
        pid=pid,
        ppid=1,
        rss_kb=rss_kb,
        command=f"{backend_bin} --daemon --socket {socket_path}",
        argv=(str(backend_bin), "--daemon", "--socket", str(socket_path)),
        elapsed_sec=age,
        started_at_ns=1,
    )
    return module._BackendDaemonProcess(sample=sample, socket_path=socket_path)


def test_molt_diff_daemon_observation_retains_native_resource_sample(
    tmp_path, monkeypatch
):
    module = _load_diff_module()
    expected = _daemon_process(
        module,
        pid=4321,
        socket_path=tmp_path / "daemon.sock",
        backend_bin=tmp_path / "molt-backend",
        rss_kb=2048,
        age=123,
    )
    monkeypatch.setattr(
        module.daemon_custody,
        "backend_daemon_process_observations",
        lambda: [(expected.sample, expected.socket_path)],
    )
    observed = module._list_backend_daemon_processes()
    assert observed == [expected]
    assert observed[0].sample is expected.sample


def _identity(
    tmp_path: Path,
    *,
    pid: int,
    socket_path: Path,
) -> custody.BackendDaemonIdentity:
    backend_bin = tmp_path / "target" / "debug" / "molt-backend"
    return custody.BackendDaemonIdentity(
        pid=pid,
        socket_path=socket_path,
        project_root=tmp_path,
        cargo_profile="dev-fast",
        config_digest=None,
        backend_bin=backend_bin,
        created_at=1_700_000_000.0,
        command=f"{backend_bin} --daemon --socket {socket_path}",
    )


def _write_session_identity(
    daemon_root: Path,
    identity: custody.BackendDaemonIdentity,
    *,
    session_id: str = "alpha-session",
) -> Path:
    # Name the sidecar with the launcher's own path authority, so the scan
    # below is tested against the names real daemons are written under.
    _socket, _log, identity_path = _backend_daemon_paths(
        project_root_str=str(identity.project_root),
        cargo_profile=identity.cargo_profile,
        config_digest="deadbeef",
        explicit_socket="",
        socket_dir_override=str(identity.socket_path.parent),
        build_state_root_str=str(daemon_root.parent),
        tempdir_str=str(daemon_root.parent),
        session_id=session_id,
    )
    assert identity_path.parent == daemon_root
    custody.write_backend_daemon_identity(identity_path, identity)
    return identity_path


@pytest.mark.parametrize(
    "recorded_birth,observed_birth,expected",
    [(1, 1, True), (1, 2, False), (None, 1, False), (1, None, False)],
)
def test_pruning_record_requires_the_observed_process_generation(
    tmp_path, recorded_birth, observed_birth, expected
):
    module = _load_diff_module()
    socket = tmp_path / "daemon.sock"
    identity = dataclasses.replace(
        _identity(tmp_path, pid=4321, socket_path=socket), started_at_ns=recorded_birth
    )
    record = custody.BackendDaemonIdentityRecord(identity, tmp_path / "identity.json")
    process = _daemon_process(
        module, pid=4321, socket_path=socket, backend_bin=identity.backend_bin
    )
    process = dataclasses.replace(
        process,
        sample=dataclasses.replace(process.sample, started_at_ns=observed_birth),
    )
    admitted = module._verified_backend_daemon_record(process, {identity.pid: record})
    assert admitted is record if expected else admitted is None


@pytest.mark.parametrize(
    ("first", "second"),
    [
        # Agent lanes named `agent-<task>-<pid>` that differ only after
        # character 32 once used one daemon label.
        (
            "agent-unit-agent-440e87baa421421f9c0d6f2e-67518",
            "agent-unit-agent-440e87baa421421f9c0d6f2e-67519",
        ),
        # The launcher kept `.` and `:` and the scan replaced them, so the
        # scan never found the lane's own daemon.
        ("lane.alpha:7", "lane.alpha:8"),
    ],
)
def test_session_daemon_scan_finds_only_its_own_session(
    tmp_path: Path, first: str, second: str
) -> None:
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    paths = {}
    for pid, session_id in ((4321, first), (4322, second)):
        identity = dataclasses.replace(
            _identity(tmp_path, pid=pid, socket_path=tmp_path / f"{pid}.sock"),
            started_at_ns=1_700_000_000_000_000_000 + pid,
        )
        paths[session_id] = _write_session_identity(
            daemon_root, identity, session_id=session_id
        )

    for session_id, path in paths.items():
        records = custody.iter_backend_daemon_identity_records(
            daemon_root, session_id=session_id
        )
        assert [record.path for record in records] == [path]


def test_molt_diff_backend_daemon_scan_failure_fails_closed(monkeypatch) -> None:
    module = _load_diff_module()

    # Observation reads the guard's native process table, never ps text. A
    # failed snapshot observes no daemon, so pruning signals nothing.
    class SnapshotError(Exception):
        pass

    def unavailable():
        raise SnapshotError("process table unavailable")

    guard = SimpleNamespace(
        sample_processes=unavailable, ProcessSnapshotError=SnapshotError
    )
    monkeypatch.setattr(
        module.daemon_custody, "_load_memory_guard_module", lambda: guard
    )

    assert module._list_backend_daemon_processes() == []


def test_molt_diff_missing_socket_daemon_without_identity_is_not_killed(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "missing.sock"
    backend_bin = tmp_path / "target" / "debug" / "molt-backend"
    process = _daemon_process(
        module,
        pid=4321,
        socket_path=socket_path,
        backend_bin=backend_bin,
    )
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "0")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: True)
    install_module_os_view(monkeypatch, module, name="posix")

    def fake_terminate(identity, **kwargs) -> bool:
        terminated.append(identity.pid)
        return True

    monkeypatch.setattr(
        module.daemon_custody,
        "terminate_backend_daemon_identity",
        fake_terminate,
    )

    module._prune_backend_daemons()

    assert terminated == []


def test_molt_diff_orphan_worker_pruning_uses_verified_pid_custody(
    monkeypatch,
) -> None:
    module = _load_diff_module()
    sample = module.memory_guard.ProcessSample(
        pid=4321,
        ppid=1,
        pgid=4321,
        rss_kb=64,
        command="python3 -c 'from multiprocessing.spawn import spawn_main'",
        elapsed_sec=5,
        started_at_ns=111,
    )
    process = module._PrunableProcess(
        pid=sample.pid,
        identity=module.memory_guard.process_identity(sample),
        command=sample.command,
        reason="orphan_diff_worker",
    )
    calls: list[tuple[int, module.memory_guard.ProcessIdentity, float]] = []

    monkeypatch.setattr(
        module.memory_guard,
        "sample_processes",
        lambda: {sample.pid: sample},
    )

    def fake_terminate_verified_pid(pid, identity, *, sampler, grace):
        assert sampler is module.memory_guard.sample_processes
        calls.append((pid, identity, grace))
        return (
            module.memory_guard.GuardTerminationAction(
                "process",
                pid,
                module.memory_guard.signal.SIGTERM,
                "SIGTERM",
                "completed_or_missing",
            ),
        )

    monkeypatch.setattr(
        module.memory_guard,
        "terminate_verified_pid",
        fake_terminate_verified_pid,
    )

    module._prune_verified_processes(
        [process],
        label="orphan multiprocessing worker",
        grace=0.75,
    )

    assert calls == [(sample.pid, module.memory_guard.process_identity(sample), 0.75)]


def test_molt_diff_build_helper_pruning_preserves_codex_protected_group(
    monkeypatch,
) -> None:
    module = _load_diff_module()
    codex = module.memory_guard.ProcessSample(
        pid=100,
        ppid=1,
        pgid=100,
        rss_kb=64,
        command="codex app-server",
        elapsed_sec=120,
        started_at_ns=100,
    )
    shell = module.memory_guard.ProcessSample(
        pid=200,
        ppid=100,
        pgid=200,
        rss_kb=64,
        command="bash -lc pytest",
        elapsed_sec=100,
        started_at_ns=200,
    )
    helper = module.memory_guard.ProcessSample(
        pid=300,
        ppid=200,
        pgid=300,
        rss_kb=64,
        command="/repo/target/dev-fast/molt-backend --output /tmp/molt_diff_x/out.o",
        elapsed_sec=3600,
        started_at_ns=300,
    )
    samples = {sample.pid: sample for sample in (codex, shell, helper)}
    sent: list[tuple[int, int]] = []
    custody_events: list[tuple[int, list[str]]] = []
    process = module._PrunableProcess(
        pid=helper.pid,
        identity=module.memory_guard.process_identity(helper),
        command=helper.command,
        reason="stale_diff_build_helper",
    )

    install_module_view(monkeypatch, "os", os, module.memory_guard, getpid=lambda: 999)
    monkeypatch.setattr(module.memory_guard, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        module.memory_guard.os, "kill", lambda pid, sig: sent.append((pid, sig))
    )

    def capture_event(process, actions):
        custody_events.append((process.pid, [action.result for action in actions]))

    monkeypatch.setattr(module, "_record_prune_custody_event", capture_event)

    module._prune_verified_processes(
        [process],
        label="orphan build helper process",
        grace=0.35,
    )

    assert sent == []
    assert custody_events == [(helper.pid, ["skipped_host_control_lineage"])]


def test_molt_diff_never_stops_a_verified_daemon_whose_socket_is_gone(
    tmp_path: Path,
    monkeypatch,
) -> None:
    # A daemon unlinks its socket only when it retires, on request or when
    # idle; it exits once its accepted connections finish.
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "owned-missing.sock"
    identity = _identity(tmp_path, pid=4321, socket_path=socket_path)
    identity_path = _write_session_identity(daemon_root, identity)
    record = custody.BackendDaemonIdentityRecord(identity=identity, path=identity_path)
    process = _daemon_process(
        module,
        pid=identity.pid,
        socket_path=socket_path,
        backend_bin=identity.backend_bin,
    )
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "0")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: pid == identity.pid)
    install_module_os_view(monkeypatch, module, name="posix")
    monkeypatch.setattr(
        module,
        "_verified_backend_daemon_record",
        lambda _process, _records_by_pid: record,
    )

    def fake_terminate(recorded_identity, **kwargs) -> bool:
        terminated.append(recorded_identity.pid)
        return True

    monkeypatch.setattr(
        module.daemon_custody,
        "terminate_backend_daemon_identity",
        fake_terminate,
    )

    module._prune_backend_daemons()

    assert terminated == []


@pytest.mark.parametrize("successor_verified", [True, False])
def test_molt_diff_never_stops_an_older_daemon_beside_its_successor(
    tmp_path: Path,
    monkeypatch,
    successor_verified: bool,
) -> None:
    # The older daemon retired and its successor bound the same path.
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "shared.sock"
    socket_path.write_text("", encoding="utf-8")
    identities = [
        _identity(tmp_path, pid=pid, socket_path=socket_path) for pid in (4321, 4322)
    ]
    records = {
        identity.pid: custody.BackendDaemonIdentityRecord(
            identity=identity, path=daemon_root / f"{identity.pid}.identity.json"
        )
        for identity in identities
    }
    processes = [
        _daemon_process(
            module,
            pid=identity.pid,
            socket_path=socket_path,
            backend_bin=identity.backend_bin,
            age=7200,
        )
        for identity in identities
    ]
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "1")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: processes)
    monkeypatch.setattr(module, "_pid_alive", lambda pid: pid in records)
    monkeypatch.setattr(module, "_daemon_health", lambda _socket: None)
    install_module_os_view(monkeypatch, module, name="posix")
    monkeypatch.setattr(
        module,
        "_verified_backend_daemon_record",
        lambda process, _records_by_pid: (
            records[process.sample.pid]
            if successor_verified or process.sample.pid == identities[0].pid
            else None
        ),
    )

    def fake_terminate(recorded_identity, **kwargs) -> bool:
        terminated.append(recorded_identity.pid)
        return True

    monkeypatch.setattr(
        module.daemon_custody, "terminate_backend_daemon_identity", fake_terminate
    )

    module._prune_backend_daemons()

    assert terminated == []


def test_molt_diff_rss_threshold_skips_unverified_daemon(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "owned.sock"
    socket_path.write_text("", encoding="utf-8")
    backend_bin = tmp_path / "target" / "debug" / "molt-backend"
    process = _daemon_process(
        module,
        pid=4321,
        socket_path=socket_path,
        backend_bin=backend_bin,
    )
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "1")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: True)
    install_module_os_view(monkeypatch, module, name="posix")
    monkeypatch.setattr(
        module,
        "_daemon_health",
        lambda *args: (_ for _ in ()).throw(
            AssertionError("unverified daemon RSS must not authorize a control probe")
        ),
    )

    def fake_terminate(identity, **kwargs) -> bool:
        terminated.append(identity.pid)
        return True

    monkeypatch.setattr(
        module.daemon_custody,
        "terminate_backend_daemon_identity",
        fake_terminate,
    )

    module._prune_backend_daemons()

    assert terminated == []


def _oversized_daemon(tmp_path: Path, monkeypatch):
    """One verified session daemon over the RSS threshold, with fake custody."""
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "owned.sock"
    identity = dataclasses.replace(
        _identity(tmp_path, pid=4321, socket_path=socket_path),
        started_at_ns=1,
    )
    identity_path = _write_session_identity(daemon_root, identity)
    record = custody.BackendDaemonIdentityRecord(identity=identity, path=identity_path)
    process = _daemon_process(
        module,
        pid=identity.pid,
        socket_path=socket_path,
        backend_bin=identity.backend_bin,
        rss_kb=2_000_000,
        age=60,
    )
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "1000")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: pid == identity.pid)

    def no_subprocess(*args, **kwargs):
        raise AssertionError("daemon resource pruning must reuse its native sample")

    install_module_view(
        monkeypatch, "subprocess", module.subprocess, module, run=no_subprocess
    )
    install_module_os_view(monkeypatch, module, name="posix")
    monkeypatch.setattr(
        module,
        "_verified_backend_daemon_record",
        lambda _process, _records_by_pid: record,
    )

    def no_kill(recorded_identity, **kwargs) -> bool:
        raise AssertionError(f"daemon {recorded_identity.pid} must not be killed")

    monkeypatch.setattr(
        module.daemon_custody, "terminate_backend_daemon_identity", no_kill
    )
    return module, socket_path, identity_path


@pytest.mark.parametrize("age", [None, 30, 1000])
def test_molt_diff_silence_and_observed_age_never_authorize_signals(
    tmp_path, monkeypatch, age
):
    module, socket_path, _identity_path = _oversized_daemon(tmp_path, monkeypatch)
    socket_path.write_text("", encoding="utf-8")
    process = module._list_backend_daemon_processes()[0]
    process = dataclasses.replace(
        process, sample=dataclasses.replace(process.sample, elapsed_sec=age)
    )
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_daemon_health", lambda _socket: None)
    terminated = []

    def terminate(identity, **kwargs):
        terminated.append(identity.pid)
        return True

    monkeypatch.setattr(
        module.daemon_custody, "terminate_backend_daemon_identity", terminate
    )
    module._prune_backend_daemons()
    assert terminated == []


def test_molt_diff_retires_an_idle_oversized_daemon_instead_of_killing_it(
    tmp_path: Path, monkeypatch
) -> None:
    from molt.cli import backend_execution

    module, socket_path, identity_path = _oversized_daemon(tmp_path, monkeypatch)
    socket_path.write_text("", encoding="utf-8")
    retired: list[Path] = []
    monkeypatch.setattr(
        module, "_daemon_health", lambda path: {"active_jobs": 0, "pid": 4321}
    )

    def retire(path: Path, *, timeout: float | None) -> bool:
        retired.append(path)
        return True

    monkeypatch.setattr(backend_execution, "retire_backend_daemon", retire)

    module._prune_backend_daemons()

    assert retired == [socket_path]
    assert custody.read_backend_daemon_identity(identity_path) is not None


@pytest.mark.parametrize(
    "health",
    [
        {"active_jobs": 2, "pid": 4321},  # a build is compiling
        # The socket already belongs to a successor daemon.
        {"active_jobs": 0, "pid": 9999},
        {"cache_entries": 3},  # a daemon that does not report activity
        None,  # no answer
    ],
)
def test_molt_diff_leaves_a_busy_or_silent_oversized_daemon_running(
    tmp_path: Path, monkeypatch, health
) -> None:
    from molt.cli import backend_execution

    module, socket_path, identity_path = _oversized_daemon(tmp_path, monkeypatch)
    socket_path.write_text("", encoding="utf-8")
    monkeypatch.setattr(module, "_daemon_health", lambda path: health)
    monkeypatch.setattr(
        backend_execution,
        "retire_backend_daemon",
        lambda path, *, timeout: pytest.fail("a busy daemon must not be retired"),
    )

    module._prune_backend_daemons()

    assert custody.read_backend_daemon_identity(identity_path) is not None


def test_molt_diff_never_stops_a_draining_daemon(tmp_path: Path, monkeypatch) -> None:
    # Retirement unlinked the socket; the daemon exits once drained.
    module, socket_path, _identity_path = _oversized_daemon(tmp_path, monkeypatch)
    assert not socket_path.exists()

    module._prune_backend_daemons()

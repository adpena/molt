from __future__ import annotations

import importlib.util
from pathlib import Path
import sys

from molt import backend_daemon_custody as custody
from molt.dx import session_artifact_component


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
    session_label = session_artifact_component(session_id)
    identity_path = (
        daemon_root / f"molt-backend.dev-fast.{session_label}.deadbeef.identity.json"
    )
    custody.write_backend_daemon_identity(identity_path, identity)
    return identity_path


def test_molt_diff_backend_daemon_scan_failure_fails_closed(monkeypatch) -> None:
    module = _load_diff_module()

    def raise_timeout(*args, **kwargs):
        raise module.subprocess.TimeoutExpired(cmd=["ps"], timeout=2.0)

    monkeypatch.setattr(module.subprocess, "run", raise_timeout)

    assert module._list_backend_daemon_processes() == []


def test_molt_diff_legacy_pid_cleanup_unlinks_without_signaling(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    legacy_pid = daemon_root / "molt-backend.dev-fast.alpha.legacy.pid"
    legacy_pid.write_text("4321\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [])
    monkeypatch.setattr(module.os, "name", "posix", raising=False)
    monkeypatch.setattr(
        module.daemon_custody,
        "terminate_backend_daemon_identity",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("legacy pid cleanup must not terminate identities")
        ),
    )

    module._prune_backend_daemons()

    assert not legacy_pid.exists()


def test_molt_diff_missing_socket_daemon_without_identity_is_not_killed(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "missing.sock"
    backend_bin = tmp_path / "target" / "debug" / "molt-backend"
    process = module._BackendDaemonProcess(
        pid=4321,
        socket_path=socket_path,
        command=f"{backend_bin} --daemon --socket {socket_path}",
    )
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "0")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(module.os, "name", "posix", raising=False)

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

    monkeypatch.setattr(module.memory_guard.os, "getpid", lambda: 999)
    monkeypatch.setattr(module.memory_guard, "_safe_getpgrp", lambda: None)
    monkeypatch.setattr(module.memory_guard, "sample_processes", lambda: samples)
    monkeypatch.setattr(
        module.memory_guard.os,
        "kill",
        lambda pid, sig: sent.append((pid, sig)),
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
    assert custody_events == [(helper.pid, ["skipped_protected_group_member"])]


def test_molt_diff_verified_missing_socket_daemon_terminates_through_custody(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "owned-missing.sock"
    identity = _identity(tmp_path, pid=4321, socket_path=socket_path)
    identity_path = _write_session_identity(daemon_root, identity)
    record = custody.BackendDaemonIdentityRecord(identity=identity, path=identity_path)
    process = module._BackendDaemonProcess(
        pid=identity.pid,
        socket_path=socket_path,
        command=f"{identity.backend_bin} --daemon --socket {socket_path}",
    )
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "0")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: pid == identity.pid)
    monkeypatch.setattr(module.os, "name", "posix", raising=False)
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

    assert terminated == [identity.pid]


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
    process = module._BackendDaemonProcess(
        pid=4321,
        socket_path=socket_path,
        command=f"{backend_bin} --daemon --socket {socket_path}",
    )
    terminated: list[int] = []
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha-session")
    monkeypatch.setenv("MOLT_DIFF_DAEMON_MAX_RSS_KB", "1")
    monkeypatch.setattr(module, "_diff_backend_daemon_root", lambda: daemon_root)
    monkeypatch.setattr(module, "_list_backend_daemon_processes", lambda: [process])
    monkeypatch.setattr(module, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(module.os, "name", "posix", raising=False)
    monkeypatch.setattr(
        module,
        "_pid_rss_age",
        lambda pid: (_ for _ in ()).throw(
            AssertionError("unverified daemon RSS must not authorize inspection/kill")
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

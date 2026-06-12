from __future__ import annotations

import importlib.util
from pathlib import Path
import sys

from molt import backend_daemon_custody as custody


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
    session_label = custody.backend_daemon_session_artifact_component(session_id)
    identity_path = (
        daemon_root / f"molt-backend.dev-fast.{session_label}.deadbeef.identity.json"
    )
    custody.write_backend_daemon_identity(identity_path, identity)
    return identity_path


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
    monkeypatch.setattr(
        module,
        "_kill_pid",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("legacy pid cleanup must not signal")
        ),
    )
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
    monkeypatch.setattr(
        module,
        "_kill_pid",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unverified daemon must not use raw kill")
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


def test_molt_diff_verified_missing_socket_daemon_terminates_through_custody(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _load_diff_module()
    daemon_root = tmp_path / "target" / ".molt_state" / "backend_daemon"
    daemon_root.mkdir(parents=True)
    socket_path = tmp_path / "owned-missing.sock"
    identity = _identity(tmp_path, pid=4321, socket_path=socket_path)
    _write_session_identity(daemon_root, identity)
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
    monkeypatch.setattr(
        module,
        "_kill_pid",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("verified daemon must terminate through custody")
        ),
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
    monkeypatch.setattr(
        module,
        "_pid_rss_age",
        lambda pid: (_ for _ in ()).throw(
            AssertionError("unverified daemon RSS must not authorize inspection/kill")
        ),
    )
    monkeypatch.setattr(
        module,
        "_kill_pid",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("unverified daemon must not use raw kill")
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

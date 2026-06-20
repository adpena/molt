from __future__ import annotations

from pathlib import Path
import signal

from molt import backend_daemon_custody as custody


def _identity(tmp_path: Path, *, pid: int = 101) -> custody.BackendDaemonIdentity:
    backend_bin = tmp_path / "target" / "debug" / "molt-backend"
    socket_path = tmp_path / "daemon.sock"
    return custody.BackendDaemonIdentity(
        pid=pid,
        socket_path=socket_path,
        project_root=tmp_path,
        cargo_profile="dev-fast",
        config_digest="abc123",
        backend_bin=backend_bin,
        created_at=1_700_000_000.0,
        command=f"{backend_bin} --daemon --socket {socket_path}",
    )


def test_backend_daemon_identity_roundtrip_and_rejects_malformed(
    tmp_path: Path,
) -> None:
    identity = _identity(tmp_path)
    identity_path = tmp_path / "state" / "molt-backend.identity.json"

    custody.write_backend_daemon_identity(identity_path, identity)

    assert custody.read_backend_daemon_identity(identity_path) == identity

    identity_path.write_text('{"schema":"wrong","pid":101}\n', encoding="utf-8")

    assert custody.read_backend_daemon_identity(identity_path) is None


def test_backend_daemon_command_match_requires_daemon_backend_and_socket(
    tmp_path: Path,
) -> None:
    identity = _identity(tmp_path)
    command = f"{identity.backend_bin} --daemon --socket {identity.socket_path}"

    assert custody.backend_daemon_command_matches_identity(
        command,
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    )
    assert not custody.backend_daemon_command_matches_identity(
        f"{identity.backend_bin} --socket {identity.socket_path}",
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    )
    assert not custody.backend_daemon_command_matches_identity(
        f"{identity.backend_bin} --daemon --socket {tmp_path / 'other.sock'}",
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    )
    assert not custody.backend_daemon_command_matches_identity(
        f"/bin/sleep 999 --daemon --socket {identity.socket_path}",
        backend_bin=identity.backend_bin,
        socket_path=identity.socket_path,
    )


def test_backend_daemon_identity_rejects_live_foreign_pid(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == identity.pid)
    monkeypatch.setattr(custody, "_process_command", lambda pid: "/bin/sleep 999")

    assert not custody.backend_daemon_identity_is_verified(
        identity,
        allow_health_probe=False,
    )


def test_backend_daemon_identity_health_probe_can_verify_matching_pid(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == identity.pid)
    monkeypatch.setattr(custody, "_process_command", lambda pid: "/bin/sleep 999")

    assert custody.backend_daemon_identity_is_verified(
        identity,
        allow_health_probe=True,
        health_probe=lambda path, timeout: (True, {"pid": identity.pid}),
    )
    assert not custody.backend_daemon_identity_is_verified(
        identity,
        allow_health_probe=True,
        health_probe=lambda path, timeout: (True, {"pid": identity.pid + 1}),
    )


def test_backend_daemon_identity_from_health_requires_spawn_generation(
    tmp_path: Path,
) -> None:
    identity = _identity(tmp_path)

    assert (
        custody.backend_daemon_identity_from_health(
            {"pid": identity.pid, "spawn_config_digest": "other"},
            socket_path=identity.socket_path,
            project_root=identity.project_root,
            cargo_profile=identity.cargo_profile,
            config_digest=identity.config_digest,
            backend_bin=identity.backend_bin,
        )
        is None
    )

    adopted = custody.backend_daemon_identity_from_health(
        {"pid": identity.pid, "spawn_config_digest": identity.config_digest},
        socket_path=identity.socket_path,
        project_root=identity.project_root,
        cargo_profile=identity.cargo_profile,
        config_digest=identity.config_digest,
        backend_bin=identity.backend_bin,
    )

    assert adopted is not None
    assert adopted.pid == identity.pid


def test_backend_daemon_termination_never_signals_unverified_identity(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    signals: list[int] = []
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(custody, "_process_command", lambda pid: "/bin/sleep 999")
    monkeypatch.setattr(
        custody.os,
        "kill",
        lambda pid, sig: signals.append(sig),
    )

    assert not custody.terminate_backend_daemon_identity(identity)
    assert signals == []


def test_backend_daemon_termination_revalidates_before_sigkill(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    command = f"{identity.backend_bin} --daemon --socket {identity.socket_path}"
    commands = [command, "/bin/sleep 999"]
    signals: list[int] = []
    ticks = iter([0.0, 0.0, 0.1])

    monkeypatch.setattr(custody, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(
        custody,
        "_process_command",
        lambda pid: commands.pop(0) if commands else "/bin/sleep 999",
    )
    monkeypatch.setattr(custody.time, "monotonic", lambda: next(ticks))
    monkeypatch.setattr(custody.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(
        custody.os,
        "kill",
        lambda pid, sig: signals.append(sig),
    )

    assert custody.terminate_backend_daemon_identity(identity, grace=0.01)
    assert signals == [signal.SIGTERM]


def test_backend_daemon_termination_escalates_only_after_verified_grace(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    command = f"{identity.backend_bin} --daemon --socket {identity.socket_path}"
    signals: list[int] = []
    ticks = iter([0.0, 0.0, 0.1])

    monkeypatch.setattr(custody, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(custody, "_process_command", lambda pid: command)
    monkeypatch.setattr(custody.time, "monotonic", lambda: next(ticks))
    monkeypatch.setattr(custody.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(
        custody.os,
        "kill",
        lambda pid, sig: signals.append(sig),
    )

    assert custody.terminate_backend_daemon_identity(identity, grace=0.01)
    assert signals == [signal.SIGTERM, signal.SIGKILL]


def test_backend_daemon_legacy_pid_cleanup_unlinks_without_signaling(
    tmp_path: Path,
    monkeypatch,
) -> None:
    pid_file = tmp_path / "molt-backend.pid"
    sidecar = tmp_path / "molt-backend.identity.json"
    pid_file.write_text("101\n", encoding="utf-8")
    sidecar.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(
        custody.os,
        "kill",
        lambda pid, sig: (_ for _ in ()).throw(AssertionError("unexpected signal")),
    )

    assert custody.remove_legacy_pid_files(tmp_path) == 1
    assert not pid_file.exists()
    assert sidecar.exists()


def test_current_session_backend_daemon_records_filter_by_session(
    tmp_path: Path,
) -> None:
    target = tmp_path / "target"
    daemon_root = target / ".molt_state" / "backend_daemon"
    alpha = _identity(tmp_path, pid=101)
    beta = _identity(tmp_path, pid=202)
    alpha_path = daemon_root / "molt-backend.dev.alpha.one.identity.json"
    beta_path = daemon_root / "molt-backend.dev.beta.one.identity.json"
    custody.write_backend_daemon_identity(alpha_path, alpha)
    custody.write_backend_daemon_identity(beta_path, beta)
    env = {"MOLT_SESSION_ID": "alpha", "CARGO_TARGET_DIR": str(target)}

    records = custody.current_session_backend_daemon_identity_records(
        env,
        project_root=tmp_path,
    )

    assert [record.identity.pid for record in records] == [101]
    assert records[0].path == alpha_path


def test_terminate_backend_daemons_for_session_removes_only_terminated_records(
    tmp_path: Path,
    monkeypatch,
) -> None:
    target = tmp_path / "target"
    daemon_root = target / ".molt_state" / "backend_daemon"
    first = _identity(tmp_path, pid=101)
    second = _identity(tmp_path, pid=202)
    first_path = daemon_root / "molt-backend.dev.alpha.first.identity.json"
    second_path = daemon_root / "molt-backend.dev.alpha.second.identity.json"
    custody.write_backend_daemon_identity(first_path, first)
    custody.write_backend_daemon_identity(second_path, second)
    attempted: list[int] = []

    def fake_terminate(identity, **kwargs):
        del kwargs
        attempted.append(identity.pid)
        return identity.pid == first.pid

    monkeypatch.setattr(custody, "terminate_backend_daemon_identity", fake_terminate)

    terminated = custody.terminate_backend_daemons_for_session(
        {"MOLT_SESSION_ID": "alpha", "CARGO_TARGET_DIR": str(target)},
        project_root=tmp_path,
    )

    assert attempted == [101, 202]
    assert [record.identity.pid for record in terminated] == [101]
    assert not first_path.exists()
    assert second_path.exists()

from __future__ import annotations

from pathlib import Path
import signal

from molt import backend_daemon_custody as custody
from tools import memory_guard


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


def _backend_daemon_force_kill_signal() -> int:
    return getattr(signal, "SIGKILL", signal.SIGTERM)


def _daemon_sample(
    identity: custody.BackendDaemonIdentity,
    *,
    command: str | None = None,
    started_at_ns: int | None = 111,
) -> memory_guard.ProcessSample:
    return memory_guard.ProcessSample(
        pid=identity.pid,
        ppid=1,
        pgid=identity.pid,
        rss_kb=1,
        command=command
        if command is not None
        else f"{identity.backend_bin} --daemon --socket {identity.socket_path}",
        started_at_ns=started_at_ns,
    )


def _patch_memory_guard_termination(
    monkeypatch,
    *,
    samples,
    signals: list[int],
) -> None:
    monkeypatch.setattr(custody, "_load_memory_guard_module", lambda: memory_guard)
    monkeypatch.setattr(memory_guard, "sample_processes", samples)
    monkeypatch.setattr(memory_guard.os, "getpid", lambda: 999_999)
    monkeypatch.setattr(memory_guard.os, "getpgrp", lambda: 999_999, raising=False)
    monkeypatch.setattr(memory_guard.time, "sleep", lambda seconds: None)
    monkeypatch.setattr(
        memory_guard,
        "_pid_exited_or_unobservable",
        lambda pid, *, grace: False,
    )
    monkeypatch.setattr(
        memory_guard.os,
        "kill",
        lambda pid, sig: None if sig == 0 else signals.append(sig),
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
    spaced_backend_bin = tmp_path / "target with space" / "debug" / "molt-backend"
    spaced_socket_path = tmp_path / "socket dir" / "daemon.sock"
    quoted_command = (
        f'"{spaced_backend_bin}" --daemon --socket "{spaced_socket_path}"'
    )
    assert custody.backend_daemon_command_matches_identity(
        quoted_command,
        backend_bin=spaced_backend_bin,
        socket_path=spaced_socket_path,
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


def test_backend_daemon_identity_health_probe_cannot_substitute_for_process_identity(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == identity.pid)
    monkeypatch.setattr(custody, "_process_command", lambda pid: "/bin/sleep 999")

    assert not custody.backend_daemon_identity_is_verified(
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
    _patch_memory_guard_termination(
        monkeypatch,
        samples=lambda: {
            identity.pid: _daemon_sample(identity, command="/bin/sleep 999"),
        },
        signals=signals,
    )

    assert not custody.terminate_backend_daemon_identity(identity)
    assert signals == []


def test_backend_daemon_termination_revalidates_before_sigkill(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    signals: list[int] = []
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: True)
    original = _daemon_sample(identity, started_at_ns=111)
    reused = _daemon_sample(identity, command="/bin/sleep 999", started_at_ns=222)
    samples = iter(
        [
            {identity.pid: original},
            {identity.pid: original},
            {identity.pid: original},
            {identity.pid: reused},
        ]
    )
    _patch_memory_guard_termination(
        monkeypatch,
        samples=lambda: next(samples),
        signals=signals,
    )

    assert custody.terminate_backend_daemon_identity(identity, grace=0.01)
    assert signals == [signal.SIGTERM]


def test_backend_daemon_termination_keeps_identity_after_protective_fallback_skip(
    tmp_path: Path,
    monkeypatch,
) -> None:
    target = tmp_path / "target"
    daemon_root = target / ".molt_state" / "backend_daemon"
    identity = _identity(tmp_path)
    identity_path = daemon_root / "molt-backend.dev.alpha.protected.identity.json"
    custody.write_backend_daemon_identity(identity_path, identity)
    sample = _daemon_sample(identity)
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: pid == identity.pid)
    monkeypatch.setattr(custody, "_load_memory_guard_module", lambda: memory_guard)
    monkeypatch.setattr(memory_guard, "sample_processes", lambda: {identity.pid: sample})
    monkeypatch.setattr(memory_guard, "is_host_control_plane_process", lambda _sample: False)
    monkeypatch.setattr(
        memory_guard,
        "terminate_verified_pid",
        lambda pid, identity_value, *, sampler, grace: (
            memory_guard.GuardTerminationAction(
                "process",
                pid,
                signal.SIGTERM,
                "SIGTERM",
                "still_live",
            ),
            memory_guard.GuardTerminationAction(
                "process",
                pid,
                _backend_daemon_force_kill_signal(),
                memory_guard._signal_name(_backend_daemon_force_kill_signal()),
                "skipped_host_control_lineage",
            ),
        ),
    )

    terminated = custody.terminate_backend_daemons_for_session(
        {"MOLT_SESSION_ID": "alpha", "CARGO_TARGET_DIR": str(target)},
        project_root=tmp_path,
    )

    assert terminated == ()
    assert identity_path.exists()


def test_backend_daemon_termination_escalates_only_after_verified_grace(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    signals: list[int] = []
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: True)
    _patch_memory_guard_termination(
        monkeypatch,
        samples=lambda: {identity.pid: _daemon_sample(identity)},
        signals=signals,
    )

    assert custody.terminate_backend_daemon_identity(identity, grace=0.01)
    assert signals == [signal.SIGTERM, _backend_daemon_force_kill_signal()]


def test_backend_daemon_termination_escalates_with_sigterm_without_sigkill(
    tmp_path: Path,
    monkeypatch,
) -> None:
    identity = _identity(tmp_path)
    signals: list[int] = []
    monkeypatch.setattr(custody, "_pid_alive", lambda pid: True)
    monkeypatch.setattr(
        memory_guard,
        "fallback_kill_signal",
        lambda: signal.SIGTERM,
    )
    _patch_memory_guard_termination(
        monkeypatch,
        samples=lambda: {identity.pid: _daemon_sample(identity)},
        signals=signals,
    )

    assert custody.terminate_backend_daemon_identity(identity, grace=0.01)
    assert signals == [signal.SIGTERM, signal.SIGTERM]


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

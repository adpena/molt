from __future__ import annotations

import ctypes
from dataclasses import replace
import importlib.util
from pathlib import Path
import shlex
import sys
from types import SimpleNamespace

import pytest

from molt import backend_daemon_custody as daemon
from tools.memory_guard_core import process_model as model


@pytest.mark.parametrize(
    "argv",
    [
        (
            "/opt/Molt Builds/molt-backend",
            "--daemon",
            "--socket",
            "/tmp/socket path.sock",
        ),
        (
            "/opt/quoted'path/molt-backend",
            "--daemon",
            "--socket",
            '/tmp/quote"path.sock',
            "",
        ),
    ],
)
def test_linux_sampling_preserves_kernel_argv_boundaries(tmp_path, argv):
    proc = tmp_path / "12"
    proc.mkdir()
    (proc / "cmdline").write_bytes(b"\0".join(arg.encode() for arg in argv) + b"\0")
    samples = model.sample_processes_linux_proc(
        tmp_path,
        stat_reader=lambda *_: (1, 12, 1_000_000_000, "molt-backend"),
        uptime_sec=2.0,
    )
    assert samples[12].argv == argv
    assert tuple(shlex.split(samples[12].command)) == argv


@pytest.mark.parametrize("raw", [b"", b"molt-backend", b"\0--daemon\0"])
def test_linux_unknown_or_malformed_argv_is_unavailable(tmp_path, raw):
    proc = tmp_path / "12"
    proc.mkdir()
    (proc / "cmdline").write_bytes(raw)
    assert model._linux_proc_argv(12, tmp_path) is None


def test_authoritative_argv_probe_rejects_changed_birth(monkeypatch):
    monkeypatch.setattr(model.sys, "platform", "linux")
    births = iter([(1, 12, 100, "molt-backend"), (1, 12, 101, "molt-backend")])
    monkeypatch.setattr(model, "_linux_proc_stat_identity", lambda *_: next(births))
    monkeypatch.setattr(
        model, "_linux_proc_argv", lambda *_: ("molt-backend", "--daemon")
    )
    assert model.process_command_argv(12) is None


def test_darwin_kernel_argv_preserves_spaces_quotes_and_empty_arguments():
    argv = (
        "/opt/Molt Builds/molt-backend",
        "--daemon",
        "--socket",
        '/tmp/quoted" path.sock',
        "",
    )
    raw = (
        len(argv).to_bytes(4, sys.byteorder, signed=True)
        + b"/actual/executable\0\0"
        + b"\0".join(arg.encode() for arg in argv)
        + b"\0ENV=value\0"
    )

    def sysctl(_mib, _length, output, size, _new, _newsize):
        size._obj.value = len(raw)
        if output is not None:
            ctypes.memmove(output, raw, len(raw))
        return 0

    def unused(*_args: object) -> int:
        raise AssertionError("argv decoding must not enumerate or size processes")

    authority = model._DarwinProcessAuthority(
        ctypes=ctypes,
        libproc=None,
        libsystem=None,
        proc_bsd_info_type=object,
        proc_task_info_type=object,
        proc_pidinfo=lambda *_args: 0,
        proc_listallpids=unused,
        sysctl=sysctl,
    )
    assert authority.argv(12) == argv
    assert tuple(shlex.split(authority.command(12))) == argv


def test_typed_daemon_identity_matches_whitespace_and_rejects_wrong_socket(tmp_path):
    binary = tmp_path / "build with spaces" / "molt-backend"
    socket = tmp_path / 'socket with "quotes".sock'
    argv = (str(binary), "--daemon", "--socket", str(socket))
    assert daemon.backend_daemon_command_matches_identity(
        argv, backend_bin=binary, socket_path=socket
    )
    assert not daemon.backend_daemon_command_matches_identity(
        argv, backend_bin=binary, socket_path=tmp_path / "socket"
    )
    assert not daemon.backend_daemon_command_matches_identity(
        (*argv, "bad\0arg"), backend_bin=binary, socket_path=socket
    )


def test_posix_malformed_quote_is_not_guessed_as_signal_authority(monkeypatch):
    monkeypatch.setattr(daemon, "os", SimpleNamespace(name="posix"))
    assert daemon._split_command('molt-backend --daemon --socket "unterminated') == ()


def test_daemon_observation_uses_only_birth_bound_typed_vectors(tmp_path, monkeypatch):
    argv = (
        str(tmp_path / "molt-backend"),
        "--daemon",
        "--socket",
        str(tmp_path / "socket with spaces"),
    )
    valid = model.ProcessSample(12, 1, 0, "misleading diagnostic", 12, 1, 100, argv)
    samples = {
        12: valid,
        13: replace(valid, pid=13, started_at_ns=None),
        14: replace(valid, pid=14, argv=None),
    }
    monkeypatch.setattr(daemon, "os", SimpleNamespace(name="posix"))
    monkeypatch.setattr(
        daemon,
        "_load_memory_guard_module",
        lambda: SimpleNamespace(
            sample_processes=lambda: samples,
            ProcessSnapshotError=model.ProcessSnapshotError,
        ),
    )
    assert daemon.backend_daemon_process_observations() == ((valid, Path(argv[-1])),)


@pytest.mark.parametrize(
    "relative", ["tools/bench_individual.py", "tests/molt_diff.py"]
)
def test_lateral_daemon_producers_preserve_typed_socket_path(
    tmp_path, monkeypatch, relative
):
    root = Path(__file__).resolve().parents[1]
    name = "molt_argv_" + Path(relative).stem
    spec = importlib.util.spec_from_file_location(name, root / relative)
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, name, module)
    spec.loader.exec_module(module)
    socket = tmp_path / "socket with spaces.sock"
    argv = (str(tmp_path / "molt-backend"), "--daemon", "--socket", str(socket))
    sample = model.ProcessSample(12, 1, 0, "diagnostic", 12, 1, 100, argv)
    monkeypatch.setattr(
        daemon, "backend_daemon_process_observations", lambda: ((sample, socket),)
    )
    observed = module._list_backend_daemon_processes()
    assert len(observed) == 1 and observed[0].socket_path == socket
    assert observed[0].argv == argv and observed[0].command == "diagnostic"


@pytest.mark.parametrize("births", [(100, 100), (100, 101)])
def test_identity_factory_binds_actual_spawn_env_and_single_birth(
    tmp_path, monkeypatch, births
):
    observed = iter(births)
    monkeypatch.setattr(daemon, "process_started_at_ns", lambda *_: next(observed))
    monkeypatch.setattr(daemon, "backend_content_sha256", lambda *_: "a" * 64)
    monkeypatch.setenv("MOLT_BACKEND_DAEMON_SUITE_LEASE", "unrelated-global-lease")
    argv = (
        str(tmp_path / "molt-backend"),
        "--daemon",
        "--socket",
        str(tmp_path / "socket with spaces"),
    )
    identity = daemon.backend_daemon_identity_for_pid(
        12,
        socket_path=Path(argv[-1]),
        project_root=tmp_path,
        cargo_profile="dev",
        config_digest="b" * 64,
        backend_bin=Path(argv[0]),
        process_command=lambda *_: argv,
        environ={"MOLT_BACKEND_DAEMON_SUITE_LEASE": "actual-spawn-lease"},
    )
    assert identity.suite_lease == "actual-spawn-lease"
    if births[0] == births[1]:
        assert identity.started_at_ns == 100
        assert identity.command is not None
    else:
        assert identity.started_at_ns is None and identity.command is None


@pytest.mark.parametrize(
    "unsafe", ["project", "binary", "socket", "identity", "log", "lease"]
)
def test_daemon_spawn_and_reuse_reject_command_scratch_dependencies(
    tmp_path, monkeypatch, unsafe
):
    from molt.cli import backend_execution as execution

    scratch = tmp_path / "command scratch"
    project = scratch / "project" if unsafe == "project" else tmp_path / "project"
    binary = (
        scratch / "molt-backend" if unsafe == "binary" else tmp_path / "molt-backend"
    )
    socket = scratch / "daemon.sock" if unsafe == "socket" else tmp_path / "daemon.sock"
    identity = (
        scratch / "identity.json"
        if unsafe == "identity"
        else tmp_path / "identity.json"
    )
    log = scratch / "daemon.log" if unsafe == "log" else tmp_path / "daemon.log"
    env = {"MOLT_GUARD_SCRATCH_ROOT": str(scratch)}
    if unsafe == "lease":
        env["MOLT_BACKEND_DAEMON_SUITE_LEASE"] = str(scratch / "lease.json")
    monkeypatch.setattr(execution, "_unix_socket_path_exceeds_limit", lambda *_: False)
    monkeypatch.setattr(
        execution, "_backend_daemon_identity_path", lambda *_a, **_k: identity
    )
    monkeypatch.setattr(execution, "_backend_daemon_log_path", lambda *_a, **_k: log)
    monkeypatch.setattr(
        execution,
        "_sweep_orphaned_backend_daemon_locks_once",
        lambda *_: pytest.fail("unsafe daemon reached reuse or spawn"),
    )
    warnings = []
    assert not execution._start_backend_daemon(
        binary,
        socket,
        cargo_profile="dev",
        project_root=project,
        config_digest="a" * 64,
        startup_timeout=1.0,
        json_output=True,
        warnings=warnings,
        backend_env=env,
    )
    assert warnings and "command scratch" in warnings[0]


def test_posix_daemon_birth_probe_uses_single_pid_authority(monkeypatch):
    monkeypatch.setattr(daemon, "os", SimpleNamespace(name="posix"))
    calls = []
    monkeypatch.setattr(
        model, "process_started_at_ns", lambda pid: calls.append(pid) or 123
    )
    monkeypatch.setattr(
        daemon,
        "_load_memory_guard_module",
        lambda: pytest.fail("birth admission scanned all processes"),
    )
    assert daemon.process_started_at_ns(12) == 123 and calls == [12]

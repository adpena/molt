from __future__ import annotations
import copy
import json
import os
import shutil
from tests.process_guard_common import install_module_view, run_custody_subject_process

import subprocess
from pathlib import Path

import pytest

from tools import cross_run


def test_guarded_run_translates_timeout_return_code(monkeypatch) -> None:
    def fake_guarded_completed_process(cmd, **kwargs):
        return subprocess.CompletedProcess(
            cmd,
            cross_run.harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE,
            stdout="partial",
            stderr="timeout",
        )

    monkeypatch.setattr(
        cross_run.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    with pytest.raises(subprocess.TimeoutExpired) as exc_info:
        cross_run._guarded_run(["sleep", "10"], timeout=1)

    assert exc_info.value.cmd == ["sleep", "10"]
    assert exc_info.value.output == "partial"
    assert exc_info.value.stderr == "timeout"


def test_local_expected_uses_memory_guard(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        cross_run.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    assert (
        cross_run._local_expected(cross_run.Case("sample", "print('ok')\n")) == "ok\n"
    )
    assert captured["cmd"] == [cross_run.sys.executable, "-c", "print('ok')\n"]
    assert captured["kwargs"]["prefix"] == "MOLT_CROSS"
    assert captured["kwargs"]["timeout"] == 15


def test_local_compile_uses_memory_guard(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}
    monkeypatch.setattr(cross_run, "_HOST_TRIPLE", "aarch64-apple-darwin")

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        out_dir = Path(cmd[cmd.index("--out-dir") + 1])
        binary = out_dir / "sample_molt"
        binary.write_text("#!/bin/sh\n", encoding="utf-8")
        return subprocess.CompletedProcess(cmd, 0, stdout="build ok", stderr="")

    monkeypatch.setattr(
        cross_run.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    binary, log = cross_run._local_compile(
        cross_run.Case("sample", "print('ok')\n"),
        "aarch64-apple-darwin",
        tmp_path,
        verbose=False,
    )

    assert binary is not None
    assert binary.name == "sample_molt"
    assert "exit=0" in log
    assert captured["cmd"][:3] == [
        cross_run._python_for_build(),
        "-m",
        "molt.cli",
    ]
    assert captured["kwargs"]["prefix"] == "MOLT_CROSS"
    assert captured["kwargs"]["cwd"] == str(cross_run.REPO)
    assert captured["kwargs"]["timeout"] == 600


def test_ssh_transport_uses_memory_guard(monkeypatch, tmp_path) -> None:
    calls: list[list[str]] = []
    host = cross_run.Host(
        name="host",
        target="aarch64-unknown-linux-gnu",
        transport="ssh",
        hostname="example.test",
        user="molt",
    )
    transport = cross_run.SSHTransport(host)

    def fake_guarded_completed_process(cmd, **kwargs):
        calls.append(cmd)
        assert kwargs["prefix"] == "MOLT_CROSS"
        return subprocess.CompletedProcess(cmd, 0, stdout="remote\n", stderr="")

    monkeypatch.setattr(
        cross_run.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    transport.prepare()
    binary = tmp_path / "binary"
    binary.write_bytes(b"payload")
    transport.push([binary])
    code, stdout, stderr = transport.run([transport.remote_path("binary")], timeout=7)

    assert code == 0
    assert stdout == "remote\n"
    assert stderr == ""
    assert calls[0][0] == "ssh"
    assert calls[1][0] == "scp"
    assert calls[1][-1] == f"molt@example.test:{transport.execution_dir}/"
    assert calls[2][0] == "ssh"
    assert transport.remote_path("binary") in calls[2][-1]


class _DockerDaemon:
    """Stateful independent daemon: killing a client does NOT stop its container."""

    identifier = "b" * 64
    sibling = "c" * 64

    def __init__(self, phase="normal"):
        self.phase = phase
        self.calls = []
        self.containers = {self.sibling: {"sentinel": "untouched"}}
        self.inspect_count = 0
        self.delayed = None
        self.bind_source = None

    def execute(self, command, **kwargs):
        self.calls.append(command)
        assert kwargs["timeout"] > 0
        if command[:2] == ["image", "inspect"]:
            return subprocess.CompletedProcess(command, 0, "[]", "")
        assert command[0] == "container"
        operation = command[1]
        if operation == "create":
            assert "--pull=never" in command
            assert "--rm" not in command
            if self.phase == "create-before":
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            self.name = command[command.index("--name") + 1]
            key, nonce = command[command.index("--label") + 1].split("=", 1)
            self.containers[self.identifier] = {
                "Id": self.identifier,
                "Name": "/" + self.name,
                "Config": {"Labels": {key: nonce}},
                "State": {
                    "Running": False,
                    "Pid": 0,
                    "ExitCode": 0,
                    "OOMKilled": False,
                },
            }
            if self.phase == "create-delayed":
                self.delayed = self.containers.pop(self.identifier)
                self.bind_source = Path(
                    command[command.index("-v") + 1].rsplit(":", 1)[0]
                )
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            if self.phase == "create-after":
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            output = "bad ack" if self.phase == "create-ack" else self.identifier
        elif operation == "ls":
            assert command[command.index("--filter") + 1].startswith("name=molt-run-")
            assert command[-1].startswith("label=org.molt.release-custody=")
            # A foreign container exists throughout; the exact filter excludes it.
            output = self.identifier if self.identifier in self.containers else ""
        elif operation == "inspect":
            assert command[2] == self.identifier
            self.inspect_count += 1
            if self.phase == "inspect-before" and self.inspect_count == 1:
                raise RuntimeError("inspect failed")
            value = copy.deepcopy(self.containers[self.identifier])
            if self.phase == "owner" and self.inspect_count > 1:
                value["Config"]["Labels"]["org.molt.release-custody"] = "foreign"
            output = json.dumps([value])
        elif operation == "start":
            assert command[2:] == ["--attach", self.identifier]
            assert kwargs["timeout"] == 9
            state = self.containers[self.identifier]["State"]
            state.update(Running=True, Pid=123)
            if self.phase in {"start-timeout", "kill", "pid", "remove"}:
                raise subprocess.TimeoutExpired(command, kwargs["timeout"])
            if self.phase == "cancel":
                raise KeyboardInterrupt("cancelled")
            state.update(Running=False, Pid=0)
            return subprocess.CompletedProcess(
                command, 7 if self.phase == "exit" else 0, "ok\n", ""
            )
        elif operation == "kill":
            assert command[2] == self.identifier
            if self.phase == "kill":
                raise RuntimeError("kill failed")
            self.containers[self.identifier]["State"].update(
                Running=False, Pid=99 if self.phase == "pid" else 0
            )
            output = self.identifier
        elif operation == "rm":
            assert command[2] == self.identifier
            state = self.containers[self.identifier]["State"]
            assert state["Running"] is False and state["Pid"] == 0
            if self.phase == "remove":
                raise RuntimeError("remove failed")
            del self.containers[self.identifier]
            output = self.identifier
        else:
            raise AssertionError(command)
        return subprocess.CompletedProcess(command, 0, output, "")

    def guarded(self, command, **kwargs):
        assert command[0] == "docker"
        assert kwargs["prefix"] == "MOLT_CROSS"
        return self.execute(command[1:], **kwargs)


def _docker_transport(monkeypatch, tmp_path, phase):
    daemon = _DockerDaemon(phase)
    monkeypatch.setattr(
        cross_run.harness_memory_guard, "guarded_completed_process", daemon.guarded
    )
    transport = cross_run.DockerTransport(
        cross_run.Host(
            name="docker",
            target="x86_64-unknown-linux-gnu",
            transport="docker",
            container="example@sha256:" + "a" * 64,
            remote_dir="/work",
        )
    )
    stage = tmp_path / "owned-stage"
    stage.mkdir()
    (stage / "payload").write_bytes(b"binary")
    (tmp_path / "sibling").write_bytes(b"sentinel")
    transport._stage = stage
    return transport, daemon, stage


@pytest.mark.parametrize(
    "phase",
    [
        "normal",
        "exit",
        "create-before",
        "create-delayed",
        "create-after",
        "create-ack",
        "inspect-before",
        "start-timeout",
        "cancel",
        "owner",
        "kill",
        "pid",
        "remove",
    ],
)
def test_docker_client_failure_cannot_retire_a_live_bind_source(
    monkeypatch, tmp_path, phase
):
    transport, daemon, stage = _docker_transport(monkeypatch, tmp_path, phase)
    if phase in {"normal", "exit"}:
        code, stdout, stderr = transport.run(
            [transport.remote_path("payload")], timeout=9
        )
        assert (code, stdout, stderr) == (7 if phase == "exit" else 0, "ok\n", "")
    else:
        expected = {
            "create-before": ExceptionGroup,
            "create-delayed": ExceptionGroup,
            "create-after": subprocess.TimeoutExpired,
            "create-ack": ValueError,
            "inspect-before": RuntimeError,
            "start-timeout": subprocess.TimeoutExpired,
            "cancel": KeyboardInterrupt,
            "owner": ValueError,
            "kill": ExceptionGroup,
            "pid": ExceptionGroup,
            "remove": ExceptionGroup,
        }[phase]
        with pytest.raises(expected) as error:
            transport.run([transport.remote_path("payload")], timeout=9)
        if phase in {"create-before", "create-delayed", "kill", "pid", "remove"}:
            assert isinstance(error.value.exceptions[0], subprocess.TimeoutExpired)
            assert len(error.value.exceptions) == 2
        if phase in {"create-before", "create-delayed"}:
            assert "creation outcome unknown" in str(error.value.exceptions[1])
    if phase in {"create-before", "create-delayed", "owner", "kill", "pid", "remove"}:
        with pytest.raises(RuntimeError, match="retirement unproved"):
            transport.cleanup()
        assert (stage / "payload").read_bytes() == b"binary"
        if phase == "create-delayed":
            # The daemon finishes AFTER the empty recovery query. A no-match
            # was not terminal evidence: the late mount must retain its source.
            assert daemon.identifier not in daemon.containers
            assert daemon.bind_source == stage and daemon.bind_source.exists()
            daemon.containers[daemon.identifier] = daemon.delayed
        assert (daemon.identifier in daemon.containers) is (phase != "create-before")
        calls = list(daemon.calls)
        with pytest.raises(RuntimeError, match="Earlier Docker"):
            transport.run(["/work/payload"], timeout=9)
        assert daemon.calls == calls
    else:
        assert daemon.identifier not in daemon.containers
        transport.cleanup()
        assert not stage.exists()
    assert daemon.containers[daemon.sibling] == {"sentinel": "untouched"}
    assert (tmp_path / "sibling").read_bytes() == b"sentinel"
    starts = [c for c in daemon.calls if c[:2] == ["container", "start"]]
    if starts:
        assert starts == [["container", "start", "--attach", daemon.identifier]]


def test_sealed_configuration_failure_retires_only_owned_systemd_unit(
    monkeypatch, tmp_path
):
    from tests.tools.test_release_sealed_replay import (
        _systemd_observation,
        _typed_observation,
    )

    nonce, root, command, interfaces = _systemd_observation()
    interfaces["Service"]["PrivateNetwork"] = False
    raw = _typed_observation(interfaces)
    transport = cross_run.SealedRootTransport()
    transport._root = root
    transport._files = []
    transport._execution = cross_run.SEALED_REPLAY_EXECUTION
    transport._provider_identity = {}
    transport._evidence = tmp_path
    monkeypatch.setattr(cross_run, "_sealed_owned_root", lambda root: [])
    monkeypatch.setattr(transport, "_provider", lambda: {})
    monkeypatch.setattr(cross_run.secrets, "token_hex", lambda size: nonce)
    monkeypatch.setattr(
        transport, "_show", lambda unit: (raw, cross_run.parse_systemd_observation(raw))
    )
    calls = []
    stopped = False

    def control(args, **kwargs):
        nonlocal stopped
        calls.append(args)
        if args[1:3] == ["stop", "--"]:
            stopped = True
        return subprocess.CompletedProcess(
            args, 0, "not-found\n" if stopped else "loaded\n", ""
        )

    # stop stdout is empty, while its following LoadState observation is absent.
    def dispatch(args, **kwargs):
        result = control(args, **kwargs)
        if args[1:3] == ["stop", "--"]:
            result.stdout = ""
        return result

    monkeypatch.setattr(transport, "_control", dispatch)

    def guarded(args, **kwargs):
        kwargs["stdout_capture_path"].write_bytes(b"")
        kwargs["stderr_capture_path"].write_bytes(b"")
        return subprocess.CompletedProcess(args, 0, "", "")

    monkeypatch.setattr(cross_run, "_guarded_run", guarded)
    with pytest.raises(ValueError, match="PrivateNetwork"):
        transport.run_sealed(command, timeout=9)
    assert [args for args in calls if args[1:3] == ["stop", "--"]] == [
        ["/usr/bin/systemctl", "stop", "--", f"molt-release-{nonce}.service"]
    ]
    assert transport._owned == {} and transport._unresolved == set()


@pytest.mark.parametrize(
    "reference", ["example:latest", "example:stable", "sha256:short"]
)
def test_docker_prepare_rejects_mutable_or_malformed_image_without_pull(
    monkeypatch, reference
):
    install_module_view(
        monkeypatch, "shutil", shutil, cross_run, which=lambda name: "/usr/bin/docker"
    )
    calls = []
    monkeypatch.setattr(cross_run, "_guarded_run", lambda *a, **k: calls.append(a))
    transport = cross_run.DockerTransport(
        cross_run.Host(
            name="test",
            target="x86_64-unknown-linux-gnu",
            transport="docker",
            container=reference,
        )
    )
    with pytest.raises(ValueError, match="immutable image digest"):
        transport.prepare()
    assert calls == []
    assert transport._stage is None and transport._container_pending is False
    transport.cleanup()
    assert calls == []


def test_docker_prepare_missing_pin_never_pulls(monkeypatch):
    install_module_view(
        monkeypatch, "shutil", shutil, cross_run, which=lambda name: "/usr/bin/docker"
    )
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 1, "", "absent")

    monkeypatch.setattr(cross_run, "_guarded_run", run)
    reference = "example@sha256:" + "a" * 64
    transport = cross_run.DockerTransport(
        cross_run.Host(
            name="test",
            target="x86_64-unknown-linux-gnu",
            transport="docker",
            container=reference,
        )
    )
    with pytest.raises(RuntimeError, match="no automatic pull"):
        transport.prepare()
    assert calls == [["docker", "image", "inspect", reference]]


def test_external_raw_capture_uses_a_positive_bounded_diagnostic_tail(
    monkeypatch, tmp_path
):
    captured = {}

    def run(command, **kwargs):
        captured.update(kwargs)
        return subprocess.CompletedProcess(command, 0, "tail", "tail")

    monkeypatch.setattr(
        cross_run.harness_memory_guard, "guarded_completed_process", run
    )
    cross_run._guarded_run(
        ["/docker", "start"],
        stdout_capture_path=tmp_path / "stdout",
        stderr_capture_path=tmp_path / "stderr",
        capture_tail_bytes=4096,
    )
    assert captured["capture_tail_bytes"] == 4096
    assert captured["stdout_capture_path"] == tmp_path / "stdout"


@pytest.mark.parametrize(
    "phase", ["normal", "prepare", "copy", "push", "timeout", "cancel"]
)
@pytest.mark.parametrize("keep_remote", [False, True])
def test_host_staging_lifetime_covers_every_setup_exit(
    monkeypatch, tmp_path, phase, keep_remote
):
    host = cross_run.Host("fake", "target", "ssh")
    stage = tmp_path / "remote-child"
    sibling = tmp_path / "sibling"
    sibling.write_bytes(b"sentinel")
    binary = tmp_path / "build-output"
    binary.write_bytes(b"original")
    pushed = []
    events = []

    class Transport:
        def prepare(self):
            stage.mkdir()
            (stage / "owner").write_bytes(b"owned")
            if phase == "prepare":
                raise RuntimeError("prepare broke after allocation")

        def push(self, files):
            pushed.extend(files)
            for path in files:
                assert path.parent != binary.parent
                assert path.name == "sample"
                shutil.copy2(path, stage / path.name)
            if phase == "push":
                raise RuntimeError("push broke")

        def remote_path(self, name):
            return str(stage / name)

        def run(self, argv, *, timeout):
            events.append("run")
            if phase == "timeout":
                raise subprocess.TimeoutExpired(argv, timeout)
            if phase == "cancel":
                raise KeyboardInterrupt("cancelled")
            return 0, "ok\n", ""

        def cleanup(self):
            events.append("cleanup")
            shutil.rmtree(stage)

    monkeypatch.setattr(cross_run, "transport_for", lambda _: Transport())
    if phase == "copy":

        def fail_copy(source, destination):
            Path(destination).write_bytes(b"partial")
            pushed.append(Path(destination))
            raise OSError("copy broke")

        install_module_view(monkeypatch, "shutil", shutil, cross_run, copy2=fail_copy)
    args = (
        host,
        [cross_run.Case("sample", "")],
        {("target", "sample"): (binary, "")},
        {"sample": "ok"},
    )
    if phase == "cancel":
        with pytest.raises(KeyboardInterrupt, match="cancelled"):
            cross_run._run_one_host(*args, keep_remote=keep_remote, verbose=False)
    else:
        result = cross_run._run_one_host(*args, keep_remote=keep_remote, verbose=False)
        assert stage.exists() is keep_remote
        assert result.cleanup_error is None
        assert bool(result.setup_error) == (phase in {"prepare", "copy", "push"})
        assert result.cases[0].status == (
            "SKIP" if result.setup_error else "FAIL" if phase == "timeout" else "PASS"
        )
    assert stage.exists() is keep_remote
    assert events.count("cleanup") == (0 if keep_remote else 1)
    assert all(not path.exists() and not path.parent.exists() for path in pushed)
    assert binary.read_bytes() == b"original"
    assert not (binary.parent / "sample").exists()
    assert sibling.read_bytes() == b"sentinel"


@pytest.mark.parametrize("phase", ["setup", "run", "cancel"])
def test_primary_and_cleanup_failures_remain_distinct(monkeypatch, tmp_path, phase):
    class Transport:
        def prepare(self):
            if phase == "setup":
                raise RuntimeError("primary setup")

        def push(self, files):
            pass

        def remote_path(self, name):
            return name

        def run(self, argv, *, timeout):
            if phase == "cancel":
                raise KeyboardInterrupt("primary cancellation")
            raise RuntimeError("primary run")

        def cleanup(self):
            raise OSError("cleanup failure")

    monkeypatch.setattr(cross_run, "transport_for", lambda _: Transport())
    # No file copies: a missing local build still exercises prepare and cleanup.
    cases = [] if phase == "setup" else [cross_run.Case("sample", "")]
    if phase == "setup":
        result = cross_run._run_one_host(
            cross_run.Host("fake", "target", "ssh"),
            cases,
            {},
            {},
            keep_remote=False,
            verbose=False,
        )
        assert "primary setup" in result.setup_error
        assert result.cleanup_error == "cleanup failure"
    else:
        binary = tmp_path / "binary"
        binary.write_bytes(b"executable")
        with pytest.raises(BaseExceptionGroup) as error:
            cross_run._run_one_host(
                cross_run.Host("fake", "target", "ssh"),
                cases,
                {("target", "sample"): (binary, "")},
                {"sample": "ok"},
                keep_remote=False,
                verbose=False,
            )
        assert len(error.value.exceptions) == 2
        assert str(error.value.exceptions[0]).startswith("primary")
        assert str(error.value.exceptions[1]) == "cleanup failure"


@pytest.mark.skipif(os.name == "nt", reason="POSIX SSH shell command contract")
@pytest.mark.parametrize("prepare_failure", [False, True])
def test_ssh_nonce_child_cleanup_preserves_parent_and_siblings(
    monkeypatch, tmp_path, prepare_failure
):
    parent = tmp_path / "parent with 'quotes'"
    parent.mkdir()
    sentinel = parent / "sibling"
    sentinel.write_bytes(b"sentinel")
    host = cross_run.Host("local-shell", "target", "ssh", remote_dir=str(parent))
    transport = cross_run.SSHTransport(host)
    commands = []

    def local_shell(command, **kwargs):
        commands.append(command)
        # Execute the actual generated POSIX shell at the raw behavior boundary;
        # no SSH daemon, provider or external host is contacted.
        proc = run_custody_subject_process(
            ["/bin/sh", "-c", command[-1]], timeout=10, capture_output=True, text=True
        )
        if prepare_failure and len(commands) == 1:
            raise subprocess.TimeoutExpired(command, 60)
        return proc

    monkeypatch.setattr(cross_run, "_guarded_run", local_shell)
    if prepare_failure:
        with pytest.raises(subprocess.TimeoutExpired):
            transport.prepare()
    else:
        transport.prepare()
    child = Path(transport.execution_dir)
    assert child.parent == parent and child != parent
    assert (child / ".molt-owner").read_text(encoding="utf-8") == transport._nonce
    (child / "payload").write_bytes(b"owned")
    transport.cleanup()
    assert not child.exists()
    assert list(parent.iterdir()) == [sentinel]
    assert sentinel.read_bytes() == b"sentinel"


@pytest.mark.parametrize("shell", ["sh", "powershell"])
def test_ssh_cleanup_refuses_foreign_owner_and_reports_remote_failure(
    monkeypatch, shell
):
    host = cross_run.Host(
        "host", "target", "ssh", remote_dir="/staging'parent", shell=shell
    )
    transport = cross_run.SSHTransport(host)
    commands = []

    def fail(command, **kwargs):
        commands.append(command)
        return subprocess.CompletedProcess(command, 9, "", "ownership differs")

    monkeypatch.setattr(cross_run, "_guarded_run", fail)
    with pytest.raises(RuntimeError, match="cleanup failed.*exit=9.*ownership differs"):
        transport.cleanup()
    text = commands[0][-1]
    assert ".molt-owner" in text and transport._nonce in text
    assert "molt-cross-" in text
    assert "retained" in text
    if shell == "powershell":
        assert text.count("[IO.FileAttributes]::ReparsePoint") == 2
        quoted = "'" + transport.execution_dir.replace("'", "''") + "'"
        assert f"Get-Item -Force -LiteralPath {quoted}" in text
        quoted_marker = (
            "'" + transport.remote_path(".molt-owner").replace("'", "''") + "'"
        )
        assert f"Get-Item -Force -LiteralPath {quoted_marker}" in text
        assert (
            text.index("ReparsePoint")
            < text.index("ReadAllText")
            < text.index("Remove-Item")
        )
    else:
        assert f"[ ! -L {cross_run.shlex.quote(transport.execution_dir)} ]" in text
        assert (
            f"[ ! -L {cross_run.shlex.quote(transport.remote_path('.molt-owner'))} ]"
            in text
        )


@pytest.mark.skipif(os.name == "nt", reason="POSIX SSH shell command contract")
@pytest.mark.parametrize("foreign", ["marker", "symlink"])
def test_ssh_changed_ownership_retains_child_and_never_follows_links(
    monkeypatch, tmp_path, foreign
):
    parent = tmp_path / "staging"
    parent.mkdir()
    sibling = parent / "sibling"
    sibling.mkdir()
    sentinel = sibling / "sentinel"
    sentinel.write_bytes(b"foreign")
    transport = cross_run.SSHTransport(
        cross_run.Host("local", "target", "ssh", remote_dir=str(parent))
    )

    def local_shell(command, **kwargs):
        return run_custody_subject_process(
            ["/bin/sh", "-c", command[-1]], timeout=10, capture_output=True, text=True
        )

    monkeypatch.setattr(cross_run, "_guarded_run", local_shell)
    transport.prepare()
    child = Path(transport.execution_dir)
    marker = child / ".molt-owner"
    if foreign == "marker":
        marker.write_text("foreign", encoding="utf-8")
    else:
        marker.unlink()
        child.rmdir()
        child.symlink_to(sibling, target_is_directory=True)
    with pytest.raises(RuntimeError, match="ownership differs; retained"):
        transport.cleanup()
    assert child.exists()
    assert sentinel.read_bytes() == b"foreign"


def test_cleanup_diagnostic_reaches_matrix_report_and_exit_status(
    monkeypatch, tmp_path, capsys
):
    host = cross_run.Host("test", "target", "ssh")
    result = cross_run.HostResult(
        host=host,
        cases=[cross_run.CaseResult("sample", "PASS", 0)],
        cleanup_error="owned stage retained: denied",
    )
    monkeypatch.setattr(cross_run, "SMOKE_CASES", [cross_run.Case("sample", "")])
    monkeypatch.setattr(cross_run, "_local_expected", lambda case: "")
    monkeypatch.setattr(cross_run, "parse_inventory", lambda path: [host])
    monkeypatch.setattr(cross_run, "_local_compile", lambda *a, **k: (None, "unused"))
    monkeypatch.setattr(cross_run, "_run_one_host", lambda *a, **k: result)
    # The main build workspace is an existing separate owner, not this fixture's
    # subject; contain its allocation without mutating the process-wide module.
    work = tmp_path / "build-work"
    work.mkdir()
    install_module_view(
        monkeypatch,
        "tempfile",
        cross_run.tempfile,
        cross_run,
        mkdtemp=lambda **kw: str(work),
    )
    assert cross_run.main(["--report", "--report-dir", str(tmp_path / "reports")]) == 1
    assert "[cleanup error] owned stage retained: denied" in capsys.readouterr().out
    reports = list((tmp_path / "reports").glob("*.md"))
    assert len(reports) == 1
    text = reports[0].read_text(encoding="utf-8")
    assert "test — cleanup error" in text and "owned stage retained: denied" in text


def test_sealed_unacknowledged_create_keeps_unknown_custody(monkeypatch, tmp_path):
    transport = cross_run.SealedRootTransport()
    transport._root_unit = "molt-release-import-" + "b" * 32 + ".service"
    transport._unresolved.add("molt-release-" + "a" * 32 + ".service")
    calls = []
    monkeypatch.setattr(transport, "_stop", lambda unit: calls.append(unit))
    sentinel = tmp_path / "retained-root"
    sentinel.write_bytes(b"sealed source")
    with pytest.raises(RuntimeError, match="creation outcome unknown"):
        transport.remove_sealed_root()
    assert calls == [] and sentinel.read_bytes() == b"sealed source"


@pytest.mark.parametrize("operation", ["claim", "stop"])
@pytest.mark.parametrize("index", [0, 1])
def test_sealed_provider_rejects_boolean_invocation_before_control(
    monkeypatch, operation, index
):
    from tests.tools.test_release_sealed_replay import (
        _systemd_observation,
        _typed_observation,
    )

    _, _, _, interfaces = _systemd_observation()
    unit = interfaces["Unit"]["Id"]
    invocation = [0, 1] * 8
    interfaces["Unit"]["InvocationID"] = invocation.copy()
    interfaces["Unit"]["InvocationID"][index] = bool(invocation[index])
    raw = _typed_observation(interfaces)
    transport = cross_run.SealedRootTransport()
    transport._owned[unit] = invocation
    monkeypatch.setattr(
        transport, "_show", lambda _: (raw, cross_run.parse_systemd_observation(raw))
    )
    calls = []
    monkeypatch.setattr(transport, "_control", lambda *a, **kw: calls.append(a))
    with pytest.raises(ValueError):
        if operation == "claim":
            transport._claim(unit, interfaces["Unit"]["Description"])
        else:
            transport._stop(unit)
    assert calls == [] and transport._owned[unit] == invocation


def _compilation_execution_record():
    # Independent wire oracle: deliberately distinct from replay's budgets.
    return {
        "schema": "molt.sealed-execution.v1",
        "mode": "compilation",
        "memory_max_bytes": 2147483648,
        "workspace_max_bytes": 536870912,
        "runtime_max_seconds": 300,
    }


def _compilation_systemd_observation():
    from tests.tools.test_release_sealed_replay import _systemd_observation

    nonce, root, command, interfaces = _systemd_observation()
    service = interfaces["Service"]
    service.update(
        WorkingDirectory="/work",
        ReadWritePaths=["/work"],
        NoExecPaths=["/evidence", "/work"],
        MemoryMax=2147483648,
        RuntimeMaxUSec=300000000,
        LimitFSIZE=536870912,
        LimitFSIZESoft=536870912,
    )
    service["TemporaryFileSystem"] = [
        row for row in service["TemporaryFileSystem"] if row[0] != "/tmp"
    ]
    service["TemporaryFileSystem"].append(
        ["/work", "rw,noexec,nosuid,nodev,size=536870912,mode=1777"]
    )
    return nonce, root, command, interfaces


def test_sealed_execution_projection_has_one_strict_wire_contract():
    record = _compilation_execution_record()
    execution = cross_run.SealedExecutionProjection.compilation(
        memory_max_bytes=2147483648,
        workspace_max_bytes=536870912,
        runtime_max_seconds=300,
    )
    assert execution.to_dict() == record
    assert cross_run.SealedExecutionProjection.from_dict(record) == execution
    assert cross_run.SEALED_REPLAY_EXECUTION.to_dict() == {
        "schema": "molt.sealed-execution.v1",
        "mode": "replay",
        "memory_max_bytes": 1073741824,
        "workspace_max_bytes": 67108864,
        "runtime_max_seconds": 60,
    }


@pytest.mark.parametrize(
    "field", ["memory_max_bytes", "workspace_max_bytes", "runtime_max_seconds"]
)
@pytest.mark.parametrize("value", [True, False, 0, -1, 1.0, "300", None, 2**63])
def test_sealed_execution_projection_rejects_invalid_budget(field, value):
    record = _compilation_execution_record()
    record[field] = value
    with pytest.raises(ValueError, match="bounded positive integer"):
        cross_run.SealedExecutionProjection.from_dict(record)


@pytest.mark.parametrize(
    "change",
    [
        lambda row: row.update(schema="molt.sealed-execution.v0"),
        lambda row: row.update(mode="compile"),
        lambda row: row.update(mode=True),
        lambda row: row.update(exec_workspace=True),
        lambda row: row.pop("workspace_max_bytes"),
        lambda row: row.update(mode="replay"),
    ],
)
def test_sealed_execution_projection_rejects_unknown_or_incomplete_policy(change):
    record = _compilation_execution_record()
    change(record)
    with pytest.raises(ValueError):
        cross_run.SealedExecutionProjection.from_dict(record)


def test_sealed_compilation_and_replay_use_independent_effective_policy_oracles():
    from tests.tools.test_release_sealed_replay import (
        _systemd_observation,
        _typed_observation,
    )

    nonce, root, command, replay = _systemd_observation()
    cross_run.validate_sealed_systemd_configuration(
        _typed_observation(replay), nonce=nonce, root=root, command=command
    )
    nonce, root, command, compilation = _compilation_systemd_observation()
    projection = cross_run.SealedExecutionProjection.from_dict(
        _compilation_execution_record()
    )
    cross_run.validate_sealed_systemd_configuration(
        _typed_observation(compilation),
        nonce=nonce,
        root=root,
        command=command,
        projection=projection,
    )
    # Neither the legacy replay boundary nor a different trusted budget may
    # approve an otherwise well-formed compilation observation.
    for expected in (
        cross_run.SEALED_REPLAY_EXECUTION,
        cross_run.SealedExecutionProjection.compilation(
            memory_max_bytes=1073741824,
            workspace_max_bytes=536870912,
            runtime_max_seconds=300,
        ),
    ):
        with pytest.raises(ValueError, match="effective configuration differs"):
            cross_run.validate_sealed_systemd_configuration(
                _typed_observation(compilation),
                nonce=nonce,
                root=root,
                command=command,
                projection=expected,
            )


@pytest.mark.parametrize(
    "field,value",
    [
        ("ReadOnlyPaths", []),
        ("ReadWritePaths", ["/work", "/bin"]),
        ("NoExecPaths", ["/tmp", "/evidence"]),
        ("WorkingDirectory", "/app"),
        ("MemoryMax", 2147483649),
        ("RuntimeMaxUSec", 300000001),
        ("LimitFSIZE", 536870913),
        ("LimitFSIZESoft", 536870913),
        ("PrivateNetwork", False),
        ("BindPaths", [["/host", "/work", False, 0]]),
        ("Environment", ["PATH=/bin", "HOME=/work"]),
    ],
)
def test_sealed_compilation_rejects_policy_weakening(field, value):
    from tests.tools.test_release_sealed_replay import _typed_observation

    nonce, root, command, interfaces = _compilation_systemd_observation()
    interfaces["Service"][field] = value
    with pytest.raises(ValueError, match=field):
        cross_run.validate_sealed_systemd_configuration(
            _typed_observation(interfaces),
            nonce=nonce,
            root=root,
            command=command,
            projection=cross_run.SealedExecutionProjection.from_dict(
                _compilation_execution_record()
            ),
        )


@pytest.mark.parametrize(
    "options",
    [
        "rw,nosuid,nodev,size=536870912,mode=1777",
        "rw,noexec,nosuid,nodev,size=536870913,mode=1777",
        "rw,noexec,nosuid,nodev,mode=1777",
    ],
)
def test_sealed_compilation_rejects_executable_or_unbounded_workspace(options):
    from tests.tools.test_release_sealed_replay import _typed_observation

    nonce, root, command, interfaces = _compilation_systemd_observation()
    interfaces["Service"]["TemporaryFileSystem"][-1] = ["/work", options]
    with pytest.raises(ValueError, match="TemporaryFileSystem"):
        cross_run.validate_sealed_systemd_configuration(
            _typed_observation(interfaces),
            nonce=nonce,
            root=root,
            command=command,
            projection=cross_run.SealedExecutionProjection.from_dict(
                _compilation_execution_record()
            ),
        )


@pytest.mark.parametrize("compilation", [False, True])
@pytest.mark.parametrize(
    "extent", [None, "large-export", "stdout-over-budget", "stderr-over-budget"]
)
def test_sealed_transport_retains_the_executed_projection(
    monkeypatch, tmp_path, compilation, extent
):
    from tests.tools.test_release_sealed_replay import (
        _systemd_observation,
        _typed_observation,
    )

    nonce, root, command, interfaces = (
        _compilation_systemd_observation() if compilation else _systemd_observation()
    )
    projection = (
        cross_run.SealedExecutionProjection.from_dict(_compilation_execution_record())
        if compilation
        else cross_run.SEALED_REPLAY_EXECUTION
    )
    raw = _typed_observation(interfaces)
    transport = cross_run.SealedRootTransport()
    transport._root = root
    transport._files = []
    transport._execution = projection
    transport._provider_identity = {}
    transport._evidence = tmp_path
    monkeypatch.setattr(cross_run, "_sealed_owned_root", lambda root: [])
    monkeypatch.setattr(transport, "_provider", lambda: {})
    monkeypatch.setattr(cross_run.secrets, "token_hex", lambda size: nonce)
    monkeypatch.setattr(
        transport, "_show", lambda unit: (raw, cross_run.parse_systemd_observation(raw))
    )
    monkeypatch.setattr(
        transport,
        "_control",
        lambda args, **kwargs: subprocess.CompletedProcess(args, 0, "loaded\n", ""),
    )

    def stop(unit):
        transport._owned.pop(unit)
        transport._unresolved.discard(unit)
        return {"fixture": "transport teardown only"}

    monkeypatch.setattr(transport, "_stop", stop)
    launches = []

    def guarded(args, **kwargs):
        launches.append(args)
        kwargs["stdout_capture_path"].write_bytes(b"")
        kwargs["stderr_capture_path"].write_bytes(b"")
        if extent is not None:
            stream = (
                "stderr_capture_path"
                if extent == "stderr-over-budget"
                else "stdout_capture_path"
            )
            size = (
                16 * 1024 * 1024 + 1
                if extent in {"large-export", "stderr-over-budget"}
                else projection.stdout_max_bytes + 1
            )
            with kwargs[stream].open("r+b") as output:
                output.truncate(size)  # Sparse fixture; no large allocation.
        return subprocess.CompletedProcess(args, 0, "", "")

    monkeypatch.setattr(cross_run, "_guarded_run", guarded)
    if (
        extent in {"stdout-over-budget", "stderr-over-budget"}
        or extent == "large-export"
        and not compilation
    ):
        with pytest.raises(ValueError, match="bounded transport"):
            transport.run_sealed(command, timeout=9, projection=projection)
        assert not transport._owned and not transport._unresolved
        return
    result = transport.run_sealed(command, timeout=9, projection=projection)
    assert result["execution"] == projection.to_dict()
    assert "--property=ReadOnlyPaths=/" in launches[0]
    assert (
        "--property=ReadWritePaths=/work"
        if compilation
        else "--property=ReadWritePaths="
    ) in launches[0]
    intent = json.loads((tmp_path / (nonce + ".intent.json")).read_text())
    assert intent["execution"] == result["execution"]


@pytest.mark.parametrize("imported_mode", [None, "replay", "compilation"])
def test_sealed_execution_rejects_projection_substitution_before_observation(
    monkeypatch, imported_mode
):
    compilation = cross_run.SealedExecutionProjection.from_dict(
        _compilation_execution_record()
    )
    transport = cross_run.SealedRootTransport()
    transport._execution = (
        cross_run.SEALED_REPLAY_EXECUTION
        if imported_mode == "replay"
        else compilation
        if imported_mode == "compilation"
        else None
    )
    requested = (
        cross_run.SEALED_REPLAY_EXECUTION
        if imported_mode == "compilation"
        else compilation
    )
    monkeypatch.setattr(
        transport, "_provider", lambda: pytest.fail("mismatch reached provider")
    )
    monkeypatch.setattr(
        cross_run,
        "_sealed_owned_root",
        lambda _: pytest.fail("mismatch reached imported root"),
    )
    with pytest.raises(ValueError, match="imported root projection"):
        transport.run_sealed([], timeout=9, projection=requested)


@pytest.mark.parametrize("compilation", [False, True])
def test_sealed_import_passes_projection_to_archive_admission(
    monkeypatch, tmp_path, compilation
):
    from types import SimpleNamespace
    from molt import toolchain_identity
    from tools.release import execution_root

    projection = (
        cross_run.SealedExecutionProjection.from_dict(_compilation_execution_record())
        if compilation
        else cross_run.SEALED_REPLAY_EXECUTION
    )
    transport = cross_run.SealedRootTransport()
    archive = tmp_path / "rootfs.tar"
    digest = "a" * 64
    monkeypatch.setattr(cross_run.sys, "platform", "linux")
    monkeypatch.setattr(transport, "_provider", lambda: {})
    monkeypatch.setattr(
        cross_run, "validate_sealed_systemd_provider", lambda *a, **k: None
    )
    monkeypatch.setattr(
        toolchain_identity,
        "capture_stable_regular_file",
        lambda *a, **k: (SimpleNamespace(sha256=digest, size=7), b"archive"),
    )
    monkeypatch.setattr(execution_root, "root_inventory", lambda _: [])

    class ArchiveBoundaryReached(Exception):
        pass

    def admit(path, root, *, expected_archive, projection):
        assert path == archive and root == tmp_path / "rootfs"
        assert expected_archive == {"sha256": digest, "size": 7}
        assert projection.to_dict() == (
            _compilation_execution_record()
            if compilation
            else {
                "schema": "molt.sealed-execution.v1",
                "mode": "replay",
                "memory_max_bytes": 1073741824,
                "workspace_max_bytes": 67108864,
                "runtime_max_seconds": 60,
            }
        )
        # Stop at the real archive-admission boundary, before memfd, directory
        # creation or service dispatch. Failed admission must not arm execution.
        raise ArchiveBoundaryReached

    monkeypatch.setattr(execution_root, "validate_sealed_tar", admit)
    monkeypatch.setattr(
        transport,
        "_control",
        lambda *a, **k: pytest.fail("unadmitted import dispatched"),
    )
    with pytest.raises(ArchiveBoundaryReached):
        transport.import_sealed_root(
            archive,
            sha256=digest,
            arch="aarch64",
            evidence=tmp_path / "evidence",
            projection=projection,
        )
    assert transport._execution is None
    assert not (tmp_path / "evidence").exists()


def test_sealed_import_rejects_replacement_of_active_projection(monkeypatch, tmp_path):
    transport = cross_run.SealedRootTransport()
    transport._execution = cross_run.SEALED_REPLAY_EXECUTION
    monkeypatch.setattr(
        transport, "_provider", lambda: pytest.fail("second import reached provider")
    )
    with pytest.raises(ValueError, match="existing root or unresolved custody"):
        transport.import_sealed_root(
            tmp_path / "another.tar",
            sha256="a" * 64,
            arch="aarch64",
            evidence=tmp_path / "evidence",
            projection=cross_run.SealedExecutionProjection.from_dict(
                _compilation_execution_record()
            ),
        )


@pytest.mark.parametrize("change", ["writable-tmp", "masked-tmp", "noexec-tmp"])
def test_sealed_compilation_cannot_replace_readonly_installed_tmp_inputs(change):
    from tests.tools.test_release_sealed_replay import _typed_observation

    nonce, root, command, interfaces = _compilation_systemd_observation()
    service = interfaces["Service"]
    assert service["ReadOnlyPaths"] == ["/"]
    assert all(row[0] != "/tmp" for row in service["TemporaryFileSystem"])
    if change == "writable-tmp":
        service["ReadWritePaths"].append("/tmp")
    elif change == "masked-tmp":
        service["TemporaryFileSystem"].append(
            ["/tmp", "rw,noexec,nosuid,nodev,size=64m,mode=1777"]
        )
    else:
        service["NoExecPaths"].append("/tmp")
    with pytest.raises(ValueError, match="effective configuration differs"):
        cross_run.validate_sealed_systemd_configuration(
            _typed_observation(interfaces),
            nonce=nonce,
            root=root,
            command=command,
            projection=cross_run.SealedExecutionProjection.from_dict(
                _compilation_execution_record()
            ),
        )

from __future__ import annotations

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

    assert cross_run._local_expected(cross_run.Case("sample", "print('ok')\n")) == "ok\n"
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


def test_ssh_transport_uses_memory_guard(monkeypatch) -> None:
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
    code, stdout, stderr = transport.run([host.remote_path("binary")], timeout=7)

    assert code == 0
    assert stdout == "remote\n"
    assert stderr == ""
    assert calls[0][0] == "ssh"
    assert calls[1][0] == "ssh"


def test_docker_transport_run_uses_memory_guard(monkeypatch, tmp_path: Path) -> None:
    captured: dict[str, object] = {}
    host = cross_run.Host(
        name="docker",
        target="x86_64-unknown-linux-gnu",
        transport="docker",
        container="example:latest",
    )
    transport = cross_run.DockerTransport(host)
    transport._stage = tmp_path

    def fake_guarded_completed_process(cmd, **kwargs):
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return subprocess.CompletedProcess(cmd, 0, stdout="ok\n", stderr="")

    monkeypatch.setattr(
        cross_run.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    code, stdout, stderr = transport.run(["./binary"], timeout=9)

    assert code == 0
    assert stdout == "ok\n"
    assert stderr == ""
    assert captured["cmd"][:2] == ["docker", "run"]
    assert captured["kwargs"]["prefix"] == "MOLT_CROSS"
    assert captured["kwargs"]["timeout"] == 9

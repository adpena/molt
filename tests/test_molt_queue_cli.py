from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys

import pytest

from molt.cli import entrypoint_parser
from molt.cli import queue_cli


def test_molt_queue_parser_preserves_queue_args() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(["queue", "run", "--detach", "--queue-size", "3"])

    assert args.command == "queue"
    assert args.queue_size is None
    assert args.queue_args == ["run", "--detach", "--queue-size", "3"]


def test_molt_queue_parser_accepts_portable_queue_size_default() -> None:
    parser = entrypoint_parser._build_entrypoint_parser()
    args = parser.parse_args(["queue", "--queue-size", "3", "run", "--detach"])

    assert args.command == "queue"
    assert args.queue_size == "3"
    assert args.queue_args == ["run", "--detach"]


def test_molt_queue_invokes_proof_queue_without_shell(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    calls: list[dict[str, object]] = []

    def fake_find_molt_root(cwd: Path) -> Path:
        assert cwd == Path.cwd()
        return tmp_path

    def fake_run(command: list[str], *, cwd: Path) -> subprocess.CompletedProcess[str]:
        calls.append({"command": command, "cwd": cwd})
        return subprocess.CompletedProcess(command, 17)

    monkeypatch.setattr(queue_cli, "_find_molt_root", fake_find_molt_root)
    monkeypatch.setattr(queue_cli.subprocess, "run", fake_run)

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(queue_args=["run", "--detach", "--queue-size", "2"])
    )

    assert rc == 17
    assert calls == [
        {
            "command": [
                sys.executable,
                str(script),
                "run",
                "--detach",
                "--queue-size",
                "2",
            ],
            "cwd": tmp_path,
        }
    ]


def test_molt_queue_queue_size_sets_portable_env(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    calls: list[tuple[list[str], Path, dict[str, str] | None]] = []

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        calls.append((command, cwd, env))
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(queue_cli.subprocess, "run", fake_run)

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(queue_size="3", queue_args=["run", "--detach"])
    )

    assert rc == 0
    command, cwd, env = calls[0]
    assert command == [
        sys.executable,
        str(script),
        "run",
        "--detach",
    ]
    assert cwd == tmp_path
    assert env is not None
    assert env[queue_cli.PROOF_QUEUE_SIZE_ENV] == "3"


def test_molt_queue_rejects_duplicate_queue_size_authority(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(
        queue_cli.subprocess,
        "run",
        lambda *args, **kwargs: pytest.fail("duplicate capacity must fail closed"),
    )

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(
            queue_size="3",
            queue_args=["run", "--detach", "--queue-size", "2"],
        )
    )

    assert rc == 2
    assert "use either top-level --queue-size" in capsys.readouterr().err


def test_molt_queue_strips_separator_and_preserves_argv(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    calls: list[list[str]] = []

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(
        queue_cli.subprocess,
        "run",
        lambda command, *, cwd: (
            calls.append(command) or subprocess.CompletedProcess(command, 0)
        ),
    )

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(
            queue_args=[
                "--",
                "exec",
                "--reason",
                "portable queue argv",
                "--",
                sys.executable,
                "-c",
                "print('ok')",
            ]
        )
    )

    assert rc == 0
    assert calls == [
        [
            sys.executable,
            str(script),
            "exec",
            "--reason",
            "portable queue argv",
            "--",
            sys.executable,
            "-c",
            "print('ok')",
        ]
    ]


def test_molt_queue_defaults_to_quickstart(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    calls: list[list[str]] = []

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(
        queue_cli.subprocess,
        "run",
        lambda command, *, cwd: (
            calls.append(command) or subprocess.CompletedProcess(command, 0)
        ),
    )

    assert queue_cli.handle_queue_command(argparse.Namespace(queue_args=[])) == 0
    assert calls == [[sys.executable, str(script), "quickstart"]]

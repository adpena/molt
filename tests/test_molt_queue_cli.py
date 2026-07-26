from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import sys

import pytest

from molt.cli import entrypoint_parser
from molt.cli import queue_cli
from molt.dx import checkout_custody


@pytest.fixture(autouse=True)
def _isolate_synthetic_queue_repos_from_hosted_checkout_contract(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.delenv("MOLT_CI_EPHEMERAL_CUSTODY_ROOT", raising=False)


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

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        calls.append({"command": command, "cwd": cwd, "env": env})
        return subprocess.CompletedProcess(command, 17)

    monkeypatch.setattr(queue_cli, "_find_molt_root", fake_find_molt_root)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)

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
            "env": calls[0]["env"],
        }
    ]
    assert calls[0]["env"] is not None


def test_molt_queue_preserves_hostile_paths_and_args_without_shell(
    monkeypatch, tmp_path: Path
) -> None:
    repo_root = tmp_path / "Molt Root With Spaces & Symbols"
    script = repo_root / "tools" / "proof_queue.py"
    script.parent.mkdir(parents=True)
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    calls: list[dict[str, object]] = []

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        calls.append({"command": command, "cwd": cwd, "env": env})
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: repo_root)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(
            queue_args=[
                "exec",
                "--reason",
                "portable argv: spaces & pipes | dollars $HOME %TEMP%",
                "--",
                sys.executable,
                "-c",
                "print('queue argv ok')",
            ]
        )
    )

    assert rc == 0
    assert calls == [
        {
            "command": [
                sys.executable,
                str(script),
                "exec",
                "--reason",
                "portable argv: spaces & pipes | dollars $HOME %TEMP%",
                "--",
                sys.executable,
                "-c",
                "print('queue argv ok')",
            ],
            "cwd": repo_root,
            "env": calls[0]["env"],
        }
    ]
    assert calls[0]["env"] is not None


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
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)
    monkeypatch.delenv("MOLT_TARGET_ROOT", raising=False)

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
    ext_root = Path(env["MOLT_EXT_ROOT"])
    assert ext_root.is_absolute()
    assert env["CARGO_TARGET_DIR"].startswith(str(ext_root / "target"))
    assert Path(env["MOLT_TARGET_ROOT"]) == checkout_custody(tmp_path).toolchain_root


@pytest.mark.parametrize("queue_size", ["0", "-1", "banana"])
def test_molt_queue_rejects_invalid_top_level_queue_size(
    monkeypatch, tmp_path: Path, capsys, queue_size: str
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(
        queue_cli.process_guard,
        "run_completed_command",
        lambda *args, **kwargs: pytest.fail("invalid capacity must fail closed"),
    )

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(queue_size=queue_size, queue_args=["status"])
    )

    assert rc == 2
    assert "--queue-size must be a positive integer" in capsys.readouterr().err


def test_molt_queue_queue_size_is_child_env_only(monkeypatch, tmp_path: Path) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    calls: list[dict[str, str] | None] = []

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        calls.append(env)
        del cwd
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setenv(queue_cli.PROOF_QUEUE_SIZE_ENV, "99")
    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)

    rc = queue_cli.handle_queue_command(
        argparse.Namespace(queue_size="3", queue_args=["run", "--detach"])
    )

    assert rc == 0
    assert calls[0] is not None
    assert calls[0][queue_cli.PROOF_QUEUE_SIZE_ENV] == "3"
    assert queue_cli.os.environ[queue_cli.PROOF_QUEUE_SIZE_ENV] == "99"


def test_molt_queue_rejects_duplicate_queue_size_authority(
    monkeypatch, tmp_path: Path, capsys
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")

    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(
        queue_cli.process_guard,
        "run_completed_command",
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
        queue_cli.process_guard,
        "run_completed_command",
        lambda command, *, cwd, env=None: (
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
        queue_cli.process_guard,
        "run_completed_command",
        lambda command, *, cwd, env=None: (
            calls.append(command) or subprocess.CompletedProcess(command, 0)
        ),
    )

    assert queue_cli.handle_queue_command(argparse.Namespace(queue_args=[])) == 0
    assert calls == [[sys.executable, str(script), "quickstart"]]


def test_molt_queue_preserves_active_warm_project_env(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    warm_venv = tmp_path / ".venv"
    warm_venv.mkdir()
    (warm_venv / "pyvenv.cfg").write_text("", encoding="utf-8")
    calls: list[dict[str, str] | None] = []

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        del cwd
        calls.append(env)
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.setenv("VIRTUAL_ENV", str(warm_venv))
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)

    rc = queue_cli.handle_queue_command(argparse.Namespace(queue_args=["status"]))

    assert rc == 0
    assert calls[0] is not None
    assert calls[0]["UV_PROJECT_ENVIRONMENT"] == str(warm_venv.resolve())


def test_molt_queue_uses_repo_local_warm_project_env_without_active_env(
    monkeypatch, tmp_path: Path
) -> None:
    script = tmp_path / "tools" / "proof_queue.py"
    script.parent.mkdir()
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    repo_venv = tmp_path / ".venv"
    repo_venv.mkdir()
    (repo_venv / "pyvenv.cfg").write_text("", encoding="utf-8")
    calls: list[dict[str, str] | None] = []

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        del cwd
        calls.append(env)
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: tmp_path)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)

    rc = queue_cli.handle_queue_command(argparse.Namespace(queue_args=["status"]))

    assert rc == 0
    assert calls[0] is not None
    assert calls[0]["UV_PROJECT_ENVIRONMENT"] == str(repo_venv.resolve())


def test_molt_queue_uses_main_worktree_project_env_for_linked_worktree(
    monkeypatch, tmp_path: Path
) -> None:
    main_repo = tmp_path / "main"
    worktree = tmp_path / "worktrees" / "lane"
    script = worktree / "tools" / "proof_queue.py"
    script.parent.mkdir(parents=True)
    script.write_text("raise SystemExit(0)\n", encoding="utf-8")
    (worktree / ".git").write_text(
        "gitdir: ../../main/.git/worktrees/lane\n", encoding="utf-8"
    )
    common_git = main_repo / ".git"
    common_git.mkdir(parents=True)
    main_venv = main_repo / ".venv"
    main_venv.mkdir()
    (main_venv / "pyvenv.cfg").write_text("", encoding="utf-8")
    calls: list[dict[str, str] | None] = []

    def fake_run(
        command: list[str],
        *,
        cwd: Path,
        env: dict[str, str] | None = None,
        capture_output: bool = False,
        text: bool = False,
        timeout: int | None = None,
    ) -> subprocess.CompletedProcess[str]:
        del capture_output, text, timeout
        if command == ["git", "rev-parse", "--git-common-dir"]:
            assert cwd == worktree
            return subprocess.CompletedProcess(command, 0, stdout=str(common_git))
        calls.append(env)
        return subprocess.CompletedProcess(command, 0)

    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(queue_cli, "_find_molt_root", lambda cwd: worktree)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)

    rc = queue_cli.handle_queue_command(argparse.Namespace(queue_args=["status"]))

    assert rc == 0
    assert calls[0] is not None
    assert calls[0]["UV_PROJECT_ENVIRONMENT"] == str(main_venv.resolve())

from __future__ import annotations

import argparse
from collections.abc import Iterator
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys

import pytest

from molt.cli import entrypoint_parser
from molt.cli import queue_cli
from molt.dx import checkout_custody


pytestmark = pytest.mark.usefixtures("developer_host_context")


@pytest.fixture(autouse=True)
def _isolate_synthetic_queue_repos_from_hosted_cargo_target(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Hosted CI exports its Cargo cache target; an explicit target directory
    # rightly wins over the custody-derived one these tests assert.
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)


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

    def fake_compiler_source_root() -> Path:
        return tmp_path

    def fake_run(
        command: list[str], *, cwd: Path, env: dict[str, str] | None = None
    ) -> int:
        calls.append({"command": command, "cwd": cwd, "env": env})
        return 17

    monkeypatch.setattr(queue_cli, "compiler_source_root", fake_compiler_source_root)
    monkeypatch.setattr(queue_cli, "forward_command", fake_run)

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
    ) -> int:
        calls.append({"command": command, "cwd": cwd, "env": env})
        return 0

    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: repo_root)
    monkeypatch.setattr(queue_cli, "forward_command", fake_run)

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
    ) -> int:
        calls.append((command, cwd, env))
        return 0

    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(queue_cli, "forward_command", fake_run)
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

    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(
        queue_cli,
        "forward_command",
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
    ) -> int:
        calls.append(env)
        del cwd
        return 0

    monkeypatch.setenv(queue_cli.PROOF_QUEUE_SIZE_ENV, "99")
    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(queue_cli, "forward_command", fake_run)

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

    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(
        queue_cli,
        "forward_command",
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

    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(
        queue_cli,
        "forward_command",
        lambda command, *, cwd, env=None: calls.append(command) or 0,
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

    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(
        queue_cli,
        "forward_command",
        lambda command, *, cwd, env=None: calls.append(command) or 0,
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
    ) -> int:
        del cwd
        calls.append(env)
        return 0

    monkeypatch.setenv("VIRTUAL_ENV", str(warm_venv))
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(queue_cli, "forward_command", fake_run)

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
    ) -> int:
        del cwd
        calls.append(env)
        return 0

    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: tmp_path)
    monkeypatch.setattr(queue_cli, "forward_command", fake_run)

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
        del capture_output, text, timeout, env
        assert command == ["git", "rev-parse", "--git-common-dir"]
        assert cwd == worktree
        return subprocess.CompletedProcess(command, 0, stdout=str(common_git))

    monkeypatch.delenv("VIRTUAL_ENV", raising=False)
    monkeypatch.delenv("UV_PROJECT_ENVIRONMENT", raising=False)
    monkeypatch.setattr(queue_cli, "compiler_source_root", lambda: worktree)
    monkeypatch.setattr(queue_cli.process_guard, "run_completed_command", fake_run)
    monkeypatch.setattr(
        queue_cli,
        "forward_command",
        lambda command, *, cwd, env: calls.append(env) or 0,
    )

    rc = queue_cli.handle_queue_command(argparse.Namespace(queue_args=["status"]))

    assert rc == 0
    assert calls[0] is not None
    assert calls[0]["UV_PROJECT_ENVIRONMENT"] == str(main_venv.resolve())


_QUEUE_FORWARDING_FIXTURE = r"""
import argparse
import json
import os
from pathlib import Path
import socket
import sys
import time

root = Path(__file__).resolve().parents[1]
if "--run-id" in sys.argv:
    port = int(sys.argv[sys.argv.index("--run-id") + 1])
    with socket.create_connection(("127.0.0.1", port), timeout=20) as connection:
        connection.sendall(f"{os.getpid()} {os.getppid()}\n".encode())
        (root / "runner-connected").write_text(str(os.getpid()), encoding="utf-8")
        token = connection.recv(1)
        if token:
            connection.sendall(token)
    raise SystemExit(0)

mode = sys.argv[1]
if mode == "lifetime":
    # Use the real queue's detached process boundary. This fixture controls
    # only the runner body; it does not fabricate a successful proof receipt.
    from tools.proof_queue_pkg import custody, state
    state.ROOT = root
    args = argparse.Namespace(logs_root=root / "logs", repo_root=root)
    pid, log = custody._launch_detached_runner(
        args, run_id=sys.argv[2], timeout=20,
    )
    deadline = time.monotonic() + 20
    while not (root / "runner-connected").exists():
        if time.monotonic() >= deadline:
            raise RuntimeError(f"detached runner did not connect; log={log.read_text()}")
        time.sleep(0.01)
    print(json.dumps({"pid": pid, "log": str(log)}), flush=True)
    raise SystemExit(0)
if mode == "signal":
    with socket.create_connection(("127.0.0.1", int(sys.argv[2])), timeout=20) as connection:
        connection.sendall(f"{os.getpid()}\n".encode())
        connection.recv(1)
    raise SystemExit(0)
print(json.dumps({
    "argv": sys.argv[3:], "cwd": str(Path.cwd()), "source": str(root),
    "queue_size": os.environ.get("MOLT_PROOF_QUEUE_SIZE"),
}), flush=True)
raise SystemExit(int(sys.argv[2]))
"""


@pytest.fixture(scope="module")
def real_queue_source(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Keep the CLI and its strict tool imports in one complete Python tree."""
    root = tmp_path_factory.mktemp("queue-forwarding") / "queue source & symbols"
    project = Path(__file__).resolve().parents[1]
    # Queue custody imports proof-plan authority, which validates every loaded
    # tools/molt module against this tree. Copy these two packages once rather
    # than assembling a fragile list of transitive guard dependencies.
    for relative in ("tools", "src/molt"):
        shutil.copytree(
            project / relative,
            root / relative,
            ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
        )
    (root / "pyproject.toml").write_bytes((project / "pyproject.toml").read_bytes())
    (root / "tools" / "proof_queue.py").write_text(
        _QUEUE_FORWARDING_FIXTURE, encoding="utf-8"
    )
    return root


@contextmanager
def _real_queue_cli(source: Path, *args: str) -> Iterator[subprocess.Popen[str]]:
    root = source
    # Keep ambient environment values out of the fixture's failure repr.
    env = os.environ.copy()
    # No compilation occurs; compiler-cache discovery is outside this fixture.
    env["MOLT_USE_SCCACHE"] = "0"
    env["MOLT_SOURCE_ROOT"] = str(root)
    env["MOLT_EXT_ROOT"] = str(root.parent / "artifacts")
    env["PYTHONPATH"] = os.pathsep.join([str(root / "src"), str(root)])
    # The test owns this bounded control-plane child and any socket runner until
    # its rendezvous closes. A nested command guard would itself reap the runner
    # at CLI exit, hiding the ownership contract being exercised here.
    with subprocess.Popen(
        [sys.executable, "-m", "molt.cli", "queue", "--queue-size", "3", "--", *args],
        cwd=root.parent,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        encoding="utf-8",
    ) as process:
        failure: BaseException | None = None
        try:
            yield process
        except BaseException as error:
            failure = error
            raise
        finally:
            try:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=10)
                if failure is not None:
                    stdout, stderr = process.communicate(timeout=10)
                    failure.add_note(
                        f"public queue CLI rc={process.returncode}\n"
                        f"stdout={stdout}\nstderr={stderr}"
                    )
            except BaseException as cleanup_error:
                if failure is None:
                    raise
                failure.add_note(f"queue fixture cleanup failed: {cleanup_error!r}")


@pytest.mark.parametrize("status", [0, 17])
def test_molt_queue_real_cli_preserves_exit_argv_source_and_cwd(
    real_queue_source: Path, status: int
) -> None:
    argument = "spaces & pipes | dollars $HOME %TEMP%"
    with _real_queue_cli(real_queue_source, "exit", str(status), argument) as process:
        stdout, stderr = process.communicate(timeout=30)
        assert process.returncode == status, stderr
    root = real_queue_source
    assert json.loads(stdout) == {
        "argv": [argument],
        "cwd": str(root),
        "source": str(root),
        "queue_size": "3",
    }


def test_molt_queue_real_cli_leaves_detached_runner_with_queue_owner(
    real_queue_source: Path,
) -> None:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(30)
        with _real_queue_cli(
            real_queue_source, "lifetime", str(listener.getsockname()[1])
        ) as process:
            connection, _ = listener.accept()
            with connection, connection.makefile("rb") as stream:
                connection.settimeout(30)
                worker_pid, worker_parent = map(int, stream.readline().split())
                stdout, stderr = process.communicate(timeout=30)
                assert process.returncode == 0, stderr
                # The queue owns the process it launched. A Windows venv's
                # python.exe is a redirector that runs the interpreter as its
                # child, so there the owned process is the worker's parent.
                redirected = os.name == "nt" and os.path.normcase(
                    sys.executable
                ) != os.path.normcase(sys._base_executable)
                assert json.loads(stdout)["pid"] == (
                    worker_parent if redirected else worker_pid
                )
                # This message is first sent after the public CLI has exited.
                # Echo then EOF proves the detached runner remained alive and
                # completed its bounded operation under the queue's ownership.
                connection.sendall(b"Q")
                assert stream.read(1) == b"Q"
                assert stream.read(1) == b""


@pytest.mark.skipif(os.name == "nt", reason="POSIX exec and signal identity")
@pytest.mark.parametrize("signum", [signal.SIGTERM, signal.SIGINT])
def test_molt_queue_real_cli_preserves_process_and_signal_identity(
    real_queue_source: Path, signum: int
) -> None:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(30)
        with _real_queue_cli(
            real_queue_source, "signal", str(listener.getsockname()[1])
        ) as process:
            connection, _ = listener.accept()
            with connection, connection.makefile("rb") as stream:
                connection.settimeout(30)
                assert int(stream.readline()) == process.pid
                process.send_signal(signum)
                process.communicate(timeout=30)
                assert process.returncode == -signum
                assert stream.read(1) == b""

#!/usr/bin/env python3
from __future__ import annotations

import os
import platform
import shlex
import shutil
import subprocess
import sys
import time
import importlib.util
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402


def _load_dx_module():
    dx_path = ROOT / "src" / "molt" / "dx.py"
    spec = importlib.util.spec_from_file_location("molt_dx_project", dx_path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load DX planner from {dx_path}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


_DX_MODULE = _load_dx_module()
TEST_PYTHONS = _DX_MODULE.TEST_PYTHONS
DxProject = _DX_MODULE.DxProject

DX = DxProject(ROOT)


def _log(msg: str) -> None:
    stamp = datetime.now().isoformat(timespec="seconds")
    print(f"[dev.py {stamp}] {msg}")


def _run_with_pty(cmd: list[str], env: dict[str, str]) -> None:
    import os
    import pty

    master_fd, slave_fd = pty.openpty()
    try:
        proc = subprocess.Popen(
            cmd,
            cwd=ROOT,
            env=env,
            stdin=slave_fd,
            stdout=slave_fd,
            stderr=slave_fd,
        )
    finally:
        os.close(slave_fd)

    try:
        while True:
            data = os.read(master_fd, 1024)
            if not data:
                break
            if hasattr(sys.stdout, "buffer"):
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
            else:
                sys.stdout.write(data.decode(errors="replace"))
                sys.stdout.flush()
    except KeyboardInterrupt:
        proc.terminate()
        raise
    finally:
        os.close(master_fd)

    rc = proc.wait()
    if rc != 0:
        raise subprocess.CalledProcessError(rc, cmd)


def _check_call_guarded(
    cmd: list[str],
    env: dict[str, str],
    *,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> None:
    resolved_limits = limits or harness_memory_guard.limits_from_env(
        "MOLT_TEST_SUITE", env
    )
    result = harness_memory_guard.guarded_completed_process(
        cmd,
        prefix="MOLT_TEST_SUITE",
        cwd=ROOT,
        env=env,
        capture_output=False,
        text=True,
        limits=resolved_limits,
        stream="stderr",
    )
    if result.returncode != 0:
        if result.stderr:
            print(result.stderr, file=sys.stderr, end="")
        raise subprocess.CalledProcessError(result.returncode, cmd)


def _uv_project_env_dir() -> Path:
    return DX.project_env_dir()


def _uv_project_python() -> Path:
    return DX.project_python()


def _uv_project_env_matches_python(requested: str | None) -> bool:
    return DX.project_env_matches_python(requested)


def _normalized_uv_run_env(
    env: dict[str, str],
    *,
    python: str | None,
) -> dict[str, str]:
    return DX.normalized_uv_run_env(env, python=python)


def run_uv(
    args: list[str],
    python: str | None = None,
    env: dict[str, str] | None = None,
    tty: bool = False,
) -> None:
    cmd = ["uv", "run"]
    if python:
        cmd.extend(["--python", python])
        if (
            python == "3.14"
            and sys.platform == "darwin"
            and platform.machine().lower() in {"arm64", "aarch64"}
        ):
            if shutil.which("python3.14"):
                cmd.append("--no-managed-python")
            else:
                raise RuntimeError(
                    "uv-managed Python 3.14 hangs on arm64; install python3.14 "
                    "or remove 3.14 from the test matrix."
                )
    cmd.extend(args)
    run_env = _normalized_uv_run_env(env or os.environ, python=python)
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", run_env)
    if tty and os.name == "posix" and not limits.enabled:
        _run_with_pty(cmd, run_env)
    else:
        _check_call_guarded(cmd, run_env, limits=limits)


def _apply_dev_trusted(env: dict[str, str]) -> None:
    raw = env.get("MOLT_DEV_TRUSTED", "").strip().lower()
    if raw and raw in {"0", "false", "no", "off"}:
        return
    env.setdefault("MOLT_TRUSTED", "1")


def _parse_test_runner_flags(args: list[str]) -> tuple[list[str], bool, str | None]:
    remaining: list[str] = []
    random_order = False
    random_seed: str | None = None
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--random-order":
            random_order = True
            i += 1
            continue
        if arg == "--random-seed":
            if i + 1 >= len(args):
                raise RuntimeError("--random-seed requires a value")
            random_order = True
            random_seed = args[i + 1]
            i += 2
            continue
        remaining.append(arg)
        i += 1
    return remaining, random_order, random_seed


def _load_dx_config() -> dict[str, object]:
    return DX.load_config()


def _canonical_env(*, create_dirs: bool = True) -> dict[str, str]:
    return DX.canonical_env(create_dirs=create_dirs)


def _dx_commands() -> dict[str, object]:
    return DX.commands()


def _format_dx_command(command: str) -> str:
    return DX.format_command(command)


def _split_command(command: object, name: str) -> list[str]:
    return DX.split_command(command, name)


def _split_command_sequence(
    command: object,
    name: str,
    *,
    commands: dict[str, object] | None = None,
    stack: tuple[str, ...] = (),
) -> list[list[str]]:
    return DX.split_command_sequence(
        command,
        name,
        commands=commands,
        stack=stack,
    )


def _run_repo_cmd(cmd: list[str], env: dict[str, str], *, tty: bool) -> None:
    _log("$ " + " ".join(shlex.quote(part) for part in cmd))
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)
    if tty and os.name == "posix" and not limits.enabled:
        _run_with_pty(cmd, env)
    else:
        _check_call_guarded(cmd, env, limits=limits)


def _run_dx_command(name: str, env: dict[str, str], *, tty: bool) -> None:
    command = _dx_commands().get(name)
    _run_repo_cmd(_split_command(command, name), env, tty=tty)


def _run_dx_command_with_args(
    name: str,
    extra_args: list[str],
    env: dict[str, str],
    *,
    tty: bool,
) -> None:
    command = _dx_commands().get(name)
    _run_repo_cmd([*_split_command(command, name), *extra_args], env, tty=tty)


def _require_project_python() -> Path:
    return DX.require_project_python("repo gates")


def _print_canonical_env(env: dict[str, str]) -> None:
    keys = [
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_DIFF_CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
        "MOLT_SESSION_ID",
        "MOLT_BACKEND_DAEMON",
        "CARGO_BUILD_JOBS",
        "PYTHONPATH",
    ]
    for key in keys:
        print(f"export {key}={shlex.quote(env[key])}")


def _run_dx_gates(args: list[str], *, tty: bool) -> None:
    allow_dirty = "--allow-dirty" in args
    unknown = [arg for arg in args if arg != "--allow-dirty"]
    if unknown:
        raise RuntimeError(
            "Unrecognized tools/dev.py gates arguments: " + " ".join(unknown)
        )
    env = _canonical_env()
    _require_project_python()
    commands = _dx_commands()
    gates = commands.get("gates")
    if gates is None:
        for name in ("build", "backend", "compliance"):
            _run_dx_command(name, env, tty=tty)
    else:
        for gate_cmd in _split_command_sequence(gates, "gates"):
            _run_repo_cmd(gate_cmd, env, tty=tty)
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)
    status_result = harness_memory_guard.guarded_completed_process(
        ["git", "status", "--short"],
        prefix="MOLT_TEST_SUITE",
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        limits=limits,
    )
    if status_result.returncode != 0:
        raise subprocess.CalledProcessError(
            status_result.returncode,
            ["git", "status", "--short"],
            output=status_result.stdout,
            stderr=status_result.stderr,
        )
    status = status_result.stdout
    if status:
        print(status, end="")
        if not allow_dirty:
            raise RuntimeError(
                "working tree is dirty; rerun with --allow-dirty while developing"
            )
    else:
        _log("git status clean")


def main() -> None:
    cmd = sys.argv[1:] or ["help"]
    use_tty = "--tty" in cmd or os.environ.get("MOLT_TTY") == "1"
    if use_tty:
        cmd = [arg for arg in cmd if arg != "--tty"]
    if not cmd:
        cmd = ["help"]
    if cmd[0] == "env":
        _print_canonical_env(_canonical_env())
    elif cmd[0] == "install":
        _run_dx_command("install", _canonical_env(), tty=use_tty)
    elif cmd[0] == "clippy":
        _run_dx_command("clippy", _canonical_env(), tty=use_tty)
    elif cmd[0] == "security":
        _run_dx_command("security", _canonical_env(), tty=use_tty)
    elif cmd[0] == "compliance":
        _require_project_python()
        _run_dx_command("compliance", _canonical_env(), tty=use_tty)
    elif cmd[0] == "backend":
        _run_dx_command("backend", _canonical_env(), tty=use_tty)
    elif cmd[0] == "gates":
        _run_dx_gates(cmd[1:], tty=use_tty)
    elif cmd[0] == "lint":
        env = _canonical_env()
        _require_project_python()
        for lint_cmd in _split_command_sequence(_dx_commands().get("lint"), "lint"):
            _run_repo_cmd(lint_cmd, env, tty=use_tty)
    elif cmd[0] == "test":
        env = os.environ.copy()
        _apply_dev_trusted(env)
        test_cmd_args, random_order, random_seed = _parse_test_runner_flags(cmd[1:])
        if test_cmd_args:
            raise RuntimeError(
                "Unrecognized tools/dev.py test arguments: " + " ".join(test_cmd_args)
            )
        src_path = str(ROOT / "src")
        existing = env.get("PYTHONPATH", "")
        env["PYTHONPATH"] = (
            src_path if not existing else f"{src_path}{os.pathsep}{existing}"
        )
        _log(f"PYTHONPATH={env['PYTHONPATH']}")
        for python in TEST_PYTHONS:
            _log(f"tests start (python {python})")
            start = time.monotonic()
            batch_cmd = ["python3", "tools/dev_test_runner.py"]
            if python == TEST_PYTHONS[0]:
                batch_cmd.append("--verified-subset")
            if random_order:
                batch_cmd.append("--random-order")
            if random_seed is not None:
                batch_cmd.extend(["--random-seed", random_seed])
            run_uv(batch_cmd, python=python, env=env, tty=use_tty)
            _log(f"tests done (python {python}) in {time.monotonic() - start:.2f}s")
    elif cmd[0] == "doctor":
        run_uv(
            ["python3", "-m", "molt.cli", "doctor", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "setup":
        run_uv(
            ["python3", "-m", "molt.cli", "setup", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "update":
        run_uv(
            ["python3", "-m", "molt.cli", "update", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "validate":
        run_uv(
            ["python3", "-m", "molt.cli", "validate", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "clean-artifacts":
        _run_dx_command_with_args(
            "clean-artifacts",
            cmd[1:],
            _canonical_env(create_dirs=False),
            tty=use_tty,
        )
    else:
        print(
            "Usage: tools/dev.py "
            "[env|install|clippy|security|compliance|backend|gates|lint|test|setup|doctor|update|validate|clean-artifacts]"
        )


if __name__ == "__main__":
    main()

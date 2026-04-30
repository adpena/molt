#!/usr/bin/env python3
from __future__ import annotations

import os
import platform
import shlex
import shutil
import subprocess
import sys
import time
import tomllib
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TEST_PYTHONS = ["3.12", "3.13", "3.14"]


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


def _uv_project_env_dir() -> Path:
    return ROOT / ".venv"


def _uv_project_python() -> Path:
    if os.name == "nt":
        return _uv_project_env_dir() / "Scripts" / "python.exe"
    return _uv_project_env_dir() / "bin" / "python3"


def _uv_project_env_matches_python(requested: str | None) -> bool:
    project_python = _uv_project_python()
    if not project_python.exists():
        return False
    if not requested:
        return True
    try:
        proc = subprocess.run(
            [
                str(project_python),
                "-c",
                ("import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')"),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return False
    return proc.stdout.strip() == requested


def _normalized_uv_run_env(
    env: dict[str, str],
    *,
    python: str | None,
) -> dict[str, str]:
    run_env = env.copy()
    run_env.setdefault("PYTHONUNBUFFERED", "1")
    run_env["UV_PROJECT_ENVIRONMENT"] = str(_uv_project_env_dir())
    for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
        run_env.pop(name, None)
    if run_env.get("UV_NO_SYNC") == "1" and not _uv_project_env_matches_python(python):
        run_env.pop("UV_NO_SYNC", None)
    return run_env


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
    if tty and os.name == "posix":
        _run_with_pty(cmd, run_env)
    else:
        subprocess.check_call(cmd, cwd=ROOT, env=run_env)


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
    with (ROOT / "pyproject.toml").open("rb") as fh:
        data = tomllib.load(fh)
    tool = data.get("tool", {})
    if not isinstance(tool, dict):
        return {}
    molt = tool.get("molt", {})
    if not isinstance(molt, dict):
        return {}
    dx = molt.get("dx", {})
    return dx if isinstance(dx, dict) else {}


def _canonical_env() -> dict[str, str]:
    dx = _load_dx_config()
    env = os.environ.copy()
    for name in ("VIRTUAL_ENV", "PYTHONHOME", "CONDA_PREFIX", "CONDA_DEFAULT_ENV"):
        env.pop(name, None)
    env_cfg = dx.get("env", {})
    if isinstance(env_cfg, dict):
        for key, raw_value in env_cfg.items():
            if not isinstance(key, str) or not isinstance(raw_value, str):
                continue
            env[key] = raw_value.format(root=str(ROOT))
    env.setdefault("MOLT_SESSION_ID", f"dev-{os.getpid()}")
    env.setdefault("MOLT_BACKEND_DAEMON", "1" if dx.get("backend_daemon") else "0")
    env.setdefault("CARGO_BUILD_JOBS", str(dx.get("cargo_build_jobs", 2)))
    for dirname in (
        env["CARGO_TARGET_DIR"],
        env["MOLT_CACHE"],
        env["MOLT_DIFF_ROOT"],
        env["MOLT_DIFF_TMPDIR"],
        env["UV_CACHE_DIR"],
        env["TMPDIR"],
    ):
        Path(dirname).mkdir(parents=True, exist_ok=True)
    return env


def _dx_commands() -> dict[str, object]:
    dx = _load_dx_config()
    commands = dx.get("commands", {})
    return commands if isinstance(commands, dict) else {}


def _format_dx_command(command: str) -> str:
    return command.format(root=str(ROOT), project_python=str(_uv_project_python()))


def _split_command(command: object, name: str) -> list[str]:
    if not isinstance(command, str) or not command.strip():
        raise RuntimeError(f"Missing [tool.molt.dx.commands].{name}")
    return shlex.split(_format_dx_command(command), posix=os.name != "nt")


def _split_command_sequence(
    command: object,
    name: str,
    *,
    commands: dict[str, object] | None = None,
    stack: tuple[str, ...] = (),
) -> list[list[str]]:
    commands = _dx_commands() if commands is None else commands

    def split_item(item: str, item_name: str) -> list[list[str]]:
        stripped = item.strip()
        if stripped.startswith("@"):
            ref = stripped[1:]
            if not ref or any(ch.isspace() for ch in ref):
                raise RuntimeError(
                    f"Invalid [tool.molt.dx.commands].{item_name} reference: {item!r}"
                )
            if ref in stack:
                chain = " -> ".join((*stack, ref))
                raise RuntimeError(f"Cyclic [tool.molt.dx.commands] reference: {chain}")
            if ref not in commands:
                raise RuntimeError(
                    f"Missing [tool.molt.dx.commands].{ref} referenced by {item_name}"
                )
            return _split_command_sequence(
                commands[ref],
                ref,
                commands=commands,
                stack=(*stack, ref),
            )
        return [_split_command(item, item_name)]

    if isinstance(command, str):
        return split_item(command, name)
    if isinstance(command, list) and command:
        split: list[list[str]] = []
        for idx, item in enumerate(command):
            if not isinstance(item, str) or not item.strip():
                raise RuntimeError(
                    f"Invalid [tool.molt.dx.commands].{name}[{idx}]: expected command string"
                )
            split.extend(split_item(item, f"{name}[{idx}]"))
        return split
    raise RuntimeError(f"Missing [tool.molt.dx.commands].{name}")


def _run_repo_cmd(cmd: list[str], env: dict[str, str], *, tty: bool) -> None:
    _log("$ " + " ".join(shlex.quote(part) for part in cmd))
    if tty and os.name == "posix":
        _run_with_pty(cmd, env)
    else:
        subprocess.check_call(cmd, cwd=ROOT, env=env)


def _run_dx_command(name: str, env: dict[str, str], *, tty: bool) -> None:
    command = _dx_commands().get(name)
    _run_repo_cmd(_split_command(command, name), env, tty=tty)


def _require_project_python() -> Path:
    python = _uv_project_python()
    if not python.exists():
        raise RuntimeError(
            f"{python} is missing; run `tools/dev.py install` before compliance gates"
        )
    return python


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
        raise RuntimeError("Unrecognized tools/dev.py gates arguments: " + " ".join(unknown))
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
    status = subprocess.run(
        ["git", "status", "--short"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    if status:
        print(status, end="")
        if not allow_dirty:
            raise RuntimeError("working tree is dirty; rerun with --allow-dirty while developing")
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
        run_uv(["ruff", "check", "."], python=TEST_PYTHONS[0], tty=use_tty)
        run_uv(["ruff", "format", "--check", "."], python=TEST_PYTHONS[0], tty=use_tty)
        run_uv(["ty", "check", "src"], python=TEST_PYTHONS[0], tty=use_tty)
        run_uv(
            ["python3", "tools/verified_subset.py", "check"],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            [
                "python3",
                "tools/check_stdlib_intrinsics.py",
                "--fallback-intrinsic-backed-only",
            ],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            [
                "python3",
                "tools/check_stdlib_intrinsics.py",
                "--critical-allowlist",
            ],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            ["python3", "tools/check_dynamic_policy.py"],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            ["python3", "tools/update_status_blocks.py", "--check"],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            ["python3", "tools/check_docs_architecture.py"],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            [
                "python3",
                "tools/check_core_lane_lowering.py",
                "--manifest",
                "tests/differential/basic/CORE_TESTS.txt",
            ],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
        run_uv(
            ["python3", "-m", "molt.cli", "debug", "verify", "--format", "json"],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
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
    else:
        print(
            "Usage: tools/dev.py "
            "[env|install|clippy|security|compliance|backend|gates|lint|test|setup|doctor|update|validate]"
        )


if __name__ == "__main__":
    main()

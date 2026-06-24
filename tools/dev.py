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
import json
from collections.abc import Mapping
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


def _uv_project_env_matches_python(
    requested: str | None,
    env: dict[str, str] | None = None,
) -> bool:
    project_python = _uv_project_python()
    if not project_python.exists():
        return False
    if not requested:
        return True
    guard_env = _canonical_harness_env(env or os.environ)
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", guard_env)
    try:
        result = harness_memory_guard.guarded_completed_process(
            [
                str(project_python),
                "-c",
                "import sys; print(f'{sys.version_info[0]}.{sys.version_info[1]}')",
            ],
            prefix="MOLT_TEST_SUITE",
            cwd=ROOT,
            env=guard_env,
            capture_output=True,
            text=True,
            limits=limits,
        )
    except OSError:
        return False
    return result.returncode == 0 and result.stdout.strip() == requested


def _normalized_uv_run_env(
    env: dict[str, str],
    *,
    python: str | None,
) -> dict[str, str]:
    project_env_matches_python: bool | None = None
    if env.get("UV_NO_SYNC") == "1":
        project_env_matches_python = _uv_project_env_matches_python(python, env)
    return DX.normalized_uv_run_env(
        env,
        python=python,
        project_env_matches_python=project_env_matches_python,
    )


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
    base_env = _canonical_harness_env(env or os.environ)
    run_env = _normalized_uv_run_env(base_env, python=python)
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", run_env)
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


def _canonical_harness_env(
    env: Mapping[str, str] | None = None,
    *,
    create_dirs: bool = True,
) -> dict[str, str]:
    dx_env = DX.canonical_env(env, create_dirs=create_dirs)
    return harness_memory_guard.canonical_harness_env(dx_env, repo_root=ROOT)


def _canonical_env(*, create_dirs: bool = True) -> dict[str, str]:
    return _canonical_harness_env(create_dirs=create_dirs)


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
    _check_call_guarded(cmd, env, limits=limits)


def _run_dx_command(name: str, env: dict[str, str], *, tty: bool) -> None:
    command = _dx_commands().get(name)
    for split in _split_command_sequence(command, name):
        _run_repo_cmd(split, env, tty=tty)


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
        "CARGO_INCREMENTAL",
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


def _default_gates_summary_path() -> Path:
    return ROOT / "logs" / "dev-gates-summary.json"


def _resolve_gates_summary_path(raw_path: str | None) -> Path:
    if raw_path is None:
        return _default_gates_summary_path()
    path = Path(raw_path).expanduser()
    if not path.is_absolute():
        path = ROOT / path
    return path


def _write_json_sidecar(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        tmp_path.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        os.replace(tmp_path, path)
    except OSError:
        try:
            tmp_path.unlink()
        except OSError:
            pass
        raise


def _parse_gates_args(args: list[str]) -> tuple[bool, Path]:
    allow_dirty = False
    summary_out: str | None = None
    unknown: list[str] = []
    i = 0
    while i < len(args):
        arg = args[i]
        if arg == "--allow-dirty":
            allow_dirty = True
            i += 1
            continue
        if arg == "--summary-out":
            if i + 1 >= len(args):
                raise RuntimeError("--summary-out requires a path")
            summary_out = args[i + 1]
            i += 2
            continue
        unknown.append(arg)
        i += 1
    if unknown:
        raise RuntimeError(
            "Unrecognized tools/dev.py gates arguments: " + " ".join(unknown)
        )
    return allow_dirty, _resolve_gates_summary_path(summary_out)


def _run_dx_gates(args: list[str], *, tty: bool) -> None:
    allow_dirty, summary_path = _parse_gates_args(args)
    env = _canonical_env()
    _require_project_python()
    commands = _dx_commands()
    gates = commands.get("gates")
    if gates is None:
        gate_commands = [
            _split_command(commands.get(name), name)
            for name in ("build", "backend", "compliance")
        ]
    else:
        gate_commands = _split_command_sequence(gates, "gates")
    limits = harness_memory_guard.limits_from_env("MOLT_TEST_SUITE", env)
    memory_guard = {
        "MOLT_TEST_SUITE": harness_memory_guard.limits_summary(limits),
    }
    started_at = datetime.now().astimezone().isoformat()
    started = time.monotonic()
    steps: list[dict[str, object]] = []
    errors: list[str] = []
    git_status_payload: dict[str, object] | None = None

    def write_summary(status: str, *, raise_on_error: bool) -> None:
        finished_at = datetime.now().astimezone().isoformat()
        payload: dict[str, object] = {
            "schema_version": "1.0",
            "command": "tools/dev.py gates",
            "status": status,
            "started_at": started_at,
            "finished_at": finished_at,
            "elapsed_s": round(time.monotonic() - started, 6),
            "summary_path": str(summary_path),
            "allow_dirty": allow_dirty,
            "memory_guard": memory_guard,
            "steps": steps,
            "git_status": git_status_payload,
            "errors": errors,
        }
        try:
            _write_json_sidecar(summary_path, payload)
        except OSError as exc:
            message = f"failed to write dev gates summary {summary_path}: {exc}"
            if raise_on_error:
                raise RuntimeError(message) from exc
            print(message, file=sys.stderr)
            return
        _log(f"gate summary: {summary_path}")

    for index, gate_cmd in enumerate(gate_commands, start=1):
        step_start = time.monotonic()
        entry: dict[str, object] = {
            "index": index,
            "cmd": gate_cmd,
        }
        try:
            _run_repo_cmd(gate_cmd, env, tty=tty)
        except subprocess.CalledProcessError as exc:
            entry["returncode"] = exc.returncode
            entry["duration_s"] = round(time.monotonic() - step_start, 6)
            steps.append(entry)
            errors.append(
                "gate command failed "
                f"(index={index}, returncode={exc.returncode}): "
                + " ".join(shlex.quote(part) for part in gate_cmd)
            )
            write_summary("error", raise_on_error=False)
            raise
        except Exception as exc:
            entry["returncode"] = None
            entry["duration_s"] = round(time.monotonic() - step_start, 6)
            entry["error"] = f"{type(exc).__name__}: {exc}"
            steps.append(entry)
            errors.append(
                f"gate command raised (index={index}): {type(exc).__name__}: {exc}"
            )
            write_summary("error", raise_on_error=False)
            raise
        entry["returncode"] = 0
        entry["duration_s"] = round(time.monotonic() - step_start, 6)
        steps.append(entry)

    status_result = harness_memory_guard.guarded_completed_process(
        ["git", "status", "--short"],
        prefix="MOLT_TEST_SUITE",
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        limits=limits,
    )
    git_status_payload = {
        "cmd": ["git", "status", "--short"],
        "returncode": status_result.returncode,
        "stdout": status_result.stdout,
        "stderr": status_result.stderr,
    }
    if status_result.returncode != 0:
        errors.append(f"git status failed with exit code {status_result.returncode}")
        write_summary("error", raise_on_error=False)
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
            errors.append(
                "working tree is dirty; rerun with --allow-dirty while developing"
            )
            write_summary("error", raise_on_error=False)
            raise RuntimeError(
                "working tree is dirty; rerun with --allow-dirty while developing"
            )
    else:
        _log("git status clean")
    write_summary("ok", raise_on_error=True)


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
    elif cmd[0] == "bench":
        env = _canonical_env()
        project_python = _require_project_python()
        if cmd[1:]:
            _run_repo_cmd(
                [str(project_python), "-m", "molt.cli", "bench", *cmd[1:]],
                env,
                tty=use_tty,
            )
        else:
            _run_dx_command("bench-smoke", env, tty=use_tty)
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
            batch_cmd = ["python", "tools/dev_test_runner.py"]
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
            ["python", "-m", "molt.cli", "doctor", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "setup":
        run_uv(
            ["python", "-m", "molt.cli", "setup", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "update":
        run_uv(
            ["python", "-m", "molt.cli", "update", *cmd[1:]],
            python=TEST_PYTHONS[0],
            tty=use_tty,
        )
    elif cmd[0] == "validate":
        run_uv(
            ["python", "-m", "molt.cli", "validate", *cmd[1:]],
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
            "[env|install|clippy|security|compliance|backend|gates|bench|lint|test|setup|doctor|update|validate|clean-artifacts]"
        )


if __name__ == "__main__":
    main()

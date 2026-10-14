from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from molt.cli.output import build_output_path, json_command_failure, read_json_payload

REPO_ROOT = Path(__file__).resolve().parents[1]


def _harness_memory_guard():
    from tools import harness_memory_guard

    return harness_memory_guard


# ---------------------------------------------------------------------------
# Compilation and execution
# ---------------------------------------------------------------------------


def _repo_root() -> Path:
    return REPO_ROOT


def fuzz_build_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    return env


def run_cpython(source_path: str, timeout: float) -> tuple[str, str, int | None]:
    env = {**os.environ, "PYTHONHASHSEED": "0"}
    guard = _harness_memory_guard()
    limits = guard.limits_from_env("MOLT_TEST_SUITE", env)
    try:
        result = guard.guarded_completed_process(
            [sys.executable, source_path],
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            limits=limits,
        )
        return result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired:
        return "", "", None


def compile_molt(
    source_path: str,
    profile: str,
    timeout: float,
    env: dict[str, str],
) -> tuple[str | None, str]:
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        "--profile",
        profile,
        "--deterministic",
        "--json",
        source_path,
    ]
    guard = _harness_memory_guard()
    limits = guard.limits_from_env("MOLT_TEST_SUITE", env)
    try:
        result = guard.guarded_completed_process(
            cmd,
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            cwd=str(_repo_root()),
            limits=limits,
        )
    except subprocess.TimeoutExpired:
        return None, "Molt build timed out"

    if result.returncode != 0:
        return (
            None,
            f"Molt build failed (rc={result.returncode}): "
            f"{json_command_failure(result.stdout, result.stderr)}",
        )
    try:
        binary = build_output_path(read_json_payload(result.stdout))
    except ValueError as exc:
        return None, f"Molt build output: {exc}"

    if not Path(binary).exists():
        return None, f"Binary not found at {binary}"

    return binary, ""


def run_molt_binary(
    binary_path: str,
    timeout: float,
    env: dict[str, str],
) -> tuple[str, str, int | None]:
    guard = _harness_memory_guard()
    limits = guard.limits_from_env("MOLT_TEST_SUITE", env)
    try:
        result = guard.guarded_completed_process(
            [binary_path],
            prefix="MOLT_TEST_SUITE",
            capture_output=True,
            text=True,
            timeout=timeout,
            env=env,
            limits=limits,
        )
        return result.stdout, result.stderr, result.returncode
    except subprocess.TimeoutExpired:
        return "", "", None

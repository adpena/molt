from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def _harness_memory_guard():
    from tools import harness_memory_guard

    return harness_memory_guard


# ---------------------------------------------------------------------------
# Compilation and execution
# ---------------------------------------------------------------------------


def _repo_root() -> Path:
    return REPO_ROOT


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("PYTHONPATH", "src")
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_DETERMINISTIC"] = "1"
    return env


def _extract_binary(build_json: dict) -> str | None:
    data = build_json
    if "data" in build_json and isinstance(build_json["data"], dict):
        data = build_json["data"]
    for key in ("output", "artifact", "binary", "path", "output_path"):
        if key in data:
            return str(data[key])
    if "build" in data and isinstance(data["build"], dict):
        for key in ("output", "artifact", "binary", "path"):
            if key in data["build"]:
                return str(data["build"][key])
    return None


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
        stderr_snippet = result.stderr[:800] if result.stderr else "(no stderr)"
        stdout_snippet = result.stdout[:400] if result.stdout else "(no stdout)"
        return (
            None,
            f"Molt build failed (rc={result.returncode}):\n"
            f"stderr: {stderr_snippet}\nstdout: {stdout_snippet}",
        )

    stdout = result.stdout.strip()
    if not stdout:
        return None, "Molt build produced no JSON output"

    json_str = None
    for line in reversed(stdout.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            json_str = line
            break

    if json_str is None:
        return None, f"No JSON object in Molt build output: {stdout[:300]}"

    try:
        build_info = json.loads(json_str)
    except json.JSONDecodeError as exc:
        return None, f"Invalid build JSON: {exc}\n{json_str[:300]}"

    binary = _extract_binary(build_info)
    if binary is None:
        data_keys = (
            list(build_info.get("data", {}).keys())
            if isinstance(build_info.get("data"), dict)
            else "N/A"
        )
        return (
            None,
            f"Cannot find binary in build output. "
            f"Keys: {list(build_info.keys())}, data keys: {data_keys}",
        )

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

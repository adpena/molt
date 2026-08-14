"""Shared subprocess execution and timing for Molt CLI command families."""

from __future__ import annotations

import shlex
import sys
import time
from pathlib import Path
from typing import Any
from molt.cli.command_runtime import (
    _run_completed_command,
)
from molt.cli.models import (
    _TimedResult,
)
from molt.cli.output import emit_json as _emit_json
from molt.cli.output import json_payload as _json_payload


def _run_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    json_output: bool = False,
    verbose: bool = False,
    label: str | None = None,
    warnings: list[str] | None = None,
    memory_guard_prefix: str | None = None,
) -> int:
    cmd = [str(part) for part in cmd]
    if verbose and not json_output:
        print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
    capture = json_output
    result = _run_completed_command(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture,
        memory_guard_prefix=memory_guard_prefix,
    )
    if json_output:
        data: dict[str, Any] = {"returncode": result.returncode}
        if result.stdout:
            data["stdout"] = result.stdout
        if result.stderr:
            data["stderr"] = result.stderr
        payload = _json_payload(
            label or cmd[0],
            "ok" if result.returncode == 0 else "error",
            data=data,
            warnings=warnings,
        )
        _emit_json(payload, json_output=True)
    return result.returncode


def _run_command_timed(
    cmd: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
    verbose: bool = False,
    capture_output: bool = False,
    memory_guard_prefix: str | None = None,
) -> _TimedResult:
    cmd = [str(part) for part in cmd]
    if verbose:
        print(f"Running: {shlex.join(cmd)}", file=sys.stderr)
    start = time.perf_counter()
    result = _run_completed_command(
        cmd,
        env=env,
        cwd=cwd,
        capture_output=capture_output,
        memory_guard_prefix=memory_guard_prefix,
    )
    duration = getattr(result, "elapsed_s", None)
    if duration is None:
        duration = time.perf_counter() - start
    return _TimedResult(
        result.returncode,
        result.stdout or "",
        result.stderr or "",
        duration,
    )


def _format_duration(seconds: float) -> str:
    if seconds < 0:
        seconds = 0.0
    if seconds < 0.001:
        return f"{seconds * 1_000_000:.0f} µs"
    if seconds < 1:
        return f"{seconds * 1000:.1f} ms"
    if seconds < 60:
        return f"{seconds:.3f} s"
    return f"{seconds / 60:.2f} min"

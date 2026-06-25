#!/usr/bin/env python3
"""Shared command-measurement schema for throughput tooling.

Build-speed tools need to compare cold, warm, edit, single, concurrent, and
differential phases without each script inventing its own subtly different
JSON shape. This module owns the command result payloads and helpers so timing,
timeout, output-tail, cwd, and optional artifact-size facts stay coherent.
"""

from __future__ import annotations

import time
from dataclasses import dataclass
from pathlib import Path

try:
    from tools.memory_guard import TIMEOUT_RETURN_CODE as TIMEOUT_RETURN_CODE
except ModuleNotFoundError:  # pragma: no cover - direct import from tools/
    from memory_guard import TIMEOUT_RETURN_CODE as TIMEOUT_RETURN_CODE  # type: ignore


TAIL_LINES = 12


@dataclass(frozen=True)
class CommandResult:
    command: list[str]
    returncode: int
    elapsed_sec: float
    timed_out: bool
    stdout_tail: str
    stderr_tail: str
    cwd: str = ""
    output_size_bytes: int | None = None


@dataclass(frozen=True)
class PhaseResult:
    phase: str
    command: list[str]
    cwd: str
    returncode: int
    elapsed_sec: float
    timed_out: bool
    stdout_tail: str
    stderr_tail: str
    output_size_bytes: int | None = None


def tail(text: str, *, lines: int = TAIL_LINES) -> str:
    if not text:
        return ""
    return "\n".join(text.splitlines()[-lines:])


def elapsed_sec(start: float, guard_elapsed_s: float | None = None) -> float:
    elapsed = (
        guard_elapsed_s if guard_elapsed_s is not None else time.perf_counter() - start
    )
    return round(elapsed, 3)


def _output_size_bytes(output_path: Path | None) -> int | None:
    if output_path is None or not output_path.exists():
        return None
    if output_path.is_file():
        return output_path.stat().st_size
    if output_path.is_dir():
        return sum(
            path.stat().st_size for path in output_path.rglob("*") if path.is_file()
        )
    return output_path.stat().st_size


def command_result(
    *,
    command: list[str],
    cwd: Path,
    returncode: int,
    elapsed: float,
    timed_out: bool,
    stdout: str,
    stderr: str,
    output_path: Path | None = None,
) -> CommandResult:
    return CommandResult(
        command=command,
        cwd=str(cwd),
        returncode=TIMEOUT_RETURN_CODE if timed_out else returncode,
        elapsed_sec=elapsed,
        timed_out=timed_out,
        stdout_tail=tail(stdout),
        stderr_tail=tail(stderr),
        output_size_bytes=_output_size_bytes(output_path),
    )


def phase_result(
    *,
    phase: str,
    command: list[str],
    cwd: Path,
    returncode: int,
    elapsed: float,
    timed_out: bool,
    stdout: str,
    stderr: str,
    output_path: Path | None = None,
) -> PhaseResult:
    return PhaseResult(
        phase=phase,
        command=command,
        cwd=str(cwd),
        returncode=TIMEOUT_RETURN_CODE if timed_out else returncode,
        elapsed_sec=elapsed,
        timed_out=timed_out,
        stdout_tail=tail(stdout),
        stderr_tail=tail(stderr),
        output_size_bytes=_output_size_bytes(output_path),
    )

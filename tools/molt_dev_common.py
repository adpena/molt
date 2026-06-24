#!/usr/bin/env python3
"""Shared primitives for the canonical Molt dev driver."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

import harness_memory_guard

DEFAULT_REPO = Path(__file__).resolve().parents[1]

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2


def _say(msg: str) -> None:
    """A status line -> stderr (so stdout stays clean for machine consumers)."""
    print(msg, file=sys.stderr, flush=True)


def _step(name: str) -> None:
    _say(f"==> {name}")


def _ok(msg: str) -> None:
    _say(f"    OK: {msg}")


def _fail(msg: str) -> None:
    _say(f"    FAIL: {msg}")


def _warn(msg: str) -> None:
    _say(f"    WARN: {msg}")


class DriverError(Exception):
    """A loud, fatal driver condition. Carries an exit code (default FAIL)."""

    def __init__(self, message: str, code: int = EXIT_FAIL):
        super().__init__(message)
        self.code = code


def _run_driver_command(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    input_text: str | None = None,
    timeout: float | None = 60.0,
    prefix: str = "MOLT_DEV",
) -> subprocess.CompletedProcess[str]:
    """Run one bounded captured driver command through the shared guard."""
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd or DEFAULT_REPO,
        env=env,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _run_driver_command_bytes(
    cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    timeout: float,
    prefix: str,
) -> subprocess.CompletedProcess[bytes]:
    """Run one bounded captured driver command with byte-exact output custody."""
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=False,
        timeout=timeout,
    )


def _run_live_gate(cmd: str, *, repo: Path, env: dict[str, str]) -> int:
    """Run a manifest gate live through memory custody while preserving shell syntax."""
    proc = harness_memory_guard.guarded_completed_process(
        ["/bin/sh", "-c", cmd],
        prefix="MOLT_TEST_SUITE",
        cwd=repo,
        env=env,
        capture_output=False,
        text=True,
        timeout=None,
    )
    return proc.returncode

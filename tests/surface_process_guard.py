from __future__ import annotations

from collections.abc import Mapping, Sequence
from pathlib import Path

from tools import harness_memory_guard
from tests import process_guard_common

DEFAULT_SURFACE_TEST_TIMEOUT_SEC = process_guard_common.DEFAULT_TEST_PROCESS_TIMEOUT_SEC


def run_surface_test_process(
    args: Sequence[str],
    *,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    input: str | None = None,
) -> harness_memory_guard.GuardedCompletedProcess:
    return process_guard_common.run_guarded_test_process(
        args,
        prefix="MOLT_SURFACE_TEST",
        cwd=cwd,
        env=env,
        timeout=timeout,
        default_timeout=DEFAULT_SURFACE_TEST_TIMEOUT_SEC,
        capture_output=capture_output,
        text=text,
        check=check,
        input=input,
    )

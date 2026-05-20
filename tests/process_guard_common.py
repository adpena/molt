from __future__ import annotations

import os
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path

from tools import harness_memory_guard

DEFAULT_TEST_PROCESS_TIMEOUT_SEC = 300.0


def run_guarded_test_process(
    args: Sequence[str],
    *,
    prefix: str,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    default_timeout: float | None = DEFAULT_TEST_PROCESS_TIMEOUT_SEC,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    input: str | None = None,
) -> harness_memory_guard.GuardedCompletedProcess:
    command = list(args)
    process_env = os.environ if env is None else env
    resolved_timeout = harness_memory_guard.timeout_from_env(
        prefix,
        process_env,
        explicit=timeout,
        default=default_timeout,
    )
    result = harness_memory_guard.guarded_completed_process(
        command,
        prefix=prefix,
        cwd=cwd,
        env=process_env,
        input=input,
        capture_output=capture_output,
        text=text,
        timeout=resolved_timeout,
    )
    if (
        resolved_timeout is not None
        and result.returncode == harness_memory_guard.memory_guard.TIMEOUT_RETURN_CODE
        and "memory_guard: timeout after" in (result.stderr or "")
    ):
        raise subprocess.TimeoutExpired(
            command,
            resolved_timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
    if check and result.returncode != 0:
        raise subprocess.CalledProcessError(
            result.returncode,
            command,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result

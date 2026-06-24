from __future__ import annotations

import subprocess
from pathlib import Path
from typing import Any

from molt import process_guard as _process_guard

_CLI_MEMORY_GUARD_PREFIX = _process_guard.CLI_MEMORY_GUARD_PREFIX
_CROSS_MEMORY_GUARD_PREFIX = "MOLT_CROSS"
_DIFF_MEMORY_GUARD_PREFIX = "MOLT_DIFF"


def _load_cli_harness_memory_guard(cwd: Path | None) -> Any:
    return _process_guard.load_harness_memory_guard(cwd)


def _with_memory_guard_env(
    env: dict[str, str] | None,
    memory_guard_prefix: str,
) -> dict[str, str] | None:
    return _process_guard.with_memory_guard_env(env, memory_guard_prefix)


def _run_completed_command(
    cmd: list[str],
    *,
    env: dict[str, str] | None,
    cwd: Path | None,
    capture_output: bool,
    memory_guard_prefix: str | None,
    input: str | None = None,
    timeout: float | None = None,
) -> subprocess.CompletedProcess[str]:
    guard_env = (
        None
        if memory_guard_prefix is None
        else _with_memory_guard_env(env, memory_guard_prefix)
    )
    if memory_guard_prefix is None:
        return subprocess.run(
            cmd,
            env=env,
            cwd=cwd,
            input=input,
            capture_output=capture_output,
            text=True,
            timeout=timeout,
        )
    return _process_guard.run_completed_command(
        cmd,
        env=guard_env,
        cwd=cwd,
        capture_output=capture_output,
        memory_guard_prefix=memory_guard_prefix,
        input=input,
        timeout=timeout,
        guard_loader=_load_cli_harness_memory_guard,
    )

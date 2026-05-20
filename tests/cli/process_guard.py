from __future__ import annotations

import os
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from tools import harness_memory_guard
from tests import process_guard_common

DEFAULT_CLI_TEST_TIMEOUT_SEC = process_guard_common.DEFAULT_TEST_PROCESS_TIMEOUT_SEC


def run_cli_test_process(
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
        prefix="MOLT_CLI_TEST",
        cwd=cwd,
        env=env,
        timeout=timeout,
        default_timeout=DEFAULT_CLI_TEST_TIMEOUT_SEC,
        capture_output=capture_output,
        text=text,
        check=check,
        input=input,
    )


def cli_test_popen_kwargs(env: Mapping[str, str] | None = None) -> dict[str, Any]:
    limits = harness_memory_guard.limits_from_env(
        "MOLT_CLI_TEST",
        os.environ if env is None else env,
    )
    return harness_memory_guard.batch_process_group_kwargs(limits)


def close_cli_test_process_group(proc: subprocess.Popen[str]) -> None:
    harness_memory_guard.force_close_process_group(proc)

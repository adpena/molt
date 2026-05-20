from __future__ import annotations

from collections.abc import Mapping, Sequence

from tools import harness_memory_guard
from tests import process_guard_common

DEFAULT_MUTATION_TIMEOUT_SEC = process_guard_common.DEFAULT_TEST_PROCESS_TIMEOUT_SEC


def run_mutation_process(
    args: Sequence[str],
    *,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
) -> harness_memory_guard.GuardedCompletedProcess:
    return process_guard_common.run_guarded_test_process(
        args,
        prefix="MOLT_MUTATION",
        env=env,
        timeout=timeout,
        default_timeout=DEFAULT_MUTATION_TIMEOUT_SEC,
        capture_output=capture_output,
        text=text,
        check=check,
    )

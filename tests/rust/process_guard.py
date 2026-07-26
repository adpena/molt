from __future__ import annotations

from collections.abc import Mapping, Sequence

from tools import harness_memory_guard
from tests import process_guard_common


def run_rust_test_process(
    args: Sequence[str],
    *,
    cwd: str | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
) -> harness_memory_guard.GuardedCompletedProcess:
    return process_guard_common.run_guarded_test_process(
        args,
        prefix="MOLT_RUST_TEST",
        cwd=cwd,
        env=env,
        timeout=timeout,
        capture_output=capture_output,
        text=text,
        check=check,
    )

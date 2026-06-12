from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

from tools import harness_memory_guard
from tests import process_guard_common

ROOT = Path(__file__).resolve().parents[2]
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


def guarded_cli_test_popen(
    args: Sequence[str],
    *,
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    stdout: int | None = subprocess.PIPE,
    stderr: int | None = subprocess.PIPE,
    text: bool = True,
) -> subprocess.Popen[str]:
    run_env = harness_memory_guard.canonical_harness_env(env, repo_root=ROOT)
    limits = harness_memory_guard.limits_from_env("MOLT_CLI_TEST", run_env)
    summary_dir = ROOT / "tmp" / "cli-test-memory-guard"
    summary_dir.mkdir(parents=True, exist_ok=True)
    summary_path = summary_dir / f"interactive-{os.getpid()}.json"
    guard_argv = [
        sys.executable,
        str(ROOT / "tools" / "memory_guard.py"),
        "--max-rss-gb",
        str(limits.max_process_rss_gb),
        "--max-total-rss-gb",
        str(limits.max_total_rss_gb),
        "--poll-interval",
        str(limits.poll_interval),
        "--child-rlimit-gb",
        str(0 if limits.child_rlimit_gb is None else limits.child_rlimit_gb),
        "--summary-json",
        str(summary_path),
        "--",
        *args,
    ]
    return subprocess.Popen(
        guard_argv,
        cwd=cwd,
        env=run_env,
        stdout=stdout,
        stderr=stderr,
        text=text,
        **harness_memory_guard.batch_process_group_kwargs(limits, env=run_env),
    )


def close_cli_test_process_group(proc: subprocess.Popen[str]) -> None:
    harness_memory_guard.force_close_process_group(proc)

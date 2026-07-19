from __future__ import annotations

import os
import json
import shutil
import subprocess
import tempfile
from contextlib import contextmanager
from collections.abc import Mapping, Sequence
from pathlib import Path
from collections.abc import Callable, Iterator

from tools import harness_memory_guard

DEFAULT_TEST_PROCESS_TIMEOUT_SEC = 300.0


def _diagnostic_value(result: object, name: str) -> object:
    value = getattr(result, name, None)
    if hasattr(value, "to_dict"):
        return value.to_dict()
    return value


def _timeout_receipt(result: object) -> str:
    """Return bounded custody evidence without ever masking the timeout."""

    try:
        stderr = getattr(result, "stderr", "") or ""
        if isinstance(stderr, bytes):
            stderr = stderr.decode("utf-8", errors="replace")
        payload = {
            "schema": "molt.test-process-timeout.v1",
            "timed_out": _diagnostic_value(result, "timed_out"),
            "child_process": _diagnostic_value(result, "child_process"),
            "termination_reports": _diagnostic_value(result, "termination_reports"),
            "orphaned_process_groups": _diagnostic_value(
                result, "orphaned_process_groups"
            ),
            "peak": _diagnostic_value(result, "peak"),
            "peak_total": _diagnostic_value(result, "peak_total"),
            "stderr_tail": stderr[-4000:],
        }
        return json.dumps(payload, default=str, sort_keys=True)
    except BaseException as error:
        return json.dumps(
            {
                "schema": "molt.test-process-timeout.v1",
                "diagnostic_error": f"{type(error).__name__}: {error}",
            },
            sort_keys=True,
        )


@contextmanager
def preserve_primary_during_cleanup(
    cleanup: Callable[[], object], *, label: str
) -> Iterator[None]:
    """Run cleanup while preserving and annotating an in-flight primary error."""

    try:
        yield
    except BaseException as primary:
        try:
            cleanup()
        except BaseException as cleanup_error:
            primary.add_note(
                json.dumps(
                    {
                        "schema": "molt.test-process-cleanup.v1",
                        "label": label,
                        "cleanup_error": (
                            f"{type(cleanup_error).__name__}: {cleanup_error}"
                        ),
                    },
                    sort_keys=True,
                )
            )
        raise
    else:
        cleanup()


@contextmanager
def guarded_temporary_directory(
    *, prefix: str, dir: str | Path | None = None
) -> Iterator[Path]:
    """Own scratch used by guarded children without masking their failures."""

    path = Path(tempfile.mkdtemp(prefix=prefix, dir=dir))
    with preserve_primary_during_cleanup(
        lambda: shutil.rmtree(path),
        label=str(path),
    ):
        yield path


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
        error = subprocess.TimeoutExpired(
            command,
            resolved_timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
        error.add_note(_timeout_receipt(result))
        raise error
    if check and result.returncode != 0:
        raise subprocess.CalledProcessError(
            result.returncode,
            command,
            output=result.stdout,
            stderr=result.stderr,
        )
    return result

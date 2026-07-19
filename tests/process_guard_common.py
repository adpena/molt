from __future__ import annotations

import os
import json
import shutil
import subprocess
import tempfile
from enum import Enum
from contextlib import contextmanager
from collections.abc import Mapping, Sequence
from pathlib import Path
from collections.abc import Callable, Iterator
from typing import Any

from tools import harness_memory_guard

DEFAULT_TEST_PROCESS_TIMEOUT_SEC = 300.0
DEFAULT_BUILD_PROCESS_TIMEOUT_SEC = 900.0


class GuardedProcessRole(str, Enum):
    """Orthogonal operation role for a suite-family process guard."""

    BUILD = "build"
    EXECUTION = "execution"


_ALWAYS_BUILD_DRIVERS = frozenset(
    {
        "c++",
        "cc",
        "cl",
        "clang",
        "clang++",
        "gcc",
        "g++",
        "ld",
        "ld.lld",
        "link",
        "lld",
        "lld-link",
        "make",
        "gmake",
        "msbuild",
        "ninja",
        "rustc",
        "wasm-ld",
        "xcodebuild",
    }
)
_BUILD_SUBCOMMAND_DRIVERS = frozenset(
    {"cargo", "dotnet", "go", "molt", "npm", "pnpm", "swift", "yarn", "zig"}
)
_BUILD_FLAG_DRIVERS = frozenset({"cmake"})
_METADATA_PROBE_FLAGS = frozenset(
    {
        "--help",
        "--version",
        "-V",
        "-dumpmachine",
        "-dumpversion",
        "-h",
        "-v",
        "-vV",
        "/?",
        "/Bv",
    }
)


def _compiler_command_is_metadata_probe(
    executable: str, arguments: tuple[str, ...]
) -> bool:
    if not arguments:
        return True
    first = arguments[0]
    if first in _METADATA_PROBE_FLAGS or first.startswith("-print-"):
        return True
    return executable == "rustc" and first == "--print" and len(arguments) <= 2


def guarded_process_role(args: Sequence[str]) -> GuardedProcessRole:
    """Classify build/compiler work independently of its test-suite family.

    Classification happens on the realized command at the launch boundary, so
    import aliases and commands assembled in variables cannot bypass it.
    """

    command = tuple(str(arg) for arg in args)
    if not command:
        return GuardedProcessRole.EXECUTION
    executable = Path(command[0]).name.lower()
    if executable.endswith(".exe"):
        executable = executable[:-4]
    arguments = command[1:]
    if executable in _ALWAYS_BUILD_DRIVERS and not _compiler_command_is_metadata_probe(
        executable, arguments
    ):
        return GuardedProcessRole.BUILD
    if (
        executable in _BUILD_SUBCOMMAND_DRIVERS
        and len(command) >= 2
        and command[1] == "build"
    ):
        return GuardedProcessRole.BUILD
    if (
        executable in _BUILD_FLAG_DRIVERS
        and len(command) >= 2
        and command[1] == "--build"
    ):
        return GuardedProcessRole.BUILD
    if any(
        current in {"molt", "molt.cli"} and following == "build"
        for current, following in zip(command, command[1:], strict=False)
    ):
        return GuardedProcessRole.BUILD
    return GuardedProcessRole.EXECUTION


def _role_prefix(prefix: str, role: GuardedProcessRole) -> str:
    return f"{prefix}_{role.value.upper()}"


def _timeout_from_role_env(
    prefix: str,
    role: GuardedProcessRole,
    env: Mapping[str, str],
    *,
    explicit: float | None,
    default: float | None,
) -> float | None:
    if explicit is not None:
        return explicit
    role_default = (
        DEFAULT_BUILD_PROCESS_TIMEOUT_SEC
        if role is GuardedProcessRole.BUILD
        else default
    )
    role_prefix = _role_prefix(prefix, role)
    role_env = f"MOLT_{role.value.upper()}_TIMEOUT_SEC"
    # Avoid the generic fallback inside timeout_from_env until the two
    # compositional authorities have had precedence.
    for name in (f"{role_prefix}_TIMEOUT_SEC", f"{prefix}_TIMEOUT_SEC", role_env):
        raw = env.get(name)
        if raw is None or not raw.strip():
            continue
        return harness_memory_guard.timeout_from_env(
            role_prefix,
            {f"{role_prefix}_TIMEOUT_SEC": raw},
            default=role_default,
        )
    return harness_memory_guard.timeout_from_env(
        role_prefix,
        env,
        default=role_default,
    )


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
    prefix: str = "MOLT_PYTEST",
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    timeout: float | None = None,
    default_timeout: float | None = DEFAULT_TEST_PROCESS_TIMEOUT_SEC,
    capture_output: bool = True,
    text: bool = True,
    check: bool = False,
    input: str | bytes | None = None,
    stdout: int | None = None,
    stderr: int | None = None,
    encoding: str = "utf-8",
    errors: str = "replace",
) -> harness_memory_guard.GuardedCompletedProcess:
    supported_streams = {None, subprocess.PIPE, subprocess.DEVNULL}
    if stdout not in supported_streams:
        raise ValueError("guarded test stdout must inherit, PIPE, or DEVNULL")
    if stderr not in supported_streams | {subprocess.STDOUT}:
        raise ValueError("guarded test stderr must inherit, PIPE, DEVNULL, or STDOUT")
    if capture_output and (stdout is not None or stderr is not None):
        raise ValueError("capture_output cannot be combined with stdout or stderr")
    if stderr == subprocess.STDOUT:
        raise ValueError(
            "guarded test commands preserve stdout/stderr separately; use an "
            "explicitly owned streaming process when interleaving is required"
        )
    command = list(args)
    process_env = os.environ if env is None else env
    role = guarded_process_role(command)
    resolved_timeout = _timeout_from_role_env(
        prefix,
        role,
        process_env,
        explicit=timeout,
        default=default_timeout,
    )
    capture_streams = capture_output or stdout is not None or stderr is not None
    result = harness_memory_guard.guarded_completed_process(
        command,
        prefix=prefix,
        operation_role=role.value,
        cwd=cwd,
        env=process_env,
        input=input,
        capture_output=capture_streams,
        text=text,
        timeout=resolved_timeout,
        encoding=encoding,
        errors=errors,
    )
    if stderr == subprocess.DEVNULL:
        result.stderr = None
    if stdout == subprocess.DEVNULL:
        result.stdout = None
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


def check_output_guarded_test_process(
    args: Sequence[str],
    **kwargs: object,
) -> str | bytes:
    """Return stdout from the canonical guarded test execution boundary."""

    if "stdout" in kwargs:
        raise ValueError("stdout is owned by check_output_guarded_test_process")
    result = run_guarded_test_process(
        args,
        capture_output=False,
        stdout=subprocess.PIPE,
        check=True,
        **kwargs,
    )
    assert result.stdout is not None
    return result.stdout


def start_owned_test_process(
    args: Sequence[str],
    *,
    prefix: str = "MOLT_PYTEST",
    cwd: str | Path | None = None,
    env: Mapping[str, str] | None = None,
    stdin: int | None = None,
    stdout: int | None = None,
    stderr: int | None = None,
    text: bool = False,
    encoding: str | None = None,
    errors: str | None = None,
    creationflags: int = 0,
) -> subprocess.Popen[Any]:
    """Start one caller-owned direct child with platform process-group custody."""

    command = list(args)
    run_env = harness_memory_guard.canonical_harness_env(env)
    limits = harness_memory_guard.limits_from_env(prefix, run_env)
    group_kwargs = harness_memory_guard.batch_process_group_kwargs(
        limits,
        env=run_env,
    )
    if creationflags:
        group_kwargs["creationflags"] = int(group_kwargs.get("creationflags", 0)) | int(
            creationflags
        )
    return subprocess.Popen(
        command,
        cwd=cwd,
        env=run_env,
        stdin=stdin,
        stdout=stdout,
        stderr=stderr,
        text=text,
        encoding=encoding,
        errors=errors,
        **group_kwargs,
    )


def close_owned_test_process(process: subprocess.Popen[Any]) -> None:
    """Close exactly the process group created by start_owned_test_process."""

    if process.poll() is None:
        harness_memory_guard.force_close_process_group(process)
    process.wait()

from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any

from .cargo_execution_policy import cargo_subprocess_environment


CLI_MEMORY_GUARD_PREFIX = "MOLT_CLI"
DEFAULT_UNGUARDED_PROBE_TIMEOUT_SECONDS = 30.0

_MEMORY_GUARD_ENV_SUFFIXES = (
    "MEMORY_GUARD",
    "MEMORY_GUARD_POLL_SEC",
    "MAX_PROCESS_RSS_GB",
    "MAX_RSS_GB",
    "MAX_TOTAL_RSS_GB",
    "MAX_TREE_RSS_GB",
    "GLOBAL_RSS_LIMIT_GB",
    "MAX_GLOBAL_RSS_GB",
    "CHILD_RLIMIT_GB",
    "MAX_CHILD_RLIMIT_GB",
    "TOTAL_MEMORY_GB",
    "MEMORY_TOTAL_GB",
    "MEM_AVAILABLE_GB",
    "MEMORY_AVAILABLE_GB",
    "MEMORY_RESERVE_GB",
    "MEM_RESERVE_GB",
)

GuardLoader = Callable[[Path | None], Any]


def _molt_repo_root() -> Path:
    root = Path(__file__).resolve().parents[2]
    if (
        not (root / "pyproject.toml").is_file()
        or not (root / "tools" / "harness_memory_guard.py").is_file()
    ):
        raise RuntimeError(
            "guarded execution requires the Molt source checkout owning "
            f"{__file__}; its repository guard tools are unavailable"
        )
    return root


def load_harness_memory_guard(cwd: Path | None) -> Any:
    del cwd  # A command directory cannot authorize a different guard module.
    root = _molt_repo_root()
    for path in (root, root / "tools"):
        if str(path) not in sys.path:
            sys.path.insert(0, str(path))
    try:
        from tools import harness_memory_guard
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            f"memory guard helper is required for guarded subprocesses: {exc}"
        ) from exc
    # Never replace a live foreign guard: it may own processes and suppression
    # state. Reject the mixed source authority before any child is launched.
    for module, relative in (
        (harness_memory_guard, "harness_memory_guard.py"),
        (getattr(harness_memory_guard, "memory_guard", None), "memory_guard.py"),
        (
            getattr(harness_memory_guard, "process_sentinel", None),
            "process_sentinel.py",
        ),
    ):
        actual = getattr(module, "__file__", None)
        expected = root / "tools" / relative
        if actual is None or Path(actual).resolve() != expected.resolve():
            raise RuntimeError(
                f"guard source authority mismatch: expected {expected}, loaded {actual!r}; "
                "use a fresh process bound to one Molt checkout"
            )
    return harness_memory_guard


def with_memory_guard_env(
    env: Mapping[str, str] | None,
    memory_guard_prefix: str,
) -> dict[str, str] | None:
    if env is None:
        return None
    merged = dict(env)
    normalized = memory_guard_prefix.strip().upper().rstrip("_")
    names: list[str] = []
    if normalized:
        names.extend(f"{normalized}_{suffix}" for suffix in _MEMORY_GUARD_ENV_SUFFIXES)
    names.extend(f"MOLT_{suffix}" for suffix in _MEMORY_GUARD_ENV_SUFFIXES)
    for name in dict.fromkeys(names):
        if name not in merged and name in os.environ:
            merged[name] = os.environ[name]
    return merged


def timeout_from_env(
    memory_guard_prefix: str,
    env: Mapping[str, str] | None,
    *,
    explicit: float | None = None,
    default: float | None = None,
    guard_loader: GuardLoader = load_harness_memory_guard,
    cwd: Path | None = None,
) -> float | None:
    harness_memory_guard = guard_loader(cwd)
    return harness_memory_guard.timeout_from_env(
        memory_guard_prefix,
        env,
        explicit=explicit,
        default=default,
    )


def run_completed_command(
    cmd: Sequence[str],
    *,
    env: Mapping[str, str] | None = None,
    cwd: str | Path | None = None,
    capture_output: bool = False,
    stdout_capture_path: str | Path | None = None,
    stderr_capture_path: str | Path | None = None,
    capture_tail_bytes: int | None = None,
    memory_guard_prefix: str | None = CLI_MEMORY_GUARD_PREFIX,
    input: str | bytes | None = None,
    timeout: float | None = None,
    text: bool | None = True,
    check: bool = False,
    stdout: int | None = None,
    stderr: int | None = None,
    encoding: str | None = None,
    errors: str | None = None,
    guard_loader: GuardLoader = load_harness_memory_guard,
) -> subprocess.CompletedProcess[Any]:
    if isinstance(cmd, (str, bytes)):
        raise TypeError("command must be typed argv, not shell text")
    command = [str(part) for part in cmd]
    if not command:
        raise ValueError("command argv must not be empty")
    env, _cargo_policies = cargo_subprocess_environment(command, env)
    if capture_output and (stdout is not None or stderr is not None):
        raise ValueError("capture_output cannot be combined with stdout or stderr")
    supported_streams = {None, subprocess.PIPE, subprocess.DEVNULL}
    if stdout not in supported_streams:
        raise ValueError("stdout must inherit, PIPE, or DEVNULL")
    if stderr not in supported_streams | {subprocess.STDOUT}:
        raise ValueError("stderr must inherit, PIPE, DEVNULL, or STDOUT")
    text_mode = bool(text or encoding is not None or errors is not None)
    if memory_guard_prefix is None:
        if stdout_capture_path is not None or stderr_capture_path is not None:
            raise ValueError("evidence capture paths require guarded execution")
        probe_timeout = (
            DEFAULT_UNGUARDED_PROBE_TIMEOUT_SECONDS if timeout is None else timeout
        )
        return subprocess.run(
            command,
            env=dict(env) if env is not None else None,
            cwd=cwd,
            input=input,
            capture_output=capture_output,
            text=text_mode,
            timeout=probe_timeout,
            check=check,
            stdout=stdout,
            stderr=stderr,
            encoding=encoding,
            errors=errors,
        )
    if stderr == subprocess.STDOUT:
        raise ValueError(
            "guarded completed commands preserve stdout/stderr separately; "
            "use an explicitly owned streaming process when interleaving is required"
        )
    guard_env = with_memory_guard_env(env, memory_guard_prefix)
    cwd_path = None if cwd is None else Path(cwd)
    harness_memory_guard = guard_loader(cwd_path)
    guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        memory_guard_prefix,
        guard_env,
        # The command's working directory is not its Molt source authority:
        # standalone Cargo workspaces and staged packages execute below or
        # outside the checkout while retaining this loaded guard's owner.
        repo_root=_molt_repo_root(),
    )
    capture_streams = capture_output or stdout is not None or stderr is not None
    result = guard_context.run(
        command,
        cwd=cwd,
        input=input,  # type: ignore[arg-type]
        capture_output=capture_streams,
        stdout_capture_path=stdout_capture_path,
        stderr_capture_path=stderr_capture_path,
        capture_tail_bytes=capture_tail_bytes,
        text=text_mode,
        timeout=timeout,
        encoding=encoding or "utf-8",
        errors=errors or "strict",
    )
    if stderr == subprocess.DEVNULL:
        result.stderr = None
    if stdout == subprocess.DEVNULL:
        result.stdout = None
    if bool(getattr(result, "timed_out", False)):
        if timeout is None:
            raise RuntimeError(
                "guarded subprocess reported a timeout without a requested "
                "timeout; timeout custody is inconsistent"
            )
        error = subprocess.TimeoutExpired(
            command,
            timeout,
            output=result.stdout,
            stderr=result.stderr,
        )
        # Preserve the canonical guard result for callers that need terminal
        # telemetry (notably per-binary Cargo test receipts).  TimeoutExpired
        # keeps the subprocess-compatible boundary while this attachment avoids
        # discarding the guard's RSS samples and exact terminal record.
        setattr(error, "guarded_result", result)
        raise error
    if check and result.returncode != 0:
        error = subprocess.CalledProcessError(
            result.returncode,
            command,
            output=result.stdout,
            stderr=result.stderr,
        )
        # Match TimeoutExpired above: subprocess compatibility remains intact,
        # while callers retain the guard's child and infrastructure outcomes.
        setattr(error, "guarded_result", result)
        raise error
    return result

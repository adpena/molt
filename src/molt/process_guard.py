from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.cargo_execution_policy import cargo_subprocess_environment


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
    return Path(__file__).resolve().parents[2]


def load_harness_memory_guard(cwd: Path | None) -> Any:
    roots = [_molt_repo_root()]
    if cwd is not None:
        roots.append(cwd.resolve())
    roots.append(Path.cwd().resolve())
    seen: set[Path] = set()
    for root in reversed(roots):
        if root in seen:
            continue
        seen.add(root)
        root_str = str(root)
        tools_str = str(root / "tools")
        if root_str not in sys.path:
            sys.path.insert(0, root_str)
        if tools_str not in sys.path:
            sys.path.insert(0, tools_str)
    try:
        from tools import harness_memory_guard
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            f"memory guard helper is required for guarded subprocesses: {exc}"
        ) from exc
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
        repo_root=(cwd_path or Path.cwd()),
    )
    capture_streams = capture_output or stdout is not None or stderr is not None
    result = guard_context.run(
        command,
        cwd=cwd,
        input=input,  # type: ignore[arg-type]
        capture_output=capture_streams,
        text=text_mode,
        timeout=timeout,
        encoding=encoding or "utf-8",
        errors=errors or "strict",
    )
    if stderr == subprocess.DEVNULL:
        result.stderr = None
    if stdout == subprocess.DEVNULL:
        result.stdout = None
    if result.timed_out:
        if timeout is None:
            raise RuntimeError(
                "guarded subprocess reported a timeout without a requested "
                "timeout; timeout custody is inconsistent"
            )
        raise subprocess.TimeoutExpired(
            command,
            timeout,
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

from __future__ import annotations

import os
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path
from typing import Any


CLI_MEMORY_GUARD_PREFIX = "MOLT_CLI"

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
    env: Mapping[str, str] | None,
    cwd: Path | None,
    capture_output: bool,
    memory_guard_prefix: str | None,
    timeout: float | None = None,
    guard_loader: GuardLoader = load_harness_memory_guard,
) -> subprocess.CompletedProcess[str]:
    command = [str(part) for part in cmd]
    if memory_guard_prefix is None:
        return subprocess.run(
            command,
            env=dict(env) if env is not None else None,
            cwd=cwd,
            capture_output=capture_output,
            text=True,
            timeout=timeout,
        )
    guard_env = with_memory_guard_env(env, memory_guard_prefix)
    harness_memory_guard = guard_loader(cwd)
    guard_context = harness_memory_guard.HarnessExecutionContext.from_env(
        memory_guard_prefix,
        guard_env,
        repo_root=(cwd or Path.cwd()),
    )
    return guard_context.run(
        command,
        cwd=cwd,
        capture_output=capture_output,
        text=True,
        timeout=timeout,
    )

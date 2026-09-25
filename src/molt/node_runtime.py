"""Read-only Node selection for Molt's direct WASM host consumers."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import os
from pathlib import Path
import subprocess

from molt import process_guard
from molt.source_root import compiler_source_root
from molt.tool_releases import pinned_executable
from molt.toolchain_identity import resolve_executable


MIN_NODE_MAJOR = 18


class NodeRuntimeError(RuntimeError):
    pass


@dataclass(frozen=True, slots=True)
class NodeRuntime:
    path: Path
    version: str
    major: int


def _probe_node(
    path: Path,
    *,
    source_root: Path,
    environment: Mapping[str, str],
    guard_prefix: str,
) -> NodeRuntime:
    try:
        result = process_guard.run_completed_command(
            [str(path), "-p", "process.versions.node"],
            env=environment,
            cwd=source_root,
            capture_output=True,
            timeout=10,
            memory_guard_prefix=guard_prefix,
        )
    except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as exc:
        raise NodeRuntimeError(f"Node probe failed for {path}: {exc}") from exc
    if result.returncode != 0 or not isinstance(result.stdout, str):
        detail = result.stderr.strip() if isinstance(result.stderr, str) else ""
        raise NodeRuntimeError(
            f"Node probe failed for {path} (exit {result.returncode}): {detail}"
        )
    version = result.stdout.strip()
    parts = version.split(".")
    if len(parts) < 2 or not all(part.isdigit() for part in parts[:2]):
        raise NodeRuntimeError(
            f"Node probe returned invalid version for {path}: {version!r}"
        )
    major = int(parts[0])
    if major < MIN_NODE_MAJOR:
        raise NodeRuntimeError(
            f"Node >= {MIN_NODE_MAJOR} is required; {path} reports {version}"
        )
    return NodeRuntime(path=path, version=version, major=major)


def resolve_node_runtime(
    *,
    source_root: Path | None = None,
    environment: Mapping[str, str] | None = None,
    guard_prefix: str = "MOLT_CROSS",
) -> NodeRuntime:
    """Select explicit, attested pinned, or PATH-selected Node.

    An invalid explicit selection never falls back. The provisioned release is
    preferred over host PATH, with no stale
    process-global cache or an installation side effect.
    """

    env = os.environ if environment is None else environment
    try:
        root = compiler_source_root() if source_root is None else source_root
    except (OSError, RuntimeError, ValueError) as exc:
        raise NodeRuntimeError(
            f"compiler source Node custody is unavailable: {exc}"
        ) from exc
    requested = env.get("MOLT_NODE_BIN", "").strip()
    if requested:
        try:
            path = resolve_executable(requested, environment=env, label="MOLT_NODE_BIN")
        except (OSError, ValueError) as exc:
            raise NodeRuntimeError(f"MOLT_NODE_BIN is invalid: {exc}") from exc
        return _probe_node(
            path, source_root=root, environment=env, guard_prefix=guard_prefix
        )

    try:
        pinned = pinned_executable("node", root)
    except (OSError, RuntimeError, ValueError) as exc:
        raise NodeRuntimeError(f"pinned Node custody is invalid: {exc}") from exc
    if pinned is not None:
        return _probe_node(
            pinned, source_root=root, environment=env, guard_prefix=guard_prefix
        )

    try:
        path = resolve_executable("node", environment=env, label="Node host")
    except (OSError, ValueError) as exc:
        raise NodeRuntimeError(
            f"Node >= {MIN_NODE_MAJOR} is unavailable on PATH; "
            "install Node or set MOLT_NODE_BIN"
        ) from exc
    return _probe_node(
        path, source_root=root, environment=env, guard_prefix=guard_prefix
    )

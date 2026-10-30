"""Read-only Node selection for Molt's direct WASM host consumers."""

from __future__ import annotations

from molt.process_environment import ProcessEnvironment

from collections.abc import Mapping
from dataclasses import dataclass
import os
from pathlib import Path
import subprocess

from molt import process_guard
from molt.guard_stems_generated import GuardStem
from molt.source_root import compiler_source_root
from molt.tool_releases import pinned_executable
from molt.toolchain_identity import (
    StableRegularFileVersion,
    executable_content_identity,
    resolve_executable,
    stable_regular_file_version,
    verify_stable_regular_file_identity,
    verify_stable_regular_file_metadata,
)


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
    environment: ProcessEnvironment,
    guard_prefix: GuardStem,
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


def _select_node_path(
    *,
    source_root: Path | None = None,
    environment: Mapping[str, str] | None = None,
) -> tuple[Path, Path]:
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
        return root, path

    try:
        pinned = pinned_executable("node", root, environ=env)
    except (OSError, RuntimeError, ValueError) as exc:
        raise NodeRuntimeError(f"pinned Node custody is invalid: {exc}") from exc
    if pinned is not None:
        return root, pinned

    try:
        path = resolve_executable("node", environment=env, label="Node host")
    except (OSError, ValueError) as exc:
        raise NodeRuntimeError(
            f"Node >= {MIN_NODE_MAJOR} is unavailable on PATH; "
            "install Node or set MOLT_NODE_BIN"
        ) from exc
    return root, path


def resolve_node_runtime(
    *,
    source_root: Path | None = None,
    environment: Mapping[str, str] | None = None,
    guard_prefix: GuardStem = "MOLT_CROSS",
) -> NodeRuntime:
    """Resolve and probe an ordinary host without benchmark identity work."""
    environment = ProcessEnvironment(os.environ if environment is None else environment)
    root, path = _select_node_path(source_root=source_root, environment=environment)
    return _probe_node(
        path,
        source_root=root,
        environment=environment,
        guard_prefix=guard_prefix,
    )


@dataclass(frozen=True, slots=True)
class NodeExecutionPlan:
    """One admitted benchmark host; observations are not used-byte attestation."""

    runtime: NodeRuntime
    entrypoint: str
    content_filename: str
    size: int
    sha256: str
    file_version: StableRegularFileVersion

    def verify(self) -> None:
        """Fence replacements without rehashing the engine for each sample.

        Windows needs a handle ChangeTime query; POSIX needs one lstat. A closed
        handle interval cannot prove which bytes the OS eventually executed.
        """
        try:
            if os.name == "nt":
                verify_stable_regular_file_identity(
                    self.file_version, label="Node execution host"
                )
            else:
                verify_stable_regular_file_metadata(
                    self.file_version, label="Node execution host"
                )
        except (OSError, ValueError) as exc:
            raise NodeRuntimeError(str(exc)) from exc

    def as_record(self) -> dict[str, object]:
        return {
            "kind": "node",
            "executable": str(self.runtime.path),
            "version": self.runtime.version,
            "identity": {
                "entrypoint": self.entrypoint,
                "content_filename": self.content_filename,
                "size": self.size,
                "sha256": self.sha256,
            },
        }


def admit_node_execution(
    *,
    source_root: Path | None = None,
    environment: Mapping[str, str] | None = None,
    guard_prefix: GuardStem = "MOLT_BENCH",
) -> NodeExecutionPlan:
    """Select, hash and version-probe once, retaining that exact executable.

    Shared selection/probe owners keep explicit/pinned/PATH policy identical to
    ordinary runners. Resolve indirection before admission: later samples never
    reselect an ambient Node or follow a retargeted entrypoint symlink.
    """
    environment = ProcessEnvironment(os.environ if environment is None else environment)
    root, selected = _select_node_path(source_root=source_root, environment=environment)
    try:
        path = selected.resolve(strict=True)
        version = stable_regular_file_version(path, label="Node execution host")
        identity = executable_content_identity(path, label="Node execution host")
        runtime = _probe_node(
            path,
            source_root=root,
            environment=environment,
            guard_prefix=guard_prefix,
        )
        plan = NodeExecutionPlan(
            runtime,
            str(identity["entrypoint"]),
            str(identity["content_filename"]),
            int(identity["size"]),
            str(identity["sha256"]),
            version,
        )
        plan.verify()
        return plan
    except (OSError, ValueError) as exc:
        raise NodeRuntimeError(f"Node execution admission failed: {exc}") from exc

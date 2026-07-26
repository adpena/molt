"""Fail-closed launch authority for the sole final runtime-WASM build."""

from __future__ import annotations

import argparse
import contextlib
from dataclasses import dataclass
import hashlib
import json
import os
import shutil
import sqlite3
import subprocess
import sys
from collections.abc import Iterator, Mapping, Sequence
from pathlib import Path
from typing import Any

from molt.cli import wasm_toolchain
from molt.cli.atomic_io import _atomic_write_text
from molt.cli.cargo_profiles import _resolve_cargo_profile_name
from molt.cli.runtime_build import (
    _compute_runtime_wasm_build_spec,
    _provision_runtime_wasm_toolchain_manifest,
    _resolved_runtime_wasm_pair_identities,
    _runtime_wasm_toolchain_manifest_path,
)
from molt.cli.runtime_paths import _runtime_wasm_artifact_path_from_env
from molt.cli.runtime_wasm_generation import runtime_wasm_generation_path
from molt.dx import CheckoutCustody, checkout_custody, development_artifact_env
from molt.path_custody import (
    CustodyPathRole,
    PathCustodyError,
    canonical_host_path,
    host_path_is_within,
)
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore


SCHEMA = "molt.runtime-wasm-final-preflight.v2"
_COMMANDS = CommandExecutor.for_file(__file__)
_GIT_TIMEOUT_SECONDS = 30.0
_COMPILER_MUTEX = "compiler-build-resource"
_TERMINAL_GUARD_STATUSES = frozenset(
    {"completed", "finalizer_completed", "spawn_failed"}
)
_BUILD_TOOL_NAMES = frozenset(
    {
        "cargo",
        "cargo.exe",
        "rustc",
        "rustc.exe",
        "wasm-ld",
        "wasm-ld.exe",
        "clang",
        "clang.exe",
        "clang++",
        "clang++.exe",
        "ninja",
        "ninja.exe",
        "cmake",
        "cmake.exe",
        "meson",
        "meson.exe",
    }
)


class RuntimeWasmPreflightError(RuntimeError):
    """Typed fail-closed preflight boundary."""


class PreflightGitError(RuntimeWasmPreflightError):
    """A bounded git identity/custody query failed."""


@dataclass(frozen=True, slots=True)
class RuntimeWasmBuildClaim:
    run_id: str
    resource_family: str
    contention_key: str
    resource_mutex_key: str
    status: str
    guard_pid: int | None


@dataclass(frozen=True, slots=True)
class RuntimeWasmPreflightRoots:
    project: Path
    custody: Path
    target: Path
    cache: Path
    runtime: Path
    proof_queue_db: Path
    marker_dirs: tuple[Path, ...]


@dataclass(frozen=True, slots=True)
class RuntimeWasmPreflightContext:
    roots: RuntimeWasmPreflightRoots
    claim: RuntimeWasmBuildClaim
    build_env: Mapping[str, str]


def _git_bytes(project_root: Path, *args: str) -> bytes:
    command = ["git", *args]
    try:
        completed = _COMMANDS.run(
            command,
            cwd=project_root,
            check=False,
            capture_output=True,
            text=False,
            timeout=_GIT_TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        raise PreflightGitError(
            f"git command timed out after {_GIT_TIMEOUT_SECONDS:.0f}s: {' '.join(command)}"
        ) from exc
    except OSError as exc:
        raise PreflightGitError(
            f"git command could not start: {' '.join(command)}: {exc}"
        ) from exc
    if completed.returncode != 0:
        detail = completed.stderr.decode("utf-8", "replace").strip()[:512]
        raise PreflightGitError(
            f"git command failed rc={completed.returncode}: {' '.join(command)}: {detail}"
        )
    return completed.stdout


def _hash_file(path: Path) -> tuple[str, int]:
    hasher = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(8 * 1024 * 1024):
            hasher.update(chunk)
            size += len(chunk)
    return hasher.hexdigest(), size


def _update_framed_digest(hasher: Any, label: bytes, payload: bytes) -> None:
    """Hash one unambiguous field as label, size, and payload digest."""

    hasher.update(len(label).to_bytes(4, "big"))
    hasher.update(label)
    hasher.update(len(payload).to_bytes(8, "big"))
    hasher.update(hashlib.sha256(payload).digest())


def _framed_digest(fields: Sequence[tuple[bytes, bytes]]) -> str:
    hasher = hashlib.sha256()
    for label, payload in fields:
        _update_framed_digest(hasher, label, payload)
    return hasher.hexdigest()


def _untracked_identity(project_root: Path, raw_relative: bytes) -> bytes:
    relative = raw_relative.decode("utf-8", "surrogateescape")
    path = project_root / relative
    if path.is_symlink():
        kind = b"symlink"
        content = os.readlink(path).encode("utf-8", "surrogateescape")
        digest = hashlib.sha256(content).hexdigest().encode("ascii")
        size = len(content)
    elif path.is_file():
        kind = b"file"
        digest_text, size = _hash_file(path)
        digest = digest_text.encode("ascii")
    else:
        kind = b"missing"
        digest = hashlib.sha256(b"").hexdigest().encode("ascii")
        size = 0
    return bytes.fromhex(
        _framed_digest(
            (
                (b"path", raw_relative),
                (b"kind", kind),
                (b"content_sha256", digest),
                (b"content_size", str(size).encode("ascii")),
            )
        )
    )


def _source_identity(project_root: Path) -> dict[str, object]:
    head = _git_bytes(project_root, "rev-parse", "HEAD").decode("ascii").strip()
    head_tree = (
        _git_bytes(project_root, "rev-parse", "HEAD^{tree}").decode("ascii").strip()
    )
    diff = _git_bytes(project_root, "diff", "--binary", "HEAD", "--")
    untracked = sorted(
        value
        for value in _git_bytes(
            project_root, "ls-files", "--others", "--exclude-standard", "-z"
        ).split(b"\0")
        if value
    )
    diff_digest = hashlib.sha256(diff).hexdigest()
    fields: list[tuple[bytes, bytes]] = [
        (b"git_head", head.encode("ascii")),
        (b"git_head_tree", head_tree.encode("ascii")),
        (b"tracked_diff_sha256", diff_digest.encode("ascii")),
        (b"tracked_diff_size", str(len(diff)).encode("ascii")),
    ]
    fields.extend(
        (b"untracked_record_sha256", _untracked_identity(project_root, raw))
        for raw in untracked
    )
    return {
        "git_head": head,
        "git_head_tree": head_tree,
        "working_tree_digest": _framed_digest(fields),
        "tracked_diff_sha256": diff_digest,
        "tracked_diff_size": len(diff),
        "untracked_count": len(untracked),
    }


def _is_build_command(command: object) -> bool:
    if not isinstance(command, list):
        return False
    for raw in command:
        if not isinstance(raw, str):
            continue
        lowered = raw.strip().lower()
        if Path(lowered.strip('"')).name in _BUILD_TOOL_NAMES:
            return True
        if lowered == "internal-runtime-wasm-build":
            return True
    return False


def _worktree_roots(
    project_root: Path,
    *,
    custody_root: Path,
    source_role: CustodyPathRole,
) -> tuple[Path, ...]:
    roots: list[Path] = []
    for line in _git_bytes(
        project_root, "worktree", "list", "--porcelain"
    ).splitlines():
        if not line.startswith(b"worktree "):
            continue
        rendered = line.removeprefix(b"worktree ").decode("utf-8", "surrogateescape")
        root = canonical_host_path(
            rendered,
            source_role,
            authority="Molt worktree source root",
            require_exists=True,
        )
        if not host_path_is_within(root, custody_root):
            raise PathCustodyError(
                f"Molt worktree escaped checkout-family custody: {root}"
            )
        roots.append(root)
    return tuple(roots)


def _marker_directories(
    project_root: Path,
    custody_root: Path,
    *,
    source_role: CustodyPathRole,
) -> tuple[Path, ...]:
    candidates = {
        project_root / "tmp/memory_guard/active",
        custody_root / "tmp/memory_guard/active",
    }
    candidates.update(
        root / "tmp/memory_guard/active"
        for root in _worktree_roots(
            project_root,
            custody_root=custody_root,
            source_role=source_role,
        )
    )
    return tuple(sorted(candidates, key=os.fspath))


def _active_build_guards(
    marker_dirs: Sequence[Path],
    *,
    live_pids: frozenset[int],
    exclude_pids: frozenset[int] = frozenset(),
) -> list[dict[str, object]]:
    conflicts: list[dict[str, object]] = []
    for marker_dir in marker_dirs:
        for marker in sorted(marker_dir.glob("guard-*.json")):
            try:
                payload = json.loads(marker.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError):
                continue
            if not isinstance(payload, dict):
                continue
            status = str(payload.get("status", ""))
            if status in _TERMINAL_GUARD_STATUSES:
                continue
            command = payload.get("command")
            launch_command = payload.get("launch_command")
            if not (_is_build_command(command) or _is_build_command(launch_command)):
                continue
            try:
                pid = int(payload.get("pid", 0))
            except (TypeError, ValueError):
                pid = 0
            if pid not in live_pids or pid in exclude_pids:
                continue
            conflicts.append(
                {
                    "marker": os.fspath(marker),
                    "pid": pid,
                    "status": status,
                    "cwd": payload.get("cwd"),
                    "command": command,
                    "launch_command": launch_command,
                }
            )
    return conflicts


def _proof_queue_claim(db: Path, run_id: str) -> RuntimeWasmBuildClaim:
    if not db.is_file():
        raise RuntimeWasmPreflightError(f"proof queue database is missing: {db}")
    uri = db.resolve().as_uri() + "?mode=ro"
    try:
        with sqlite3.connect(uri, uri=True) as conn:
            conn.row_factory = sqlite3.Row
            row = conn.execute(
                """
                SELECT run_id, resource_family, contention_key, resource_mutex_key,
                       status, guard_pid
                FROM proof_runs
                WHERE run_id = ?
                """,
                (run_id,),
            ).fetchone()
    except sqlite3.Error as exc:
        raise RuntimeWasmPreflightError(
            f"proof queue claim query failed: {exc}"
        ) from exc
    if row is None:
        raise RuntimeWasmPreflightError(f"proof queue claim does not exist: {run_id}")
    if row["status"] != "running" or row["resource_mutex_key"] != _COMPILER_MUTEX:
        raise RuntimeWasmPreflightError(
            f"proof queue claim is not a live {_COMPILER_MUTEX} claim: "
            f"run_id={run_id} status={row['status']!r} mutex={row['resource_mutex_key']!r}"
        )
    return RuntimeWasmBuildClaim(
        run_id=str(row["run_id"]),
        resource_family=str(row["resource_family"]),
        contention_key=str(row["contention_key"]),
        resource_mutex_key=str(row["resource_mutex_key"]),
        status=str(row["status"]),
        guard_pid=int(row["guard_pid"]) if row["guard_pid"] is not None else None,
    )


def _proof_queue_conflicts(db: Path, *, exclude_run_id: str) -> list[dict[str, object]]:
    uri = db.resolve().as_uri() + "?mode=ro"
    with sqlite3.connect(uri, uri=True) as conn:
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            """
            SELECT run_id, resource_family, contention_key, resource_mutex_key,
                   status, guard_pid, started_at, command_json
            FROM proof_runs
            WHERE status IN ('dispatched', 'running')
              AND run_id != ?
              AND (
                resource_mutex_key = 'compiler-build-resource'
                OR resource_family IN (
                    'wasm', 'wasm-browser', 'native-build',
                    'queue-native-rust', 'rust'
                )
              )
            ORDER BY started_at, run_id
            """,
            (exclude_run_id,),
        )
    return [{key: row[key] for key in row.keys()} for row in rows]


def _claim_binds_current_process(
    claim: RuntimeWasmBuildClaim,
    samples: Mapping[int, object],
) -> bool:
    """Require the live queue guard to own this process through ancestry."""

    guard_pid = claim.guard_pid
    current_pid = os.getpid()
    if guard_pid is None or guard_pid not in samples:
        return False
    if guard_pid == current_pid:
        return True
    from tools import memory_guard

    return guard_pid in memory_guard._ancestor_pids(samples, current_pid)


def _revalidate_launch_custody(context: RuntimeWasmPreflightContext) -> None:
    """Bind the exact live claim and guard ancestry immediately before exec."""

    from tools import memory_guard

    final_claim = _proof_queue_claim(context.roots.proof_queue_db, context.claim.run_id)
    if final_claim != context.claim:
        raise RuntimeWasmPreflightError(
            "compiler-build-resource claim changed before runtime build exec"
        )
    samples = memory_guard.sample_processes()
    if not _claim_binds_current_process(final_claim, samples):
        raise RuntimeWasmPreflightError(
            "proof queue guard_pid is not the live current-process custody ancestor"
        )


def _custody_roles(custody: CheckoutCustody) -> tuple[CustodyPathRole, CustodyPathRole]:
    if custody.kind == "github-actions-ephemeral":
        return CustodyPathRole.HOSTED_SOURCE, CustodyPathRole.HOSTED_EXECUTION
    if custody.kind == "explicit-scratch":
        return CustodyPathRole.EXPLICIT_SCRATCH, CustodyPathRole.EXPLICIT_SCRATCH
    return CustodyPathRole.DURABLE_AUTHORITY, CustodyPathRole.DURABLE_AUTHORITY


def _resolve_preflight_context(
    project_root: Path, env: Mapping[str, str]
) -> RuntimeWasmPreflightContext:
    if env.get("MOLT_PROOF_QUEUE", "").strip() != "1":
        raise RuntimeWasmPreflightError(
            "final runtime build requires MOLT_PROOF_QUEUE=1"
        )
    run_id = env.get("MOLT_PROOF_QUEUE_RUN_ID", "").strip()
    db_raw = env.get("MOLT_PROOF_QUEUE_DB", "").strip()
    if not run_id or not db_raw:
        raise RuntimeWasmPreflightError("proof queue run id and database are required")

    custody = checkout_custody(project_root, env, require_exists=True)
    source_role, execution_role = _custody_roles(custody)
    canonical_project = canonical_host_path(
        project_root,
        source_role,
        authority="runtime-WASM source root",
        require_exists=True,
    )
    build_env = development_artifact_env(
        canonical_project,
        env,
        session_prefix="proof-wasm",
        create_dirs=False,
    )
    required = ("CARGO_TARGET_DIR", "MOLT_CACHE")
    missing = [name for name in required if not build_env.get(name, "").strip()]
    if missing:
        raise RuntimeWasmPreflightError(
            "canonical build environment is missing: " + ", ".join(missing)
        )
    canonical_custody = canonical_host_path(
        custody.custody_root,
        execution_role,
        authority="runtime-WASM custody root",
        require_exists=True,
    )
    target = canonical_host_path(
        build_env["CARGO_TARGET_DIR"],
        execution_role,
        authority="runtime-WASM target root",
    )
    cache = canonical_host_path(
        build_env["MOLT_CACHE"],
        execution_role,
        authority="runtime-WASM cache root",
    )
    runtime = canonical_host_path(
        _runtime_wasm_artifact_path_from_env(
            canonical_project, "molt_runtime.wasm", build_env
        ).parent,
        execution_role,
        authority="runtime-WASM publication root",
    )
    proof_queue_db = canonical_host_path(
        db_raw,
        execution_role,
        authority="runtime-WASM proof queue database",
        require_exists=True,
    )
    build_env.update(
        {
            "CARGO_TARGET_DIR": os.fspath(target),
            "MOLT_CACHE": os.fspath(cache),
            "MOLT_WASM_RUNTIME_DIR": os.fspath(runtime),
            "MOLT_PROOF_QUEUE": "1",
            "MOLT_PROOF_QUEUE_RUN_ID": run_id,
            "MOLT_PROOF_QUEUE_DB": os.fspath(proof_queue_db),
        }
    )
    roots = RuntimeWasmPreflightRoots(
        project=canonical_project,
        custody=canonical_custody,
        target=target,
        cache=cache,
        runtime=runtime,
        proof_queue_db=proof_queue_db,
        marker_dirs=_marker_directories(
            canonical_project,
            canonical_custody,
            source_role=source_role,
        ),
    )
    claim = _proof_queue_claim(proof_queue_db, run_id)
    return RuntimeWasmPreflightContext(roots=roots, claim=claim, build_env=build_env)


def _disk_facts(
    paths: Sequence[Path], *, reserve_bytes: int
) -> list[dict[str, object]]:
    by_volume: dict[str, dict[str, object]] = {}
    for path in paths:
        anchor = path
        while not anchor.exists() and anchor != anchor.parent:
            anchor = anchor.parent
        usage = shutil.disk_usage(anchor)
        volume = os.path.normcase(os.fspath(anchor.anchor or anchor))
        by_volume.setdefault(
            volume,
            {
                "volume": volume,
                "anchor": os.fspath(anchor),
                "total_bytes": usage.total,
                "used_bytes": usage.used,
                "free_bytes": usage.free,
                "reserve_bytes": reserve_bytes,
                "ready": usage.free >= reserve_bytes,
            },
        )
    return [by_volume[key] for key in sorted(by_volume)]


@contextlib.contextmanager
def _exact_build_environment(values: Mapping[str, str]) -> Iterator[None]:
    previous = {key: os.environ.get(key) for key in values}
    os.environ.update(values)
    try:
        yield
    finally:
        for key, value in previous.items():
            if value is None:
                os.environ.pop(key, None)
            else:
                os.environ[key] = value


def _planned_pair(
    *,
    project_root: Path,
    target_root: Path,
    cache_root: Path,
    runtime_dir: Path,
    build_profile: str,
    stdlib_profile: str,
) -> dict[str, object]:
    required_env = {
        "CARGO_TARGET_DIR": os.fspath(target_root),
        "MOLT_CACHE": os.fspath(cache_root),
        "MOLT_WASM_RUNTIME_DIR": os.fspath(runtime_dir),
    }
    with _exact_build_environment(required_env):
        cargo_profile, error = _resolve_cargo_profile_name(build_profile)  # type: ignore[arg-type]
        if error is not None:
            raise ValueError(error)
        shared = runtime_dir / "molt_runtime.wasm"
        reloc = runtime_dir / "molt_runtime_reloc.wasm"
        try:
            linker_identity = wasm_toolchain.resolve_wasm_linker()
        except wasm_toolchain.WasmLinkerContractError as exc:
            raise ValueError(f"runtime WASM linker identity failed: {exc}") from exc
        if linker_identity is None:
            raise ValueError("runtime WASM linker identity failed: wasm-ld not found")
        common = {
            "cargo_profile": cargo_profile,
            "simd_enabled": True,
            "freestanding": False,
            "stdlib_profile": stdlib_profile,
            "resolved_modules": None,
            "required_link_features": frozenset(),
            "required_exports": None,
            "wasm_linker_identity": linker_identity,
        }
        shared_spec = _compute_runtime_wasm_build_spec(
            project_root, shared, reloc=False, **common
        )
        reloc_spec = _compute_runtime_wasm_build_spec(
            project_root, reloc, reloc=True, **common
        )
        if (
            shared_spec.target_root != target_root
            or reloc_spec.target_root != target_root
        ):
            raise ValueError("resolved runtime target root differs from preflight root")
        toolchain_manifest = _provision_runtime_wasm_toolchain_manifest(shared_spec)
        manifest_path = _runtime_wasm_toolchain_manifest_path(shared_spec)
        toolchain_manifest.write(manifest_path)
        shared_identity, reloc_identity = _resolved_runtime_wasm_pair_identities(
            project_root,
            shared_spec,
            reloc_spec,
            toolchain_manifest=toolchain_manifest,
        )
    if shared_identity.pair_digest != reloc_identity.pair_digest:
        raise ValueError("planned runtime identities do not form one pair")
    return {
        "required_env": required_env,
        "cargo_profile": cargo_profile,
        "toolchain_manifest": os.fspath(manifest_path),
        "toolchain_digest": toolchain_manifest.digest,
        "pair_digest": shared_identity.pair_digest,
        "shared": {"path": os.fspath(shared), "digest": shared_identity.digest},
        "reloc": {"path": os.fspath(reloc), "digest": reloc_identity.digest},
        "generation": os.fspath(runtime_wasm_generation_path(shared)),
        "expected_identity": os.fspath(
            target_root
            / ".molt_state/runtime_wasm_generations"
            / f"{shared_identity.pair_digest}.expected.json"
        ),
    }


def _custody_facts(
    context: RuntimeWasmPreflightContext,
    *,
    live_pids: frozenset[int],
) -> tuple[list[dict[str, object]], list[dict[str, object]]]:
    queue_conflicts = _proof_queue_conflicts(
        context.roots.proof_queue_db, exclude_run_id=context.claim.run_id
    )
    guard_conflicts = _active_build_guards(
        context.roots.marker_dirs,
        live_pids=live_pids,
        exclude_pids=frozenset({os.getpid()}),
    )
    return queue_conflicts, guard_conflicts


def build_preflight(
    *,
    context: RuntimeWasmPreflightContext,
    reserve_bytes: int,
    build_profile: str,
    stdlib_profile: str,
) -> dict[str, object]:
    roots = context.roots
    errors: list[str] = []
    try:
        source_before = _source_identity(roots.project)
    except (OSError, UnicodeError, PreflightGitError) as exc:
        source_before = None
        errors.append(f"source commit/tree identity failed: {exc}")
    try:
        from tools import memory_guard

        process_samples = memory_guard.sample_processes()
        live_pids = frozenset(process_samples)
        queue_conflicts, guard_conflicts = _custody_facts(context, live_pids=live_pids)
    except Exception as exc:  # pragma: no cover - fail-closed host sampler boundary
        live_pids = frozenset()
        process_samples = {}
        queue_conflicts = []
        guard_conflicts = []
        errors.append(f"process custody sampler failed: {type(exc).__name__}: {exc}")
    disk = _disk_facts(
        (roots.project, roots.target, roots.cache, roots.runtime),
        reserve_bytes=reserve_bytes,
    )
    try:
        plan = _planned_pair(
            project_root=roots.project,
            target_root=roots.target,
            cache_root=roots.cache,
            runtime_dir=roots.runtime,
            build_profile=build_profile,
            stdlib_profile=stdlib_profile,
        )
    except (OSError, RuntimeError, ValueError) as exc:
        plan = None
        errors.append(f"planned runtime identity failed: {exc}")

    # Re-read every mutable authority after planning. The proof-queue mutex is
    # still held by this process and remains held across the subsequent exec.
    try:
        source_after = _source_identity(roots.project)
    except (OSError, UnicodeError, PreflightGitError) as exc:
        source_after = None
        errors.append(f"final source identity failed: {exc}")
    final_disk = _disk_facts(
        (roots.project, roots.target, roots.cache, roots.runtime),
        reserve_bytes=reserve_bytes,
    )
    try:
        final_claim = _proof_queue_claim(roots.proof_queue_db, context.claim.run_id)
        final_queue_conflicts, final_guard_conflicts = _custody_facts(
            context, live_pids=live_pids
        )
    except (OSError, RuntimeWasmPreflightError, sqlite3.Error) as exc:
        final_claim = None
        final_queue_conflicts = []
        final_guard_conflicts = []
        errors.append(f"final custody validation failed: {exc}")

    source_ready = source_before is not None and source_before == source_after
    disk_ready = all(bool(fact["ready"]) for fact in disk + final_disk)
    custody_ready = not (
        queue_conflicts
        or guard_conflicts
        or final_queue_conflicts
        or final_guard_conflicts
    )
    claim_ready = final_claim == context.claim
    guard_binding_ready = final_claim is not None and _claim_binds_current_process(
        final_claim, process_samples
    )
    checks = {
        "canonical_exact_roots": True,
        "disk_reserve_revalidated": disk_ready,
        "exclusive_build_custody_revalidated": custody_ready,
        "compiler_build_claim_held": claim_ready,
        "live_guard_custody_binding": guard_binding_ready,
        "planned_pair_identity": plan is not None,
        "source_identity_revalidated": source_ready,
    }
    if not disk_ready:
        errors.append("one or more build volumes are below the free-space reserve")
    if not custody_ready:
        errors.append("active competing Molt-owned compiler/build custody exists")
    if not claim_ready:
        errors.append("compiler-build-resource claim changed during preflight")
    if not guard_binding_ready:
        errors.append("live proof queue guard_pid does not own the current process")
    if not source_ready:
        errors.append("source identity changed during preflight")
    ready = all(checks.values()) and not errors
    return {
        "schema": SCHEMA,
        "status": "ready" if ready else "blocked",
        "checks": checks,
        "claim": {
            "run_id": context.claim.run_id,
            "resource_family": context.claim.resource_family,
            "contention_key": context.claim.contention_key,
            "resource_mutex_key": context.claim.resource_mutex_key,
            "status": context.claim.status,
            "guard_pid": context.claim.guard_pid,
        },
        "roots": {
            "project": os.fspath(roots.project),
            "custody": os.fspath(roots.custody),
            "target": os.fspath(roots.target),
            "cache": os.fspath(roots.cache),
            "runtime": os.fspath(roots.runtime),
            "proof_queue_db": os.fspath(roots.proof_queue_db),
            "active_marker_dirs": [os.fspath(path) for path in roots.marker_dirs],
        },
        "source": source_after,
        "disk": {"before": disk, "after": final_disk},
        "custody": {
            "proof_queue_conflicts": queue_conflicts,
            "active_build_guards": guard_conflicts,
            "final_proof_queue_conflicts": final_queue_conflicts,
            "final_active_build_guards": final_guard_conflicts,
        },
        "plan": plan,
        "errors": errors,
    }


def _build_command(
    *,
    build_profile: str,
    stdlib_profile: str,
    cargo_timeout: float | None,
) -> list[str]:
    command = [
        sys.executable,
        "-m",
        "molt.cli",
        "internal-runtime-wasm-build",
        "--build-profile",
        build_profile,
        "--kind",
        "both",
        "--stdlib-profile",
        stdlib_profile,
        "--json",
    ]
    if cargo_timeout is not None:
        command.extend(("--cargo-timeout", str(cargo_timeout)))
    return command


def _exec_build(command: Sequence[str], env: Mapping[str, str]) -> None:
    os.execvpe(command[0], list(command), dict(env))


def _blocked_payload(error: Exception) -> dict[str, object]:
    return {
        "schema": SCHEMA,
        "status": "blocked",
        "checks": {},
        "errors": [f"{type(error).__name__}: {error}"],
    }


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--project-root", type=Path, default=Path.cwd())
    parser.add_argument("--reserve-gib", type=float, default=8.0)
    parser.add_argument(
        "--build-profile", choices=("dev", "release"), default="release"
    )
    parser.add_argument("--stdlib-profile", default="full")
    parser.add_argument("--cargo-timeout", type=float)
    parser.add_argument("--launch", action="store_true")
    args = parser.parse_args(argv)
    if not args.launch:
        parser.error(
            "--launch is required; reusable source-only validation is forbidden"
        )

    try:
        context = _resolve_preflight_context(args.project_root, os.environ)
        payload = build_preflight(
            context=context,
            reserve_bytes=max(0, int(args.reserve_gib * 1024**3)),
            build_profile=args.build_profile,
            stdlib_profile=args.stdlib_profile,
        )
        receipt = (
            context.roots.custody
            / "logs/runtime_wasm_final_preflight"
            / f"{context.claim.run_id}.json"
        )
        if not host_path_is_within(receipt, context.roots.custody):
            raise RuntimeWasmPreflightError("preflight receipt escaped custody root")
        payload["receipt"] = os.fspath(receipt)
        encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"
        _atomic_write_text(receipt, encoded)
        if payload["status"] == "ready":
            try:
                _revalidate_launch_custody(context)
            except (OSError, RuntimeWasmPreflightError, sqlite3.Error) as exc:
                payload = _blocked_payload(exc)
                payload["receipt"] = os.fspath(receipt)
                encoded = (
                    json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"
                )
                _atomic_write_text(receipt, encoded)
    except (OSError, PathCustodyError, RuntimeWasmPreflightError, sqlite3.Error) as exc:
        payload = _blocked_payload(exc)
        encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"
        print(encoded, end="", flush=True)
        return 2

    print(encoded, end="", flush=True)
    if payload["status"] != "ready":
        return 2
    command = _build_command(
        build_profile=args.build_profile,
        stdlib_profile=args.stdlib_profile,
        cargo_timeout=args.cargo_timeout,
    )
    _exec_build(command, context.build_env)
    raise AssertionError("os.execvpe returned")


if __name__ == "__main__":
    raise SystemExit(main())

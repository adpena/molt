#!/usr/bin/env python3
"""Produce every installed-runtime cell from one immutable source snapshot.

Release-time production is the only distribution lane that runs runtime Cargo
plans. Coverage is derived from the existing authorities: guest profiles
(``BuildProfile``), runtime stdlib tiers, the source-extension runtime feature,
and the WASM freestanding/SIMD policy. There is no second matrix. The exact
release commit is materialized with the canonical Git snapshot authority and
verified unchanged after the builds; the source-checkout builders then publish
canonical native-link and immutable WASM-generation receipts. Native cells also
stage the archive-derived callable projection written by the same
``_runtime_callable_symbols_file`` authority source-checkout codegen uses, so
installed codegen never runs a symbol reader. Members are staged
as ``<output>/<cell-id>/<member>`` plus ``runtime-inventory.json``. Staged
receipts pass the installed admission rules and their ``compile.sources`` must
equal the snapshot's runtime source tree identity. The inventory carries the
snapshot's Git identity, which the bundle must match.
"""

from __future__ import annotations

from molt.temporary_artifacts import OwnedTemporaryDirectory

import argparse
from collections.abc import Iterator
import contextlib
from dataclasses import dataclass
import json
import os
from pathlib import Path
import shutil
import sys
from typing import Any, Mapping, Sequence, get_args

ROOT = Path(__file__).resolve().parents[2]
INVENTORY_NAME = "runtime-inventory.json"
# Variables that select a runtime configuration other than the derived release
# policy. Production and coverage verification refuse them rather than
# publishing a cell the policy does not name.
POLICY_OVERRIDE_ENV = (
    "MOLT_DEV_CARGO_PROFILE",
    "MOLT_RELEASE_CARGO_PROFILE",
    "MOLT_WASM_CARGO_PROFILE",
    "MOLT_RUNTIME_BUILD_PROFILE",
    "MOLT_RUNTIME_TK_NATIVE",
    "MOLT_RUNTIME_GPU_METAL",
    "MOLT_RUNTIME_GPU_WEBGPU",
    "MOLT_RUNTIME_GPU_CUDA",
    "MOLT_RUNTIME_GPU_HIP",
    "MOLT_SKIP_RUNTIME_REBUILD",
    "MOLT_WASM_RUNTIME_DIR",
    "MOLT_SOURCE_ROOT",
    "MOLT_BUNDLE_ROOT",
)


@dataclass(frozen=True)
class RuntimeCellRequest:
    kind: str
    guest_profile: str
    stdlib_profile: str
    extra_runtime_features: tuple[str, ...] = ()
    freestanding: bool = False


def require_release_policy_environment(env: Mapping[str, str] = os.environ) -> None:
    present = sorted(name for name in POLICY_OVERRIDE_ENV if env.get(name))
    if present:
        raise ValueError(
            "runtime cell policy overrides are not release policy: "
            + ", ".join(present)
        )


def declared_runtime_cells() -> tuple[RuntimeCellRequest, ...]:
    """Project the supported guest surface onto runtime cells.

    Every program an installed compiler accepts selects one of these cells:
    guest profile x concrete stdlib tier x (native: plain or source-extension
    loader; WASM: hosted SIMD or freestanding scalar).
    """
    from molt.cli.config_resolution import RUNTIME_STDLIB_PROFILE_TIERS
    from molt.cli.models import BuildProfile
    from molt.cli.runtime_features import SOURCE_EXTENSION_RUNTIME_FEATURES
    from molt.compiler_distribution import NATIVE_RUNTIME_CELL, WASM_RUNTIME_CELL

    requests: list[RuntimeCellRequest] = []
    for profile in get_args(BuildProfile):
        for tier in RUNTIME_STDLIB_PROFILE_TIERS:
            for extra in ((), SOURCE_EXTENSION_RUNTIME_FEATURES):
                requests.append(
                    RuntimeCellRequest(NATIVE_RUNTIME_CELL, profile, tier, extra)
                )
            for freestanding in (False, True):
                requests.append(
                    RuntimeCellRequest(
                        WASM_RUNTIME_CELL, profile, tier, freestanding=freestanding
                    )
                )
    return tuple(requests)


def runtime_cell_key(request: RuntimeCellRequest) -> dict[str, Any]:
    """The installed selector's own key projection for one derived request."""
    from molt.cli.cargo_profiles import _resolve_cargo_profile_name
    from molt.cli.installed_runtime import (
        native_runtime_cell_key,
        wasm_runtime_cell_key,
    )
    from molt.cli.runtime_wasm_build_policy import runtime_wasm_simd_policy
    from molt.compiler_distribution import NATIVE_RUNTIME_CELL

    require_release_policy_environment()
    cargo_profile, error = _resolve_cargo_profile_name(request.guest_profile)  # type: ignore[arg-type]
    if error is not None:
        raise ValueError(error)
    if request.kind == NATIVE_RUNTIME_CELL:
        return native_runtime_cell_key(
            target_triple=None,
            cargo_profile=cargo_profile,
            stdlib_profile=request.stdlib_profile,
            extra_runtime_features=request.extra_runtime_features,
        )
    return wasm_runtime_cell_key(
        cargo_profile=cargo_profile,
        stdlib_profile=request.stdlib_profile,
        simd_enabled=runtime_wasm_simd_policy(freestanding=request.freestanding),
        freestanding=request.freestanding,
    )


def _selector(kind: str, key: Mapping[str, Any]) -> str:
    return json.dumps([kind, dict(key)], sort_keys=True)


def declared_cell_keys() -> list[str]:
    return sorted(
        _selector(request.kind, runtime_cell_key(request))
        for request in declared_runtime_cells()
    )


def inventory_cell_keys(inventory: Mapping[str, Any]) -> list[str]:
    return sorted(_selector(cell["kind"], cell["key"]) for cell in inventory["cells"])


def _stage_member(
    source: Path, cell_root: Path, name: str, role: str
) -> dict[str, Any]:
    from molt.toolchain_identity import stable_regular_file_content_identity

    before = stable_regular_file_content_identity(source, label=f"runtime cell {role}")
    destination = cell_root / name
    shutil.copyfile(source, destination)
    destination.chmod(0o644)
    after = stable_regular_file_content_identity(destination, label=f"staged {role}")
    if (before["sha256"], before["size"]) != (after["sha256"], after["size"]):
        raise ValueError(f"runtime cell {role} changed while staging: {source}")
    return {
        "role": role,
        "name": name,
        "sha256": after["sha256"],
        "size": after["size"],
    }


def _publish_cell(
    output: Path,
    *,
    kind: str,
    key: dict[str, Any],
    members: Sequence[tuple[Path, str, str]],
) -> dict[str, Any]:
    from molt.compiler_distribution import runtime_cell_id

    # This tree remains private until the complete inventory is admitted.
    # Never remove a fixed scratch name that another producer might own.
    with OwnedTemporaryDirectory(prefix=".cell-", dir=output) as temporary:
        staging = Path(temporary) / "members"
        staging.mkdir()
        files = sorted(
            (
                _stage_member(source, staging, name, role)
                for source, name, role in members
            ),
            key=lambda entry: entry["name"],
        )
        cell_id = runtime_cell_id(kind, key, files)
        destination = output / cell_id
        if destination.exists():
            raise ValueError(f"duplicate runtime cell {cell_id}")
        staging.rename(destination)
    return {"id": cell_id, "kind": kind, "key": key, "files": files}


def _produce_native(
    request: RuntimeCellRequest, source_root: Path, output: Path, timeout: float | None
) -> dict[str, Any]:
    from molt.cli.cargo_profiles import _resolve_cargo_profile_name
    from molt.cli.models import _RuntimeArtifactState
    from molt.cli.native_link_custody import native_link_custody_archive_path
    from molt.cli.native_link_manifest import (
        native_link_dependency_manifest_path,
        read_native_link_dependency_manifest,
    )
    from molt.cli.native_symbol_inspection import _native_symbol_artifact_identity
    from molt.cli.runtime_callable_symbols import _runtime_callable_symbols_file
    from molt.cli.runtime_native_build import _ensure_runtime_lib
    from molt.cli.runtime_paths import _runtime_lib_path
    from molt.compiler_distribution import (
        NATIVE_CALLABLE_PROJECTION_ROLE,
        NATIVE_RUNTIME_CELL,
    )

    cargo_profile, _error = _resolve_cargo_profile_name(request.guest_profile)  # type: ignore[arg-type]
    runtime_lib = _runtime_lib_path(
        source_root, cargo_profile, None, stdlib_profile=request.stdlib_profile
    )
    state = _RuntimeArtifactState(
        runtime_lib=runtime_lib, extra_runtime_features=request.extra_runtime_features
    )
    if (
        not _ensure_runtime_lib(
            runtime_lib,
            None,
            True,
            cargo_profile,
            source_root,
            timeout,
            stdlib_profile=request.stdlib_profile,
            extra_runtime_features=request.extra_runtime_features,
            runtime_state=state,
        )
        or state.native_runtime_build_identity is None
    ):
        failure = state.native_runtime_build_failure
        raise RuntimeError(
            f"native runtime cell build failed: {request}: "
            + (failure.summary if failure is not None else "no admitted identity")
        )
    runtime_lib = state.runtime_lib
    if runtime_lib is None:
        raise RuntimeError("native runtime cell has no admitted generation")
    manifest = read_native_link_dependency_manifest(
        runtime_lib,
        target_triple=None,
        cargo_profile=cargo_profile,
        runtime_build_identity=state.native_runtime_build_identity,
    )
    manifest_path = native_link_dependency_manifest_path(runtime_lib)
    members = [
        (runtime_lib, runtime_lib.name, "runtime_archive"),
        (manifest_path, manifest_path.name, "native_link_manifest"),
    ]
    custody = native_link_custody_archive_path(runtime_lib, manifest["custody"])  # type: ignore[arg-type]
    if custody is not None:
        members.append((custody, custody.name, "native_link_custody_archive"))
    # The same content-addressed projection source-checkout codegen admits; the
    # release host's reader runs here once so installed hosts never need one.
    projection, failure = _runtime_callable_symbols_file(
        runtime_lib,
        identity=_native_symbol_artifact_identity(runtime_lib),
        target_triple=None,
    )
    if projection is None:
        raise RuntimeError(
            f"native runtime cell callable projection failed: {request}: {failure}"
        )
    members.append(
        (
            projection.identity.path,
            projection.identity.path.name,
            NATIVE_CALLABLE_PROJECTION_ROLE,
        )
    )
    return _publish_cell(
        output, kind=NATIVE_RUNTIME_CELL, key=runtime_cell_key(request), members=members
    )


def _produce_wasm(
    request: RuntimeCellRequest, source_root: Path, output: Path, timeout: float | None
) -> dict[str, Any]:
    from molt.cli.cargo_profiles import _resolve_cargo_profile_name
    from molt.cli.runtime_build import _initialize_runtime_artifact_state
    from molt.cli.runtime_wasm_build_policy import runtime_wasm_simd_policy
    from molt.cli.runtime_wasm_build_spec import runtime_wasm_distribution_surface
    from molt.cli.runtime_wasm_pair_build import _ensure_runtime_wasm_both
    from molt.compiler_distribution import WASM_RUNTIME_CELL

    cargo_profile, _error = _resolve_cargo_profile_name(request.guest_profile)  # type: ignore[arg-type]
    state = _initialize_runtime_artifact_state(
        is_rust_transpile=False,
        is_wasm=True,
        emit_mode="wasm",
        molt_root=source_root,
        runtime_cargo_profile=cargo_profile,
        target_triple=None,
        stdlib_profile=request.stdlib_profile,
    )
    # A distributed cell carries its complete tier ceiling and the full
    # canonical CPython C-API export surface; installed admission then checks
    # each program's own imports against the shipped members.
    required_exports, ceiling = runtime_wasm_distribution_surface(
        request.stdlib_profile
    )
    if (
        not _ensure_runtime_wasm_both(
            state,
            json_output=True,
            cargo_profile=cargo_profile,
            cargo_timeout=timeout,
            project_root=source_root,
            simd_enabled=runtime_wasm_simd_policy(freestanding=request.freestanding),
            freestanding=request.freestanding,
            stdlib_profile=request.stdlib_profile,
            resolved_modules=None,
            required_link_features=ceiling,
            required_exports=required_exports,
            bind_for_codegen=True,
        )
        or state.runtime_wasm_codegen_binding is None
    ):
        failure = state.runtime_wasm_build_failure
        raise RuntimeError(
            f"WASM runtime cell build failed: {request}: "
            + (failure.summary if failure is not None else "no codegen binding")
        )
    generation = state.runtime_wasm_codegen_binding.generation
    return _publish_cell(
        output,
        kind=WASM_RUNTIME_CELL,
        key=runtime_cell_key(request),
        members=[
            # The pinned snapshot is the same canonical generation payload.
            (
                generation.manifest,
                "molt_runtime.generation.json",
                "wasm_generation_manifest",
            ),
            (generation.shared, generation.shared.name, "wasm_shared_member"),
            (generation.reloc, generation.reloc.name, "wasm_reloc_member"),
        ],
    )


@contextlib.contextmanager
def _isolated_build_environment(work: Path) -> Iterator[None]:
    """Keep Cargo and build state outside the immutable source snapshot."""
    updates = {
        "CARGO_TARGET_DIR": str(work / "target"),
        "MOLT_BUILD_STATE_DIR": str(work / "build-state"),
        "MOLT_HOME": str(work / "home"),
        "MOLT_CACHE": str(work / "cache"),
    }
    previous = {name: os.environ.get(name) for name in updates}
    os.environ.update(updates)
    try:
        # Source-checkout WASM coordinates default to ``cwd/wasm``; keep them
        # (and every other cwd-relative output) in the private work root.
        with contextlib.chdir(work):
            yield
    finally:
        for name, value in previous.items():
            if value is None:
                os.environ.pop(name, None)
            else:
                os.environ[name] = value


def produce_runtime_cells(
    repo_root: Path,
    output: Path,
    *,
    source_sha: str,
    platform: str,
    arch: str,
    cargo_timeout: float | None = None,
) -> dict[str, Any]:
    """Publish all cells and their verified inventory in one durable commit.

    Compilation, provenance checks and inventory admission happen in a private
    sibling. Failure leaves the requested path absent; an existing generation
    is never replaced, including one committed by a concurrent producer.
    """
    from molt.file_publication import (
        durable_publish_directory_exclusive,
        resolve_owned_path,
    )
    from molt.verified_subset import current_host_coordinate

    require_release_policy_environment()
    if current_host_coordinate() != (platform, arch):
        raise ValueError("runtime cells are produced on their own release host")
    # Bind relative output before entering the private build cwd. Reject path
    # indirection before resolving away its spelling.
    output = resolve_owned_path(output)
    if output.exists():
        raise FileExistsError(f"runtime cell destination already exists: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with OwnedTemporaryDirectory(
        prefix=".runtime-cells-", dir=output.parent
    ) as temporary:
        stage = Path(temporary) / "publish"
        inventory = _populate_runtime_cells(
            repo_root,
            stage,
            source_sha=source_sha,
            platform=platform,
            arch=arch,
            cargo_timeout=cargo_timeout,
        )
        durable_publish_directory_exclusive(stage, output)
    return inventory


def _populate_runtime_cells(
    repo_root: Path,
    output: Path,
    *,
    source_sha: str,
    platform: str,
    arch: str,
    cargo_timeout: float | None = None,
) -> dict[str, Any]:
    from molt.compiler_distribution import (
        NATIVE_RUNTIME_CELL,
        RUNTIME_INVENTORY_SCHEMA,
        validate_runtime_inventory,
        verify_runtime_tree,
        verify_source_inventory,
    )
    from molt.cli.installed_runtime import admit_runtime_cell_receipts
    from molt.cli.runtime_build_identity import verify_runtime_source_identities
    from molt.exact_json import write_exact
    from molt.toolchain_identity import resolve_executable
    from tools.release.compiler_payload import source_environment, source_snapshot
    from tools.release.git_source_snapshot import materialize_git_source_snapshot

    snapshot = source_snapshot(repo_root, source_sha)
    output.mkdir(parents=True, exist_ok=False)
    with OwnedTemporaryDirectory(prefix="molt-runtime-cells-") as temporary:
        work = Path(temporary)
        env = source_environment()
        source_root = materialize_git_source_snapshot(
            snapshot,
            work / "source",
            repo_root=repo_root,
            git=resolve_executable("git", environment=env, label="release source Git"),
            environment=env,
        )
        records = [entry.as_record() for entry in snapshot.files]
        with _isolated_build_environment(work):
            cells = [
                (
                    _produce_native
                    if request.kind == NATIVE_RUNTIME_CELL
                    else _produce_wasm
                )(request, source_root, output, cargo_timeout)
                for request in declared_runtime_cells()
            ]
        # Every staged cell passes the installed receipt admission, and each
        # identity's recorded runtime sources must be this snapshot's. The
        # union of their source roots is hashed once, not once per cell.
        verify_runtime_source_identities(
            source_root,
            [
                identity
                for cell in cells
                for identity in admit_runtime_cell_receipts(cell, output / cell["id"])
            ],
        )
        # Builds read the snapshot only; any write would change provenance.
        verify_source_inventory(source_root, records)
    inventory = validate_runtime_inventory(
        {
            "schema": RUNTIME_INVENTORY_SCHEMA,
            "platform": platform,
            "arch": arch,
            "source": {
                "object_format": snapshot.object_format,
                "commit": snapshot.source_sha,
                "tree": snapshot.tree_sha,
            },
            "cells": sorted(cells, key=lambda cell: cell["id"]),
        },
        platform=platform,
        arch=arch,
    )
    if inventory_cell_keys(inventory) != declared_cell_keys():
        raise ValueError(
            "produced runtime cells differ from the derived release policy"
        )
    verify_runtime_tree(output, inventory)
    write_exact(output / INVENTORY_NAME, inventory)
    return inventory


def read_runtime_inventory(root: Path, *, platform: str, arch: str) -> dict[str, Any]:
    """Admit one staged cell tree exactly as the installed CLI will see it."""
    from molt.compiler_distribution import (
        validate_runtime_inventory,
        verify_runtime_tree,
    )
    from molt.exact_json import read_exact

    inventory = validate_runtime_inventory(
        read_exact(
            root / INVENTORY_NAME, max_bytes=16 * 1024 * 1024, label="runtime inventory"
        ),
        platform=platform,
        arch=arch,
    )
    verify_runtime_tree(root, inventory, extra_names=frozenset({INVENTORY_NAME}))
    return inventory


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument(
        "--platform", choices=["macos", "linux", "windows"], required=True
    )
    parser.add_argument("--arch", required=True)
    parser.add_argument("--cargo-timeout", type=float)
    args = parser.parse_args(argv)
    inventory = produce_runtime_cells(
        ROOT,
        args.output,
        source_sha=args.source_sha,
        platform=args.platform,
        arch=args.arch,
        cargo_timeout=args.cargo_timeout,
    )
    print(json.dumps({"cells": len(inventory["cells"])}, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())

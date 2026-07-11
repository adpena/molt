#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.scientific_stack_versions import (  # noqa: E402
    NUMPY_WITNESS_SEAL_NAME,
    attest_numpy_witness_seal,
    numpy_witness_seal_root,
    resolve_scientific_stack,
)

BUILD_ROOT_NAME = "pact_numpy_multiarray_meson_wasm_build"
NUMPY_SOURCE_REL = Path("bench/friends/repos/numpy_off_the_shelf")


def _copy_tree(source: Path, destination: Path) -> None:
    if not source.is_dir():
        raise ValueError(f"required NumPy seal custody input is missing: {source}")
    shutil.copytree(source, destination, copy_function=shutil.copy2)


def _relocate_manifests(staging: Path, destination_parent: Path) -> None:
    seal_root = staging / NUMPY_WITNESS_SEAL_NAME
    build_root = destination_parent / BUILD_ROOT_NAME
    source_root = destination_parent / NUMPY_SOURCE_REL
    manifest_paths = sorted(seal_root.rglob("*.extension_manifest.json"))
    root_manifest = seal_root / "extension_manifest.json"
    if root_manifest.is_file():
        manifest_paths.append(root_manifest)
    for manifest_path in manifest_paths:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        source_plan = manifest.get("source_plan")
        if not isinstance(source_plan, dict):
            continue
        source_plan["build_root"] = str(build_root.resolve())
        source_plan["source_root"] = str(source_root.resolve())
        if source_plan.get("compile_commands"):
            source_plan["compile_commands"] = str(
                (build_root / "compile_commands.json").resolve()
            )
        manifest_path.write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )


def provision(source_repo_root: Path) -> Path:
    stack = resolve_scientific_stack()
    source_repo_root = source_repo_root.resolve()
    source_seal = source_repo_root / "tmp" / NUMPY_WITNESS_SEAL_NAME
    attest_numpy_witness_seal(source_seal, stack=stack)
    destination = numpy_witness_seal_root(stack=stack)
    destination_parent = destination.parent
    destination_parent.mkdir(parents=True, exist_ok=True)
    staging = Path(
        tempfile.mkdtemp(prefix=f".{NUMPY_WITNESS_SEAL_NAME}-", dir=destination_parent)
    )
    try:
        _copy_tree(source_seal, staging / NUMPY_WITNESS_SEAL_NAME)
        _copy_tree(source_repo_root / "tmp" / BUILD_ROOT_NAME, staging / BUILD_ROOT_NAME)
        _copy_tree(source_repo_root / NUMPY_SOURCE_REL, staging / NUMPY_SOURCE_REL)
        _relocate_manifests(staging, destination_parent)
        staged_seal = staging / NUMPY_WITNESS_SEAL_NAME
        attest_numpy_witness_seal(staged_seal, stack=stack)
        if destination.exists():
            existing = attest_numpy_witness_seal(destination, stack=stack)
            print(
                f"already provisioned: configured={stack.numpy} "
                f"effective={existing} root={destination}"
            )
            return destination
        os.replace(staged_seal, destination)
        os.replace(staging / BUILD_ROOT_NAME, destination_parent / BUILD_ROOT_NAME)
        source_destination = destination_parent / NUMPY_SOURCE_REL
        source_destination.parent.mkdir(parents=True, exist_ok=True)
        os.replace(staging / NUMPY_SOURCE_REL, source_destination)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    effective = attest_numpy_witness_seal(destination, stack=stack)
    print(
        f"provisioned: configured={stack.numpy} effective={effective} root={destination}"
    )
    return destination


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Publish a genuine NumPy witness seal into version-keyed shared custody."
    )
    parser.add_argument("--source-repo-root", required=True, type=Path)
    args = parser.parse_args()
    try:
        provision(args.source_repo_root)
    except ValueError as exc:
        print(exc, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

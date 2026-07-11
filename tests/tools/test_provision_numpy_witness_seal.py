from __future__ import annotations

import json
from pathlib import Path

from tools.provision_numpy_witness_seal import (
    BUILD_ROOT_NAME,
    NUMPY_SOURCE_REL,
    NUMPY_WITNESS_SEAL_NAME,
    _relocate_manifests,
)


def test_relocate_manifests_moves_source_custody_to_version_store(
    tmp_path: Path,
) -> None:
    staging = tmp_path / "staging"
    destination_parent = tmp_path / "package-seals/numpy/2.5.1"
    seal_root = staging / NUMPY_WITNESS_SEAL_NAME
    seal_root.mkdir(parents=True)
    manifest_path = seal_root / "extension_manifest.json"
    manifest_path.write_text(
        json.dumps(
            {
                "source_plan": {
                    "build_root": "C:/stale/worktree/tmp/build",
                    "source_root": "C:/stale/worktree/numpy",
                    "compile_commands": "C:/stale/worktree/tmp/build/compile_commands.json",
                }
            }
        ),
        encoding="utf-8",
    )

    _relocate_manifests(staging, destination_parent)

    source_plan = json.loads(manifest_path.read_text(encoding="utf-8"))["source_plan"]
    assert source_plan == {
        "build_root": str((destination_parent / BUILD_ROOT_NAME).resolve()),
        "source_root": str((destination_parent / NUMPY_SOURCE_REL).resolve()),
        "compile_commands": str(
            (destination_parent / BUILD_ROOT_NAME / "compile_commands.json").resolve()
        ),
    }

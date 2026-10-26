from __future__ import annotations

import os
from pathlib import Path

from molt.cli.cargo_source_closure import _cargo_crate_source_closure
from molt.cli.runtime_features import RUNTIME_DEFAULT_FEATURE_MARKERS


_RUNTIME_FACADE_CRATE = Path("runtime/molt-runtime")


def _runtime_source_features(runtime_features: tuple[str, ...]) -> tuple[str, ...]:
    return tuple(
        sorted(
            {
                feature
                for feature in runtime_features
                if feature and feature not in RUNTIME_DEFAULT_FEATURE_MARKERS
            }
        )
    )


def runtime_source_paths(
    project_root: Path,
    runtime_features: tuple[str, ...] = (),
) -> tuple[Path, ...]:
    project_root = Path(os.path.normcase(os.path.realpath(project_root)))
    return tuple(
        _cargo_crate_source_closure(
            project_root=project_root,
            crate_root=project_root / _RUNTIME_FACADE_CRATE,
            crate_features=_runtime_source_features(runtime_features),
            # Crates declare the files outside them that their builds read
            # (package.metadata.molt.build-inputs); only the workspace's own
            # manifest and lock remain here.
            extra_source_paths=(
                project_root / "Cargo.toml",
                project_root / "Cargo.lock",
            ),
        )
    )


def wasm_cpython_abi_source_paths(project_root: Path) -> tuple[Path, ...]:
    """The standalone CAPI Cargo producer's declared file closure."""
    return tuple(
        _cargo_crate_source_closure(
            project_root=project_root,
            crate_root=project_root / "runtime" / "molt-cpython-abi",
            crate_features=(),
            extra_source_paths=(
                project_root / "Cargo.toml",
                project_root / "Cargo.lock",
            ),
        )
    )

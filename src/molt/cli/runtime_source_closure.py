from __future__ import annotations

import os
from pathlib import Path

from molt.cli.cargo_source_closure import _cargo_crate_source_closure


_RUNTIME_FACADE_CRATE = Path("runtime/molt-runtime")
_RUNTIME_SOURCE_FEATURE_MARKERS = frozenset({"default-features", "no-default-features"})


def _runtime_source_features(runtime_features: tuple[str, ...]) -> tuple[str, ...]:
    return tuple(
        sorted(
            {
                feature
                for feature in runtime_features
                if feature and feature not in _RUNTIME_SOURCE_FEATURE_MARKERS
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
            extra_source_paths=(
                project_root / "Cargo.toml",
                project_root / "Cargo.lock",
                project_root / "runtime/build_support",
                # Compiled by molt-runtime's sitebuiltins implementation.
                project_root / "LICENSE",
            ),
        )
    )

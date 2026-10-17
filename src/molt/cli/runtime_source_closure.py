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
            extra_source_paths=(
                project_root / "Cargo.toml",
                project_root / "Cargo.lock",
                project_root / "runtime/build_support",
                # Compiled by molt-runtime's sitebuiltins implementation.
                project_root / "LICENSE",
                # Build scripts compile the C API shims against these headers
                # (molt.h and shared/), and molt-runtime-constants includes the
                # WASM table layout; Cargo's dep-info lists all of them.
                project_root / "include/molt",
                project_root / "runtime/wasm_table_layout.inc",
                # molt-runtime compiles these through #[path] in every lane,
                # even where their own crates are not dependencies.
                project_root / "runtime/molt-runtime-http/src/functions_logging.rs",
                project_root / "runtime/molt-runtime-itertools/src/itertools.rs",
                project_root / "runtime/molt-runtime-path/src/os_ext.rs",
                project_root / "runtime/molt-runtime-path/src/pathlib.rs",
            ),
        )
    )

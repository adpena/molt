from __future__ import annotations

import functools
import hashlib
import os
from pathlib import Path

from molt.cli.cargo_source_closure import _cargo_crate_source_closure
from molt.file_hashing import _sha256_file


_RUNTIME_FACADE_CRATE = Path("runtime/molt-runtime")
_RUNTIME_SOURCE_FEATURE_MARKERS = frozenset({"default-features", "no-default-features"})


def _runtime_manifest_cache_stamp(project_root: Path) -> str:
    runtime_root = project_root / "runtime"
    manifests = [
        project_root / "Cargo.toml",
        project_root / "Cargo.lock",
        runtime_root / "Cargo.toml",
        runtime_root / "Cargo.lock",
    ]
    manifests.extend(sorted(runtime_root.glob("*/Cargo.toml")))
    digest = hashlib.sha256()
    for manifest in manifests:
        resolved = manifest.resolve(strict=False)
        try:
            label = resolved.relative_to(project_root).as_posix()
        except ValueError as exc:
            raise ValueError(
                f"runtime manifest escaped project root: {resolved}"
            ) from exc
        digest.update(label.encode("utf-8"))
        digest.update(b"\0")
        if resolved.is_file():
            digest.update(_sha256_file(resolved).encode("ascii"))
        else:
            digest.update(b"missing")
        digest.update(b"\0")
    return digest.hexdigest()


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


@functools.lru_cache(maxsize=256)
def _runtime_source_paths_cached(
    project_root_str: str,
    runtime_features: tuple[str, ...],
    manifest_cache_stamp: str,
) -> tuple[Path, ...]:
    del manifest_cache_stamp
    project_root = Path(project_root_str)
    return tuple(
        _cargo_crate_source_closure(
            project_root=project_root,
            crate_root=project_root / _RUNTIME_FACADE_CRATE,
            crate_features=runtime_features,
            extra_source_paths=(
                project_root / "Cargo.toml",
                project_root / "Cargo.lock",
                project_root / "runtime/Cargo.toml",
                project_root / "runtime/Cargo.lock",
                project_root / "runtime/build_support",
                # Compiled by molt-runtime's sitebuiltins implementation.
                project_root / "LICENSE",
                # `molt-runtime/build.rs` probes this closure unconditionally;
                # absence is identity-bearing and must remain in the path set.
                project_root / "third_party/cpython/Modules/_decimal/libmpdec",
            ),
        )
    )


def runtime_source_paths(
    project_root: Path,
    runtime_features: tuple[str, ...] = (),
) -> tuple[Path, ...]:
    project_root = Path(os.path.normcase(os.path.realpath(project_root)))
    normalized = _runtime_source_features(runtime_features)
    return _runtime_source_paths_cached(
        os.fspath(project_root),
        normalized,
        _runtime_manifest_cache_stamp(project_root),
    )

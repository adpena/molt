from __future__ import annotations

import functools
import os
import tomllib
from pathlib import Path
from typing import Any, Sequence


def _dedupe_source_paths(paths: Sequence[Path]) -> list[Path]:
    deduped: list[Path] = []
    seen: set[Path] = set()
    for path in paths:
        if path in seen:
            continue
        seen.add(path)
        deduped.append(path)
    return deduped


def _crate_source_paths(crate_root: Path) -> tuple[Path, Path, Path]:
    return (
        crate_root / "src",
        crate_root / "Cargo.toml",
        crate_root / "build.rs",
    )


def _cargo_manifest_stamp(manifest: Path) -> str:
    try:
        stat = manifest.stat()
    except OSError:
        return "missing"
    return f"{stat.st_size}:{stat.st_mtime_ns}:{stat.st_ctime_ns}"


@functools.lru_cache(maxsize=512)
def _read_cargo_manifest_cached(
    manifest_str: str,
    manifest_stamp: str,
) -> dict[str, Any]:
    manifest = Path(manifest_str)
    try:
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return {}
    return data if isinstance(data, dict) else {}


def _read_cargo_manifest(manifest: Path) -> dict[str, Any]:
    return _read_cargo_manifest_cached(
        os.fspath(manifest),
        _cargo_manifest_stamp(manifest),
    )


def _manifest_dependency_tables(data: dict[str, Any]) -> list[dict[str, Any]]:
    tables: list[dict[str, Any]] = []
    for key in ("dependencies", "build-dependencies"):
        table = data.get(key)
        if isinstance(table, dict):
            tables.append(table)
    target = data.get("target")
    if isinstance(target, dict):
        for target_table in target.values():
            if not isinstance(target_table, dict):
                continue
            for key in ("dependencies", "build-dependencies"):
                table = target_table.get(key)
                if isinstance(table, dict):
                    tables.append(table)
    return tables


def _local_path_dependencies(
    *,
    crate_root: Path,
    data: dict[str, Any],
    selected_optional_deps: set[str],
    child_features: dict[str, set[str]],
) -> list[tuple[str, Path, tuple[str, ...]]]:
    deps: list[tuple[str, Path, tuple[str, ...]]] = []
    for table in _manifest_dependency_tables(data):
        for dep_name, spec in table.items():
            if not isinstance(spec, dict):
                continue
            dep_path = spec.get("path")
            if not isinstance(dep_path, str) or not dep_path:
                continue
            optional = bool(spec.get("optional", False))
            if optional and dep_name not in selected_optional_deps:
                continue
            features = set(child_features.get(dep_name, set()))
            spec_features = spec.get("features", [])
            if isinstance(spec_features, list):
                features.update(
                    feature for feature in spec_features if isinstance(feature, str)
                )
            dep_root = (crate_root / dep_path).resolve()
            deps.append((dep_name, dep_root, tuple(sorted(features))))
    return deps


def _feature_dependency_selection(
    data: dict[str, Any],
    requested_features: tuple[str, ...],
) -> tuple[set[str], dict[str, set[str]]]:
    features = data.get("features")
    if not isinstance(features, dict):
        return set(), {}
    if requested_features:
        pending = list(requested_features)
    else:
        default_features = features.get("default", [])
        pending = [item for item in default_features if isinstance(item, str)]
    seen_features: set[str] = set()
    selected_optional_deps: set[str] = set()
    child_features: dict[str, set[str]] = {}
    while pending:
        feature = pending.pop()
        if feature in seen_features:
            continue
        seen_features.add(feature)
        entries = features.get(feature, [])
        if not isinstance(entries, list):
            continue
        for entry in entries:
            if not isinstance(entry, str) or not entry:
                continue
            if entry.startswith("dep:"):
                selected_optional_deps.add(entry[4:])
                continue
            if "/" in entry:
                dep_name, child_feature = entry.split("/", 1)
                dep_name = dep_name.removesuffix("?")
                child_feature = child_feature.removesuffix("?")
                if dep_name and child_feature:
                    selected_optional_deps.add(dep_name)
                    child_features.setdefault(dep_name, set()).add(child_feature)
                continue
            pending.append(entry)
    return selected_optional_deps, child_features


def _cargo_crate_source_closure(
    *,
    project_root: Path,
    crate_root: Path,
    crate_features: tuple[str, ...],
    extra_source_paths: Sequence[Path] = (),
) -> list[Path]:
    source_paths: list[Path] = []
    project_root_resolved = project_root.resolve()
    pending: list[tuple[Path, tuple[str, ...]]] = [(crate_root, crate_features)]
    seen: set[tuple[Path, tuple[str, ...]]] = set()
    while pending:
        current_root, current_features = pending.pop()
        key = (current_root, current_features)
        if key in seen:
            continue
        seen.add(key)
        source_paths.extend(_crate_source_paths(current_root))
        data = _read_cargo_manifest(current_root / "Cargo.toml")
        selected_optional_deps, child_features = _feature_dependency_selection(
            data, current_features
        )
        for _dep_name, dep_root, dep_features in _local_path_dependencies(
            crate_root=current_root,
            data=data,
            selected_optional_deps=selected_optional_deps,
            child_features=child_features,
        ):
            if (
                project_root_resolved in dep_root.parents
                or dep_root == project_root_resolved
            ):
                pending.append((dep_root, dep_features))
    source_paths.extend(extra_source_paths)
    return _dedupe_source_paths(source_paths)

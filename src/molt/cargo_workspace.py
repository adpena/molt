"""Read the root Cargo membership authority without launching Cargo.

These are declared workspace members, not a substitute for Cargo's dependency
resolver. Isolated workspaces and unrelated manifests are never discovered by a
recursive fallback. Malformed or missing input is an error, not an empty suite.
"""

from __future__ import annotations

import tomllib
from dataclasses import dataclass
from pathlib import Path, PureWindowsPath
from collections.abc import Callable
from typing import Any, Literal


def _read_manifest(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as stream:
            return tomllib.load(stream)
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(f"Cargo manifest {path}: {exc}") from exc


def _patterns(value: object, *, field: str, manifest: Path) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise ValueError(f"{manifest}: {field} must be a list of nonempty paths")
    patterns: list[str] = []
    for item in value:
        if not isinstance(item, str) or not item or item != item.strip():
            raise ValueError(f"{manifest}: {field} must be a list of nonempty paths")
        if (
            Path(item).is_absolute()
            or PureWindowsPath(item).anchor
            or ".." in Path(item).parts
            or "\\" in item
        ):
            raise ValueError(
                f"{manifest}: {field} path must be root-relative: {item!r}"
            )
        patterns.append(item)
    return tuple(patterns)


def workspace_member_manifests(project_root: Path) -> tuple[Path, ...]:
    """Return unique declared member manifests in deterministic manifest order."""
    root = project_root.resolve()
    manifest = root / "Cargo.toml"
    workspace = _read_manifest(manifest).get("workspace")
    if not isinstance(workspace, dict):
        raise ValueError(f"{manifest}: missing [workspace] authority")
    members = _patterns(
        workspace.get("members"), field="workspace.members", manifest=manifest
    )
    excludes = _patterns(
        workspace.get("exclude", []), field="workspace.exclude", manifest=manifest
    )
    result: dict[Path, None] = {}
    for pattern in members:
        candidates = (
            sorted(root.glob(pattern))
            if any(char in pattern for char in "*?[")
            else [root / pattern]
        )
        if not candidates:
            raise ValueError(
                f"{manifest}: workspace member pattern has no matches: {pattern!r}"
            )
        for candidate in candidates:
            relative = candidate.relative_to(root)
            if any(relative.match(exclude) for exclude in excludes):
                continue
            member = (candidate / "Cargo.toml").resolve()
            if not member.is_relative_to(root):
                raise ValueError(
                    f"{manifest}: workspace member escapes root: {candidate}"
                )
            if not member.is_file():
                raise ValueError(
                    f"{manifest}: workspace member manifest is missing: {member}"
                )
            result[member] = None
    return tuple(result)


def workspace_package_names(project_root: Path) -> tuple[str, ...]:
    """Return package identities, never directory aliases or partial results."""
    names: dict[str, Path] = {}
    for manifest in workspace_member_manifests(project_root):
        package = _read_manifest(manifest).get("package")
        name = package.get("name") if isinstance(package, dict) else None
        if not isinstance(name, str) or not name.strip():
            raise ValueError(f"{manifest}: missing package.name")
        if name in names:
            raise ValueError(
                f"{manifest}: duplicate package.name {name!r}; also in {names[name]}"
            )
        names[name] = manifest
    return tuple(names)


@dataclass(frozen=True)
class LocalCargoDependency:
    source_manifest: Path
    dependency_manifest: Path
    dependency_name: str
    kind: Literal["normal", "build", "dev"]
    target: str | None


@dataclass(frozen=True)
class LocalCargoManifestFacts:
    input_manifests: tuple[Path, ...]
    dependencies: tuple[LocalCargoDependency, ...]


def _table(value: object, field: str, manifest: Path) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValueError(f"{manifest}: {field} must be a table")
    result: dict[str, Any] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise ValueError(f"{manifest}: {field} keys must be strings")
        result[key] = item
    return result


def _local_manifest(value: object, base: Path, field: str) -> Path:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{base}: {field} must be a nonempty path")
    return (base.parent / value / "Cargo.toml").resolve()


def cargo_workspace_manifest(
    manifest: Path,
    load: Callable[[Path], dict[str, Any]] = _read_manifest,
    *,
    data: dict[str, Any] | None = None,
) -> Path | None:
    """Return the manifest of the workspace ``manifest`` belongs to.

    This is Cargo's rule: a manifest with ``[workspace]`` is its own root,
    ``package.workspace`` names the root, and otherwise the nearest ancestor
    manifest with ``[workspace]`` is. None means a standalone package. Cargo
    builds a workspace into ``<root>/target`` by default.
    """
    data = load(manifest) if data is None else data
    if "workspace" in data:
        _table(data["workspace"], "workspace", manifest)
        return manifest
    package = _table(data.get("package", {}), "package", manifest)
    if "workspace" in package:
        owner = _local_manifest(package["workspace"], manifest, "package.workspace")
        _table(load(owner).get("workspace"), "workspace", owner)
        return owner
    for directory in manifest.parent.parents:
        owner = directory / "Cargo.toml"
        if not owner.is_file():
            continue
        owner_data = load(owner)
        if "workspace" in owner_data:
            _table(owner_data["workspace"], "workspace", owner)
            return owner
    return None


def workspace_manifest_facts(project_root: Path) -> LocalCargoManifestFacts:
    """Collect conservative local inputs and declared edges, not resolved facts.

    Start with root and declared members, then follow explicit local dependency
    paths, including optional, dev, build, and every target's dependencies. Root
    patch/replace paths also participate. Exclusion from workspace membership
    does not exclude a dependency from lock custody. External local paths are
    valid inputs too; unrelated directories and isolated workspaces are never
    scanned. Patches/replacements add lock inputs, not package dependency edges;
    package inheritance also adds inputs without inventing dependency edges.
    Cargo still validates versions, features, and actual resolution.
    """
    root_manifest = project_root.resolve() / "Cargo.toml"
    documents: dict[Path, dict[str, Any]] = {}
    pending: list[Path] = []
    edges: dict[LocalCargoDependency, None] = {}

    def include(manifest: Path) -> dict[str, Any]:
        manifest = manifest.resolve()
        if manifest not in documents:
            documents[manifest] = _read_manifest(manifest)
            pending.append(manifest)
        return documents[manifest]

    def inheritance_workspace(manifest: Path, data: dict[str, Any]) -> Path:
        owner = cargo_workspace_manifest(manifest, include, data=data)
        if owner is None:
            raise ValueError(
                f"{manifest}: inherited dependency has no workspace authority"
            )
        return owner

    def dependencies(
        specs: object,
        manifest: Path,
        field: str,
        *,
        kind: Literal["normal", "build", "dev"] | None = None,
        target: str | None = None,
    ) -> None:
        for name, spec in _table(specs, field, manifest).items():
            if isinstance(spec, str):
                continue
            spec = _table(spec, f"{field}.{name}", manifest)
            base = manifest
            if "workspace" in spec:
                if spec["workspace"] is not True:
                    raise ValueError(
                        f"{manifest}: {field}.{name}.workspace must be true"
                    )
                base = inheritance_workspace(manifest, documents[manifest])
                shared = _table(
                    documents[base]["workspace"].get("dependencies", {}),
                    "workspace.dependencies",
                    base,
                )
                if name not in shared:
                    raise ValueError(f"{base}: missing workspace.dependencies.{name}")
                spec = shared[name]
                if isinstance(spec, str):
                    continue
                spec = _table(spec, f"workspace.dependencies.{name}", base)
                if "workspace" in spec:
                    raise ValueError(
                        f"{base}: workspace dependency cannot inherit itself: {name}"
                    )
            if "path" in spec:
                dependency = _local_manifest(spec["path"], base, f"{field}.{name}.path")
                include(dependency)
                if kind is not None:
                    edges[
                        LocalCargoDependency(manifest, dependency, name, kind, target)
                    ] = None

    include(root_manifest)
    for manifest in workspace_member_manifests(project_root):
        include(manifest)
    dependency_tables: dict[str, Literal["normal", "build", "dev"]] = {
        "dependencies": "normal",
        "build-dependencies": "build",
        "dev-dependencies": "dev",
        "build_dependencies": "build",
        "dev_dependencies": "dev",
    }
    index = 0
    while index < len(pending):
        manifest = pending[index]
        index += 1
        data = documents[manifest]
        package = _table(data.get("package", {}), "package", manifest)
        if "workspace" in package or any(
            isinstance(value, dict) and value.get("workspace") is True
            for value in package.values()
        ):
            # An external dependency may inherit its version (a lockfile input)
            # even when none of its dependencies use workspace inheritance.
            inheritance_workspace(manifest, data)
        for field, kind in dependency_tables.items():
            if field in data:
                dependencies(data[field], manifest, field, kind=kind)
        for target, target_data in _table(
            data.get("target", {}), "target", manifest
        ).items():
            target_data = _table(target_data, f"target.{target}", manifest)
            for field, kind in dependency_tables.items():
                if field in target_data:
                    dependencies(
                        target_data[field],
                        manifest,
                        f"target.{target}.{field}",
                        kind=kind,
                        target=target,
                    )
        # Cargo honors patches/replacements only at the workspace being resolved.
        if manifest == root_manifest:
            for source, specs in _table(
                data.get("patch", {}), "patch", manifest
            ).items():
                dependencies(specs, manifest, f"patch.{source}")
            if "replace" in data:
                dependencies(data["replace"], manifest, "replace")
    return LocalCargoManifestFacts(tuple(pending), tuple(edges))

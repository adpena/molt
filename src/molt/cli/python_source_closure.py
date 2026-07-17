"""Deterministic local Python import closure for executable tooling inputs."""

from __future__ import annotations

import ast
import hashlib
import json
from collections.abc import Iterable
from pathlib import Path
import tomllib

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.python_import_resolution import (
    LocalPythonModuleResolver,
    PythonImportPolicy,
    local_import_targets,
    resolve_local_import_targets,
)


_EXECUTABLE_TOOL_IMPORT_POLICY = PythonImportPolicy(
    module_level_only=False,
    include_parent_packages=True,
    fail_on_nonliteral_dynamic_import=True,
)
_DYNAMIC_IMPORT_MANIFEST = Path("src/molt/cli/python_source_closure.toml")
_GRAPH_CACHE_SCHEMA_VERSION = 3
_GRAPH_CACHE_RELPATH = Path(".molt_cache/python_source_closure_graph.json")


def _source_digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def _read_graph_cache(project_root: Path) -> dict[str, dict[str, object]]:
    cache_path = project_root / _GRAPH_CACHE_RELPATH
    try:
        payload = json.loads(cache_path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    if not isinstance(payload, dict):
        return {}
    if payload.get("schema_version") != _GRAPH_CACHE_SCHEMA_VERSION:
        return {}
    entries = payload.get("entries")
    if not isinstance(entries, dict):
        return {}
    return {
        key: value
        for key, value in entries.items()
        if isinstance(key, str) and isinstance(value, dict)
    }


def _write_graph_cache(
    project_root: Path,
    entries: dict[str, dict[str, object]],
) -> None:
    cache_path = project_root / _GRAPH_CACHE_RELPATH
    payload = {
        "schema_version": _GRAPH_CACHE_SCHEMA_VERSION,
        "entries": entries,
    }
    try:
        _atomic_write_text(
            cache_path,
            json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n",
        )
    except OSError:
        return


def _relative_cache_key(project_root: Path, source: Path) -> str:
    try:
        return source.resolve().relative_to(project_root).as_posix()
    except ValueError as exc:
        raise ValueError(
            f"Python tooling source is outside project root: {source}"
        ) from exc


def _dynamic_contract_digest(
    expected: int | None,
    targets: tuple[str, ...],
) -> str:
    payload = json.dumps(
        {"expected": expected, "targets": targets},
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _module_name_for_path(path: Path, resolver: LocalPythonModuleResolver) -> str:
    module, _package = resolver.module_identity(path)
    return module


def _molt_cli_lazy_targets(
    source: Path,
    resolver: LocalPythonModuleResolver,
) -> set[str]:
    """Derive finite ``molt.cli`` lazy imports from their source authorities."""

    tree = resolver.read_ast(source)
    targets: set[str] = set()
    registry_found = False
    for node in tree.body:
        value = None
        if (
            isinstance(node, ast.AnnAssign)
            and isinstance(node.target, ast.Name)
            and node.target.id == "_LAZY_REEXPORTS"
        ):
            value = node.value
        elif isinstance(node, ast.Assign) and any(
            isinstance(target, ast.Name) and target.id == "_LAZY_REEXPORTS"
            for target in node.targets
        ):
            value = node.value
        if value is not None:
            if not isinstance(value, ast.Dict):
                raise ValueError(
                    f"molt.cli lazy reexport registry is not a literal dict: {source}"
                )
            registry_found = True
            for registry_value in value.values:
                if (
                    not isinstance(
                        registry_value,
                        (ast.Tuple, ast.List),
                    )
                    or not registry_value.elts
                ):
                    raise ValueError(f"invalid molt.cli lazy reexport row in {source}")
                module_node = registry_value.elts[0]
                if not (
                    isinstance(module_node, ast.Constant)
                    and isinstance(module_node.value, str)
                ):
                    raise ValueError(
                        f"non-literal molt.cli lazy reexport module in {source}"
                    )
                targets.add(f"molt.cli.{module_node.value}")
        assignment_value = None
        if isinstance(node, ast.Assign):
            assignment_value = node.value
        elif isinstance(node, ast.AnnAssign):
            assignment_value = node.value
        if (
            isinstance(assignment_value, ast.Call)
            and isinstance(assignment_value.func, ast.Name)
            and assignment_value.func.id == "_LazyPostLoweringModule"
            and assignment_value.args
        ):
            module_node = assignment_value.args[0]
            if not (
                isinstance(module_node, ast.Constant)
                and isinstance(module_node.value, str)
            ):
                raise ValueError(f"non-literal molt.cli lazy proxy module in {source}")
            targets.add(f"molt.cli.{module_node.value}")
    if not registry_found:
        raise ValueError(f"molt.cli lazy reexport registry is missing: {source}")
    return targets


def _read_dynamic_import_manifest(
    project_root: Path,
    resolver: LocalPythonModuleResolver,
) -> tuple[Path | None, dict[Path, tuple[int, tuple[str, ...]]]]:
    manifest = project_root / _DYNAMIC_IMPORT_MANIFEST
    if not manifest.is_file():
        return None, {}
    try:
        payload = tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise ValueError(
            f"cannot read Python tooling import manifest {manifest}: {exc}"
        ) from exc
    if payload.get("schema_version") != 1:
        raise ValueError(
            f"unsupported Python tooling import manifest schema: {manifest}"
        )
    rows = payload.get("source")
    if not isinstance(rows, list):
        raise ValueError(
            f"Python tooling import manifest has no source rows: {manifest}"
        )
    overrides: dict[Path, tuple[int, tuple[str, ...]]] = {}
    for row in rows:
        if not isinstance(row, dict):
            raise ValueError(f"invalid Python tooling import manifest row: {manifest}")
        source_value = row.get("path")
        expected = row.get("nonliteral_calls")
        modules = row.get("modules", [])
        module_trees = row.get("module_trees", [])
        derive_molt_cli_lazy_targets = row.get("derive_molt_cli_lazy_targets", False)
        if (
            not isinstance(source_value, str)
            or not isinstance(expected, int)
            or expected < 0
            or not isinstance(modules, list)
            or not all(isinstance(module, str) for module in modules)
            or not isinstance(module_trees, list)
            or not all(isinstance(module, str) for module in module_trees)
            or not isinstance(derive_molt_cli_lazy_targets, bool)
        ):
            raise ValueError(f"invalid Python tooling import manifest row: {manifest}")
        source = (project_root / source_value).resolve()
        resolver.module_identity(source)
        if source in overrides:
            raise ValueError(f"duplicate Python tooling import manifest row: {source}")
        targets = set(modules)
        for module_tree in module_trees:
            tree_source = resolver.source_for_module(module_tree)
            if tree_source is None or tree_source.name != "__init__.py":
                raise ValueError(
                    f"dynamic import module tree is not a local package: {module_tree}"
                )
            package_dir = tree_source.parent
            targets.add(module_tree)
            for child in package_dir.rglob("*.py"):
                targets.add(_module_name_for_path(child, resolver))
        if derive_molt_cli_lazy_targets:
            targets.update(_molt_cli_lazy_targets(source, resolver))
        overrides[source] = (expected, tuple(sorted(targets)))
    return manifest.resolve(), overrides


def local_python_import_closure(
    project_root: Path,
    seeds: Iterable[Path],
) -> tuple[Path, ...]:
    """Return every local Python source transitively imported by *seeds*.

    Both ``tools`` top-level modules and ``src`` packages are resolved.  Syntax
    errors and roots outside those authorities fail closed so a linker cache can
    never reuse output after an untracked tooling change.
    """

    root = project_root.resolve()
    search_roots = tuple(
        candidate.resolve()
        for candidate in (root / "tools", root / "src")
        if candidate.is_dir()
    )
    if not search_roots:
        raise ValueError(f"project has no local Python source roots: {root}")
    resolver = LocalPythonModuleResolver(search_roots)
    manifest, dynamic_import_overrides = _read_dynamic_import_manifest(root, resolver)
    cached_entries = _read_graph_cache(root)
    next_entries: dict[str, dict[str, object]] = {}
    pending = [seed.resolve() for seed in seeds]
    reached: set[Path] = set()
    while pending:
        source = pending.pop()
        if source in reached:
            continue
        if not source.is_file():
            raise FileNotFoundError(f"missing Python tooling source: {source}")
        resolver.module_identity(source)
        reached.add(source)
        try:
            expected_dynamic_imports, dynamic_targets = dynamic_import_overrides.get(
                source,
                (None, ()),
            )
            cache_key = _relative_cache_key(root, source)
            source_digest = _source_digest(source)
            contract_digest = _dynamic_contract_digest(
                expected_dynamic_imports,
                dynamic_targets,
            )
            cached = cached_entries.get(cache_key)
            cached_targets = cached.get("targets") if cached is not None else None
            if (
                cached is not None
                and cached.get("source_sha256") == source_digest
                and cached.get("dynamic_contract_sha256") == contract_digest
                and isinstance(cached_targets, list)
                and all(isinstance(target, str) for target in cached_targets)
            ):
                targets = {
                    target for target in cached_targets if isinstance(target, str)
                }
            else:
                targets = local_import_targets(
                    source,
                    resolver,
                    _EXECUTABLE_TOOL_IMPORT_POLICY,
                    expected_nonliteral_dynamic_imports=expected_dynamic_imports,
                    nonliteral_dynamic_import_targets=dynamic_targets,
                )
            next_entries[cache_key] = {
                "source_sha256": source_digest,
                "dynamic_contract_sha256": contract_digest,
                "targets": sorted(targets),
            }
            dependencies = resolve_local_import_targets(
                targets,
                resolver,
                _EXECUTABLE_TOOL_IMPORT_POLICY,
            )
        except ValueError as exc:
            raise ValueError(
                f"cannot derive Python tooling import closure for {source}: {exc}"
            ) from exc
        for dependency in dependencies:
            if dependency not in reached:
                pending.append(dependency)
    if manifest is not None:
        reached.add(manifest)
    _write_graph_cache(root, next_entries)
    return tuple(sorted(reached, key=lambda path: path.as_posix()))

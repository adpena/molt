"""One policy-keyed local Python dependency graph for compiler and tool inputs."""

from __future__ import annotations

import ast
import hashlib
import json
from collections.abc import Callable, Iterable, Iterator
from contextlib import contextmanager
from contextvars import ContextVar
from dataclasses import asdict
from pathlib import Path
import tomllib
from typing import Literal, cast

from molt.cli.atomic_io import _atomic_write_text
from molt.cli.python_import_resolution import (
    LocalPythonModuleResolver,
    LocalPythonModuleSource,
    LocalPythonImportAnalysis,
    LocalPythonImportDiagnostic,
    LocalPythonImportRequest,
    PythonImportPolicy,
    PythonSourceSnapshot,
    analyze_local_imports,
    local_import_analysis_identity,
    resolve_local_import_requests,
    relative_python_module_name,
)


_EXECUTABLE_TOOL_IMPORT_POLICY = PythonImportPolicy(
    module_level_only=False,
    include_parent_packages=True,
    fail_on_nonliteral_dynamic_import=True,
)
_DYNAMIC_IMPORT_MANIFEST = Path("src/molt/cli/python_source_closure.toml")
_GRAPH_CACHE_SCHEMA_VERSION = 5
_GRAPH_CACHE_RELPATH = Path(".molt_cache/python_source_closure_graph.json")
_GraphQuery = tuple[Path, tuple[Path, ...], tuple[Path, ...], PythonImportPolicy]
_GRAPH_TRANSACTION: ContextVar[dict[_GraphQuery, tuple[Path, ...]] | None] = ContextVar(
    "_GRAPH_TRANSACTION", default=None
)


@contextmanager
def local_python_import_graph_transaction() -> Iterator[None]:
    """Reuse immutable tooling closure queries only within one build command."""
    if _GRAPH_TRANSACTION.get() is not None:
        yield
        return
    previous_context = _GRAPH_TRANSACTION.set({})
    try:
        yield
    finally:
        _GRAPH_TRANSACTION.reset(previous_context)


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
    retained: dict[str, dict[str, object]] = {}
    for key, value in entries.items():
        if not isinstance(key, str) or not isinstance(value, dict):
            continue
        try:
            source = project_root / key
            if (
                Path(key).is_absolute()
                or source.resolve().relative_to(project_root).as_posix() != key
                or not source.is_file()
            ):
                continue
        except (OSError, ValueError):
            continue
        retained[key] = value
    return retained


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


def _analysis_policy_digest(
    module: str,
    is_package: bool,
    policy: PythonImportPolicy,
) -> str:
    payload = json.dumps(
        {
            "module": module,
            "is_package": is_package,
            "policy": asdict(policy),
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _dynamic_contract_digest(expected: int | None, targets: tuple[str, ...]) -> str:
    payload = json.dumps({"expected": expected, "targets": targets}, sort_keys=True)
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _analysis_payload(analysis: LocalPythonImportAnalysis) -> dict[str, object]:
    return {
        "requests": [asdict(request) for request in analysis.requests],
        "unresolved_dynamic_imports": [
            asdict(diagnostic) for diagnostic in analysis.unresolved_dynamic_imports
        ],
    }


def _cached_analysis(
    value: object, source_digest: str, contract_digest: str, analysis_digest: str
) -> LocalPythonImportAnalysis | None:
    if (
        not isinstance(value, dict)
        or value.get("source_sha256") != source_digest
        or value.get("dynamic_contract_sha256") != contract_digest
        or value.get("analysis_authority_sha256") != analysis_digest
    ):
        return None
    rows = value.get("requests")
    diagnostics = value.get("unresolved_dynamic_imports")
    if not isinstance(rows, list) or not isinstance(diagnostics, list):
        return None
    requests: list[LocalPythonImportRequest] = []
    unresolved: list[LocalPythonImportDiagnostic] = []
    for row in rows:
        if not isinstance(row, dict):
            return None
        kind, candidates = row.get("kind"), row.get("candidates")
        line, column = row.get("line"), row.get("column")
        if (
            kind not in ("direct", "from", "dynamic", "manifest")
            or not isinstance(candidates, list)
            or not candidates
            or not all(isinstance(target, str) and target for target in candidates)
            or type(line) is not int
            or line < 0
            or type(column) is not int
            or column < 0
        ):
            return None
        requests.append(
            LocalPythonImportRequest(
                cast(Literal["direct", "from", "dynamic", "manifest"], kind),
                tuple(cast(list[str], candidates)),
                line,
                column,
            )
        )
    for row in diagnostics:
        if not isinstance(row, dict):
            return None
        line, column, message = row.get("line"), row.get("column"), row.get("message")
        if (
            type(line) is not int
            or line < 0
            or type(column) is not int
            or column < 0
            or not isinstance(message, str)
            or not message
        ):
            return None
        unresolved.append(LocalPythonImportDiagnostic(line, column, message))
    return LocalPythonImportAnalysis(tuple(requests), tuple(unresolved))


def _molt_cli_lazy_targets(
    snapshot: PythonSourceSnapshot,
) -> set[str]:
    """Derive finite ``molt.cli`` lazy imports from their source authorities."""

    source, tree = snapshot.path, snapshot.tree
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
    capture: Callable[[Path], PythonSourceSnapshot],
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
                member = relative_python_module_name(child, package_dir)
                targets.add(f"{module_tree}.{member}" if member else module_tree)
        if derive_molt_cli_lazy_targets:
            targets.update(_molt_cli_lazy_targets(capture(source)))
        overrides[source] = (expected, tuple(sorted(targets)))
    return manifest.resolve(), overrides


def local_python_import_closure(
    project_root: Path,
    seeds: Iterable[Path],
    *,
    policy: PythonImportPolicy = _EXECUTABLE_TOOL_IMPORT_POLICY,
    search_roots: tuple[Path, ...] | None = None,
) -> tuple[Path, ...]:
    """Return the policy projection of one source-byte-keyed dependency graph.

    Seeds may name files or whole Python source directories. Executable tools
    default to ``tools``/``src``/repository search order, full lexical imports,
    parent-package execution and checked dynamic manifests. Lowering supplies its existing
    module-level-only policy and ``src`` root. No failed analysis becomes an
    empty closure. Cached grouped requests always resolve against fresh topology
    outside the explicit build transaction, including previously missing members.
    """

    root = project_root.resolve()
    roots = tuple(
        candidate.resolve()
        for candidate in (
            search_roots
            if search_roots is not None
            else (root / "tools", root / "src", root)
        )
        if candidate.is_dir()
    )
    if not roots:
        raise ValueError(f"project has no local Python source roots: {root}")
    seed_paths = tuple(sorted({seed.resolve() for seed in seeds}))
    if any(not path.is_relative_to(root) for path in (*roots, *seed_paths)):
        raise ValueError(f"Python tooling source is outside project root: {root}")
    query = (root, seed_paths, roots, policy)
    transaction = _GRAPH_TRANSACTION.get()
    if transaction is not None and query in transaction:
        return transaction[query]
    resolver = LocalPythonModuleResolver(roots)
    snapshots: dict[Path, PythonSourceSnapshot] = {}

    def capture(path: Path) -> PythonSourceSnapshot:
        if path not in snapshots:
            snapshots[path] = resolver.capture_source(path)
        return snapshots[path]

    manifest, dynamic_import_overrides = (
        _read_dynamic_import_manifest(root, resolver, capture)
        if not policy.module_level_only
        else (None, {})
    )
    cached_entries = _read_graph_cache(root)
    analysis_digest = hashlib.sha256(
        json.dumps(local_import_analysis_identity(), sort_keys=True).encode("utf-8")
    ).hexdigest()
    # Keep records from sibling policies/seeds. Each source/module/policy variant
    # replaces its byte generation; aliases share a snapshot, never an execution
    # context. Merging next_entries below retains every alias seen in this walk.
    next_entries = dict(cached_entries)
    pending: list[LocalPythonModuleSource] = []
    for seed in seed_paths:
        for path in seed.rglob("*.py") if seed.is_dir() else (seed,):
            path = path.resolve()
            pending.append(
                LocalPythonModuleSource(resolver.module_identity(path)[0], path)
            )
    visited: set[LocalPythonModuleSource] = set()
    reached: set[Path] = set()
    while pending:
        module_source = pending.pop()
        if module_source in visited:
            continue
        source = module_source.path
        if not source.is_file():
            raise FileNotFoundError(f"missing Python tooling source: {source}")
        visited.add(module_source)
        module = module_source.name
        reached.add(source)
        try:
            expected_dynamic_imports, dynamic_targets = dynamic_import_overrides.get(
                source,
                (None, ()),
            )
            cache_key = _relative_cache_key(root, source)
            snapshot = capture(source)
            policy_digest = _analysis_policy_digest(
                module,
                source.name == "__init__.py",
                policy,
            )
            contract_digest = _dynamic_contract_digest(
                expected_dynamic_imports, dynamic_targets
            )
            variants = dict(next_entries.get(cache_key, {}))
            analysis = _cached_analysis(
                variants.get(policy_digest),
                snapshot.sha256,
                contract_digest,
                analysis_digest,
            )
            if analysis is None:
                analysis = analyze_local_imports(
                    snapshot,
                    module_source,
                    policy,
                    expected_nonliteral_dynamic_imports=expected_dynamic_imports,
                    nonliteral_dynamic_import_targets=dynamic_targets,
                )
            # Reapply the contract even on a cache hit; accepted unresolved sites
            # are retained, never silently converted into a complete analysis.
            analysis.validate_dynamic_contract(source, policy, expected_dynamic_imports)
            variants[policy_digest] = {
                "source_sha256": snapshot.sha256,
                "dynamic_contract_sha256": contract_digest,
                "analysis_authority_sha256": analysis_digest,
                **_analysis_payload(analysis),
            }
            next_entries[cache_key] = variants
            dependencies = resolve_local_import_requests(
                analysis,
                resolver,
                policy,
            )
        except ValueError as exc:
            raise ValueError(
                f"cannot derive Python tooling import closure for {source}: {exc}"
            ) from exc
        for dependency in dependencies:
            if dependency not in visited:
                pending.append(dependency)
    if manifest is not None:
        reached.add(manifest)
    _write_graph_cache(root, next_entries)
    result = tuple(sorted(reached, key=lambda path: path.as_posix()))
    if transaction is not None:
        transaction[query] = result
    return result

from __future__ import annotations

import ast
import codecs
from collections import deque
import contextlib
import functools
import hashlib
import json
import os
import re
import sys
import tokenize
from collections.abc import Collection, Iterable, Mapping, MutableMapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from types import MappingProxyType
from typing import Any, cast

from molt._runtime_feature_gates import link_affecting_feature_gate_for_symbol
from molt.cli.artifact_state import _build_state_subdir_cached
from molt.cli.atomic_io import _atomic_write_text, _write_text_if_changed
from molt.cli.backend_cache import (
    _read_artifact_sync_state,
    _write_artifact_sync_payload,
)
from molt.cli.cache_fingerprints import _cache_tooling_fingerprint
from molt.cli.compiler_metadata import _compiler_root
from molt.cli.config_resolution import STATIC_IMPORT_MODULES_ENV
from molt.cli.default_paths import _default_molt_cache
from molt.cli.file_hashing import _sha256_file
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli.models import (
    ImportScanMode,
    _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN,
    _ImportAdmissionPolicy,
    _ImportPlan,
    _ModuleGraphAugmentation,
    _ModuleGraphMetadata,
    _PersistedModuleGraphState,
    _PreparedEntryModuleGraph,
    _RuntimeImportSupportPolicy,
    _SupportModuleAugmentation,
)
from molt.cli.output import CliFailure as _CliFailure
from molt.cli.output import fail as _fail
from molt.cli.runtime_features import _runtime_builtin_features_for_profile
from molt.cli.runtime_paths import _build_state_root
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_source_for_target,
)
from molt.cli.toolchain_validation import _is_path_within


STUB_MODULES = {"molt_buffer", "molt_cbor", "molt_json", "molt_msgpack"}


STUB_PARENT_MODULES = {"molt"}


# Modules whose function-body imports are part of the required static module
# graph.  The default set is limited to stdlib modules with proven module-init
# semantics; third-party lazy backend families stay runtime import obligations
# unless they are entry modules or explicitly admitted here.
STDLIB_NESTED_IMPORT_SCAN_MODULES = {
    "collections",
    "typing",
    # EmailMessage lazily imports email.policy inside __init__.
    "email.message",
}


# Submodule prefixes excluded from the module graph because they target
# platforms that Molt does not support (e.g. Emscripten/Pyodide).  The import
# scanner still discovers them but the graph walker skips any candidate whose
# dotted name starts with one of these prefixes.
PLATFORM_EXCLUDED_SUBMODULES = ("urllib3.contrib.emscripten",)


ENTRY_OVERRIDE_SPAWN = "multiprocessing.spawn"


IMPORTER_MODULE_NAME = "_molt_importer"


_RUNTIME_IMPORT_PROTOCOL_MARKERS = (
    "import ",
    "from ",
    "__import__",
    "import_module",
    "find_spec",
)


_RUNTIME_IMPORT_PROTOCOL_TARGETS = frozenset(
    {
        "__import__",
        "builtins.__import__",
        "importlib.import_module",
        "importlib.util.find_spec",
    }
)


_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES = (
    "importlib",
    "importlib.util",
    "importlib.machinery",
)


_RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES = frozenset(
    {
        "builtins",
        "_intrinsics",
        *_RUNTIME_IMPORT_SUPPORT_ROOT_MODULES,
        "importlib.abc",
        IMPORTER_MODULE_NAME,
    }
)


@dataclass(frozen=True)
class _ModuleSourceLease:
    path: Path
    inline_source: str | None = None
    source_size: int | None = None
    mtime_ns: int | None = None

    @classmethod
    def path_backed(
        cls, path: Path, path_stat: os.stat_result | None = None
    ) -> "_ModuleSourceLease":
        if path_stat is None:
            with contextlib.suppress(OSError):
                path_stat = path.stat()
        return cls(
            path=path,
            inline_source=None,
            source_size=path_stat.st_size if path_stat is not None else None,
            mtime_ns=path_stat.st_mtime_ns if path_stat is not None else None,
        )

    @classmethod
    def inline(
        cls,
        path: Path,
        source: str,
        path_stat: os.stat_result | None = None,
    ) -> "_ModuleSourceLease":
        return cls(
            path=path,
            inline_source=source,
            source_size=len(source),
            mtime_ns=path_stat.st_mtime_ns if path_stat is not None else None,
        )

    @property
    def path_backed_source(self) -> bool:
        return self.inline_source is None

    def read(self, resolution_cache: "_ModuleResolutionCache | None" = None) -> str:
        if self.inline_source is not None:
            return self.inline_source
        if self.source_size is not None or self.mtime_ns is not None:
            stat = self.path.stat()
            if self.source_size is not None and stat.st_size != self.source_size:
                raise OSError(
                    f"Source lease for {self.path} changed size during compile"
                )
            if self.mtime_ns is not None and stat.st_mtime_ns != self.mtime_ns:
                raise OSError(
                    f"Source lease for {self.path} changed mtime during compile"
                )
        if resolution_cache is not None:
            return resolution_cache.read_module_source(self.path, retain=False)
        return _read_module_source(self.path)

    def worker_payload(self) -> dict[str, Any]:
        if self.inline_source is not None:
            return {
                "kind": "inline",
                "path": str(self.path),
                "source": self.inline_source,
                "source_size": self.source_size,
                "mtime_ns": self.mtime_ns,
            }
        return {
            "kind": "path",
            "path": str(self.path),
            "source_size": self.source_size,
            "mtime_ns": self.mtime_ns,
        }


@dataclass(frozen=True)
class _ModuleSourceCatalog:
    leases: Mapping[str, _ModuleSourceLease]

    def lease_for(self, module_name: str, module_path: Path) -> _ModuleSourceLease:
        lease = self.leases.get(module_name)
        if lease is not None:
            return lease
        return _ModuleSourceLease.path_backed(module_path)

    def source_size(self, module_name: str, module_path: Path | None = None) -> int:
        lease = self.leases.get(module_name)
        if lease is not None and lease.source_size is not None:
            return lease.source_size
        if module_path is not None:
            with contextlib.suppress(OSError):
                return module_path.stat().st_size
        return 0

    def read_source(
        self,
        module_name: str,
        module_path: Path,
        resolution_cache: "_ModuleResolutionCache | None" = None,
    ) -> str:
        return self.lease_for(module_name, module_path).read(resolution_cache)

    def worker_source_lease_payload(
        self, module_name: str, module_path: Path
    ) -> dict[str, Any]:
        return self.lease_for(module_name, module_path).worker_payload()


def _stat_ctime_ns(stat: os.stat_result) -> int:
    ctime_ns = getattr(stat, "st_ctime_ns", None)
    if isinstance(ctime_ns, int):
        return ctime_ns
    return int(stat.st_ctime * 1_000_000_000)


def _stat_device(stat: os.stat_result) -> int:
    return int(getattr(stat, "st_dev", 0) or 0)


_SOURCE_HASH_CACHE_SCHEMA_VERSION = 1


def _source_hash_stat_identity_is_strong(
    *,
    ctime_ns: int,
    inode: int,
    device: int,
) -> bool:
    if sys.platform.startswith("win"):
        return False
    return ctime_ns > 0 and inode > 0 and device >= 0


@functools.lru_cache(maxsize=16384)
def _source_hash_cache_path_cached(
    cache_root_str: str,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
) -> Path:
    identity = {
        "path": path_str,
        "size": size,
        "mtime_ns": mtime_ns,
        "ctime_ns": ctime_ns,
        "inode": inode,
        "device": device,
    }
    encoded = json.dumps(identity, sort_keys=True, separators=(",", ":")).encode(
        "utf-8"
    )
    digest = hashlib.sha256(encoded).hexdigest()
    return Path(cache_root_str) / "source_hash_cache" / digest[:2] / f"{digest}.json"


def _source_hash_cache_path(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
) -> Path:
    return _source_hash_cache_path_cached(
        os.fspath(cache_root),
        path_str,
        size,
        mtime_ns,
        ctime_ns,
        inode,
        device,
    )


def _read_source_hash_cache_payload(cache_path: Path) -> dict[str, Any] | None:
    try:
        data = json.loads(cache_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    return data if isinstance(data, dict) else None


def _write_source_hash_cache_payload(
    cache_path: Path,
    payload: dict[str, Any],
) -> None:
    try:
        _atomic_write_text(cache_path, json.dumps(payload, sort_keys=True) + "\n")
    except OSError:
        return


def _read_persistent_source_hash(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
) -> str | None:
    if not _source_hash_stat_identity_is_strong(
        ctime_ns=ctime_ns, inode=inode, device=device
    ):
        return None
    cache_path = _source_hash_cache_path(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        device=device,
    )
    payload = _read_source_hash_cache_payload(cache_path)
    if (
        not isinstance(payload, dict)
        or payload.get("version") != _SOURCE_HASH_CACHE_SCHEMA_VERSION
        or payload.get("path") != path_str
        or payload.get("size") != size
        or payload.get("mtime_ns") != mtime_ns
        or payload.get("ctime_ns") != ctime_ns
        or payload.get("inode") != inode
        or payload.get("device") != device
    ):
        return None
    source_hash = payload.get("source_sha256")
    return source_hash if isinstance(source_hash, str) and source_hash else None


def _write_persistent_source_hash(
    cache_root: Path,
    *,
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
    source_hash: str,
) -> None:
    if not _source_hash_stat_identity_is_strong(
        ctime_ns=ctime_ns, inode=inode, device=device
    ):
        return
    cache_path = _source_hash_cache_path(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        device=device,
    )
    payload = {
        "version": _SOURCE_HASH_CACHE_SCHEMA_VERSION,
        "path": path_str,
        "size": size,
        "mtime_ns": mtime_ns,
        "ctime_ns": ctime_ns,
        "inode": inode,
        "device": device,
        "source_sha256": source_hash,
    }
    _write_source_hash_cache_payload(cache_path, payload)


@functools.lru_cache(maxsize=16384)
def _source_content_sha256_cached(
    path_str: str,
    size: int,
    mtime_ns: int,
    ctime_ns: int,
    inode: int,
    device: int,
    cache_root_str: str,
) -> str | None:
    cache_root = Path(cache_root_str)
    cached_hash = _read_persistent_source_hash(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        device=device,
    )
    if cached_hash is not None:
        return cached_hash
    try:
        source_hash = _sha256_file(Path(path_str))
    except OSError:
        return None
    _write_persistent_source_hash(
        cache_root,
        path_str=path_str,
        size=size,
        mtime_ns=mtime_ns,
        ctime_ns=ctime_ns,
        inode=inode,
        device=device,
        source_hash=source_hash,
    )
    return source_hash


def _source_content_sha256(
    path: Path,
    path_stat: os.stat_result | None = None,
) -> str | None:
    if path_stat is None:
        try:
            path_stat = path.stat()
        except OSError:
            return None
    try:
        path_str = os.fspath(path.resolve())
    except OSError:
        path_str = os.fspath(path)
    ctime_ns = _stat_ctime_ns(path_stat)
    inode = int(getattr(path_stat, "st_ino", 0) or 0)
    device = _stat_device(path_stat)
    if not _source_hash_stat_identity_is_strong(
        ctime_ns=ctime_ns, inode=inode, device=device
    ):
        try:
            return _sha256_file(Path(path_str))
        except OSError:
            return None
    return _source_content_sha256_cached(
        path_str,
        path_stat.st_size,
        path_stat.st_mtime_ns,
        ctime_ns,
        inode,
        device,
        os.fspath(_default_molt_cache()),
    )


def _payload_source_matches(
    payload: Mapping[str, Any],
    path: Path,
    path_stat: os.stat_result,
) -> bool:
    expected_hash = payload.get("source_sha256")
    if not isinstance(expected_hash, str) or not expected_hash:
        return False
    if (
        payload.get("size") != path_stat.st_size
        or payload.get("mtime_ns") != path_stat.st_mtime_ns
    ):
        return False
    return _source_content_sha256(path, path_stat) == expected_hash


def _module_name_from_path(path: Path, roots: list[Path], stdlib_root: Path) -> str:
    resolved = path.resolve()
    resolved_roots = tuple(root.resolve() for root in roots)
    resolved_stdlib_root = stdlib_root.resolve()
    cpython_test_root: Path | None = None
    cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
    if cpython_dir:
        cpython_test_root = (Path(cpython_dir) / "Lib" / "test").resolve()
    return _module_name_from_resolved_path(
        resolved,
        resolved_roots=resolved_roots,
        resolved_stdlib_root=resolved_stdlib_root,
        cpython_test_root=cpython_test_root,
    )


def _entry_module_root_for_path(path: Path) -> Path:
    resolved = path.resolve()
    package_dir = resolved.parent
    topmost_package_parent = package_dir
    while (package_dir / "__init__.py").exists():
        topmost_package_parent = package_dir.parent
        if topmost_package_parent == package_dir:
            break
        package_dir = topmost_package_parent
    return topmost_package_parent


def _module_name_from_resolved_path(
    resolved: Path,
    *,
    resolved_roots: tuple[Path, ...],
    resolved_stdlib_root: Path,
    cpython_test_root: Path | None,
) -> str:
    resolved_parts = resolved.parts
    rel_parts = _relative_parts_if_within(resolved_parts, resolved_stdlib_root.parts)
    if rel_parts is not None:
        module_name = _module_name_from_relative_parts(
            rel_parts, fallback_parent=resolved.parent.name
        )
        if module_name is not None:
            return module_name

    best_rel_parts: tuple[str, ...] | None = None
    best_len = -1
    for root_resolved in resolved_roots:
        if cpython_test_root is not None and root_resolved == cpython_test_root:
            continue
        candidate_parts = _relative_parts_if_within(resolved_parts, root_resolved.parts)
        if candidate_parts is None:
            continue
        root_len = len(root_resolved.parts)
        if root_len > best_len:
            best_len = root_len
            best_rel_parts = candidate_parts
    if best_rel_parts is None:
        # Paths outside known module roots should still compile deterministically as
        # top-level modules instead of leaking absolute-path segments into module ids.
        if resolved.name == "__init__.py":
            return resolved.parent.name or "__init__"
        return resolved.stem
    module_name = _module_name_from_relative_parts(
        best_rel_parts, fallback_parent=resolved.parent.name
    )
    if module_name is not None:
        return module_name
    return resolved.parent.name or resolved.stem


@functools.lru_cache(maxsize=4096)
def _expand_module_chain_cached(name: str) -> tuple[str, ...]:
    name = name.strip()
    if not name:
        return ()
    parts = name.split(".")
    if any(not part or not part.isidentifier() for part in parts):
        return ()
    return tuple(".".join(parts[:idx]) for idx in range(1, len(parts) + 1))


def _expand_module_chain(name: str) -> list[str]:
    return list(_expand_module_chain_cached(name))


def _module_dependencies_from_imports(
    module_name: str,
    module_graph: Mapping[str, Path],
    imports: Iterable[str],
) -> set[str]:
    deps: set[str] = set()
    for name in imports:
        for candidate in _expand_module_chain_cached(name):
            if candidate == "molt" and module_name.startswith("molt."):
                continue
            if candidate in module_graph and candidate != module_name:
                deps.add(candidate)
            if candidate.startswith("molt.stdlib."):
                stdlib_candidate = candidate[len("molt.stdlib.") :]
                if stdlib_candidate in module_graph and stdlib_candidate != module_name:
                    deps.add(stdlib_candidate)
    return deps


def _module_dependencies(
    tree: ast.AST,
    module_name: str,
    module_graph: dict[str, Path],
    *,
    imports: list[str] | None = None,
) -> set[str]:
    path = module_graph.get(module_name)
    is_package = path is not None and path.name == "__init__.py"
    collected_imports = (
        imports
        if imports is not None
        else _collect_imports(tree, module_name, is_package)
    )
    return _module_dependencies_from_imports(
        module_name,
        module_graph,
        collected_imports,
    )


def _module_dependency_layers(
    module_order: list[str],
    module_deps: dict[str, set[str]],
) -> list[list[str]]:
    if not module_order:
        return []
    depth_by_module: dict[str, int] = {}
    for name in module_order:
        deps = [
            dep
            for dep in module_deps.get(name, set())
            if dep in depth_by_module and dep != name
        ]
        if not deps:
            depth_by_module[name] = 0
            continue
        depth_by_module[name] = max(depth_by_module[dep] for dep in deps) + 1
    grouped: dict[int, list[str]] = {}
    for name in module_order:
        grouped.setdefault(depth_by_module.get(name, 0), []).append(name)
    return [grouped[level] for level in sorted(grouped)]


def _module_order_has_back_edges(
    module_order: list[str],
    module_deps: dict[str, set[str]],
) -> bool:
    seen: set[str] = set()
    module_set = set(module_order)
    for name in module_order:
        deps = {dep for dep in module_deps.get(name, set()) if dep in module_set}
        if not deps.issubset(seen):
            return True
        seen.add(name)
    return False


def _topo_sort_modules(
    module_graph: dict[str, Path], module_deps: dict[str, set[str]]
) -> list[str]:
    in_degree = {name: 0 for name in module_graph}
    dependents = _reverse_module_dependencies(module_deps, module_graph)
    for name, deps in module_deps.items():
        for dep in deps:
            in_degree[name] += 1
    ready = deque(sorted(name for name, degree in in_degree.items() if degree == 0))
    order: list[str] = []
    while ready:
        name = ready.popleft()
        order.append(name)
        for child in sorted(dependents[name]):
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    if len(order) != len(module_graph):
        remaining = sorted(name for name in module_graph if name not in order)
        order.extend(remaining)
    return order


def _analyze_module_schedule(
    module_graph: Mapping[str, Path],
    module_deps: Mapping[str, set[str]],
) -> tuple[
    list[str],
    dict[str, set[str]],
    bool,
    list[list[str]],
    dict[str, frozenset[str]],
]:
    module_names = set(module_graph)
    in_degree = {name: 0 for name in module_names}
    reverse_module_deps = _reverse_module_dependencies(dict(module_deps), module_names)
    for name, deps in module_deps.items():
        for dep in deps:
            if dep in module_names and name in in_degree:
                in_degree[name] += 1
    ready = deque(sorted(name for name, degree in in_degree.items() if degree == 0))
    order: list[str] = []
    while ready:
        name = ready.popleft()
        order.append(name)
        for child in sorted(reverse_module_deps.get(name, ())):
            if child not in in_degree:
                continue
            in_degree[child] -= 1
            if in_degree[child] == 0:
                ready.append(child)
    has_back_edges = len(order) != len(module_names)
    if has_back_edges:
        remaining = sorted(name for name in module_names if name not in order)
        order.extend(remaining)
    layers = _module_dependency_layers(order, dict(module_deps))
    module_dep_closures = _module_dependency_closures(
        dict(module_deps),
        module_names,
        module_order=order,
        has_back_edges=has_back_edges,
    )
    return order, reverse_module_deps, has_back_edges, layers, module_dep_closures


def _reverse_module_dependencies(
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
) -> dict[str, set[str]]:
    dependents: dict[str, set[str]] = {name: set() for name in module_names}
    for name, deps in module_deps.items():
        if name not in dependents:
            dependents[name] = set()
        for dep in deps:
            dependents.setdefault(dep, set()).add(name)
    return dependents


def _dependent_module_closure(
    dirty_modules: Collection[str],
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    reverse_module_deps: Mapping[str, set[str]] | None = None,
) -> set[str]:
    dependents = (
        reverse_module_deps
        if reverse_module_deps is not None
        else _reverse_module_dependencies(module_deps, module_names)
    )
    closure: set[str] = {name for name in dirty_modules if name in dependents}
    queue = deque(sorted(closure))
    while queue:
        module_name = queue.popleft()
        for dependent in sorted(dependents.get(module_name, ())):
            if dependent not in closure:
                closure.add(dependent)
                queue.append(dependent)
    return closure


def _module_dependency_closure(
    module_name: str,
    module_deps: dict[str, set[str]],
) -> set[str]:
    closure: set[str] = {module_name}
    queue = deque([module_name])
    while queue:
        current = queue.popleft()
        for dep in sorted(module_deps.get(current, ())):
            if dep not in closure:
                closure.add(dep)
                queue.append(dep)
    return closure


_DEAD_MODULE_ELIMINATION_SAFELIST: frozenset[str] = frozenset(
    {
        "builtins",
        "sys",
        "os",
        "os.path",
        "_collections_abc",
        "abc",
        "io",
        "typing",
        "types",
        "functools",
        "collections",
        "collections.abc",
        "enum",
        "dataclasses",
        "warnings",
        "importlib",
        "importlib.util",
        "importlib.machinery",
        "importlib.abc",
        "_thread",
        "threading",
        "copyreg",
        "keyword",
        "operator",
        "reprlib",
        "itertools",
        IMPORTER_MODULE_NAME,
        "molt.stdlib",
    }
)


def _compute_reachable_modules(
    entry_module: str,
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    extra_roots: Collection[str] = (),
) -> set[str]:
    reachable: set[str] = set()
    queue: deque[str] = deque()
    module_name_set = set(module_names)

    def _seed(name: str) -> None:
        if name in reachable:
            return
        reachable.add(name)
        queue.append(name)

    _seed(entry_module)
    for safe in _DEAD_MODULE_ELIMINATION_SAFELIST:
        if safe in module_name_set:
            _seed(safe)
    for root in extra_roots:
        if root in module_name_set:
            _seed(root)
    while queue:
        current = queue.popleft()
        for dep in module_deps.get(current, ()):
            _seed(dep)
    parents_to_add: set[str] = set()
    for name in list(reachable):
        parts = name.split(".")
        for i in range(1, len(parts)):
            parent = ".".join(parts[:i])
            if parent in module_name_set:
                parents_to_add.add(parent)
    reachable.update(parents_to_add)
    return reachable


def _apply_dead_module_elimination(
    module_order: list[str],
    module_layers: list[list[str]],
    entry_module: str,
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    extra_roots: Collection[str] = (),
) -> tuple[list[str], list[list[str]], int]:
    reachable = _compute_reachable_modules(
        entry_module,
        module_deps,
        module_names,
        extra_roots=extra_roots,
    )
    filtered_order = [m for m in module_order if m in reachable]
    filtered_layers = [[m for m in layer if m in reachable] for layer in module_layers]
    filtered_layers = [layer for layer in filtered_layers if layer]
    eliminated_count = len(module_order) - len(filtered_order)
    return filtered_order, filtered_layers, eliminated_count


def _module_dependency_closures(
    module_deps: dict[str, set[str]],
    module_names: Collection[str],
    *,
    module_order: Sequence[str] | None = None,
    has_back_edges: bool = False,
) -> dict[str, frozenset[str]]:
    if module_order is not None and not has_back_edges:
        closures: dict[str, frozenset[str]] = {}
        for module_name in tuple(module_order):
            closure: set[str] = {module_name}
            for dep in module_deps.get(module_name, ()):
                closure.update(closures.get(dep, frozenset({dep})))
            closures[module_name] = frozenset(closure)
        for module_name in module_names:
            closures.setdefault(module_name, frozenset({module_name}))
        return closures
    closures: dict[str, frozenset[str]] = {}
    for module_name in sorted(module_names):
        closures[module_name] = frozenset(
            _module_dependency_closure(module_name, module_deps)
        )
    return closures


def _stdlib_root_path() -> Path:
    override = os.environ.get("MOLT_PROJECT_ROOT")
    if override:
        root = Path(override).expanduser()
        if not root.is_absolute():
            root = (Path.cwd() / root).absolute()
        candidate = root / "src/molt/stdlib"
        if candidate.exists():
            return candidate.resolve()
    candidate = Path(__file__).resolve().parent / "stdlib"
    if candidate.exists():
        return candidate.resolve()
    return Path("src/molt/stdlib").resolve()


def _resolve_module_path(module_name: str, roots: list[Path]) -> Path | None:
    return _resolve_module_path_parts(tuple(module_name.split(".")), roots)


@functools.lru_cache(maxsize=65536)
def _case_exact_dir_entries_cached(
    dir_text: str, mtime_ns: int, size: int
) -> frozenset[str]:
    del mtime_ns, size
    try:
        return frozenset(entry.name for entry in os.scandir(dir_text))
    except OSError:
        return frozenset()


def _case_exact_dir_entries(dir_text: str) -> frozenset[str]:
    try:
        stat = os.stat(dir_text)
    except OSError:
        return frozenset()
    return _case_exact_dir_entries_cached(dir_text, stat.st_mtime_ns, stat.st_size)


def _case_exact_file_under(root_text: str, rel_parts: tuple[str, ...]) -> bool:
    if not rel_parts:
        return False
    current = root_text
    for part in rel_parts:
        if part not in _case_exact_dir_entries(current):
            return False
        current = os.path.join(current, part)
    return os.path.isfile(current)


def _case_exact_file(path: Path) -> bool:
    if path.is_absolute():
        anchor = path.anchor
        rel_parts = tuple(part for part in path.parts[1:] if part)
        return _case_exact_file_under(anchor, rel_parts)
    return _case_exact_file_under(os.curdir, tuple(path.parts))


def _resolve_module_path_parts(
    parts: tuple[str, ...], roots: list[Path]
) -> Path | None:
    if not parts:
        return None
    module_filename = f"{parts[-1]}.py"
    for root in roots:
        root_text = os.fspath(root)
        pkg_text = os.path.join(root_text, *parts, "__init__.py")
        if _case_exact_file_under(root_text, (*parts, "__init__.py")):
            return Path(pkg_text)
        if len(parts) == 1:
            mod_text = os.path.join(root_text, module_filename)
            mod_parts = (module_filename,)
        else:
            mod_text = os.path.join(root_text, *parts[:-1], module_filename)
            mod_parts = (*parts[:-1], module_filename)
        if _case_exact_file_under(root_text, mod_parts):
            return Path(mod_text)
    return None


@dataclass
class _ModuleResolutionCache:
    roots_cache: dict[str, list[Path]] = field(default_factory=dict)
    resolve_cache: dict[str, Path | None] = field(default_factory=dict)
    namespace_dir_cache: dict[str, bool] = field(default_factory=dict)
    resolved_path_cache: dict[Path, Path] = field(default_factory=dict)
    resolved_roots_cache: dict[tuple[Path, ...], tuple[Path, ...]] = field(
        default_factory=dict
    )
    source_cache: dict[Path, str] = field(default_factory=dict)
    source_error_cache: dict[Path, Exception] = field(default_factory=dict)
    ast_cache: dict[tuple[Path, str, str], ast.AST] = field(default_factory=dict)
    ast_error_cache: dict[tuple[Path, str, str], SyntaxError] = field(
        default_factory=dict
    )
    runtime_import_protocol_cache: dict[
        tuple[Path, str | None, bool, ImportScanMode], bool
    ] = field(default_factory=dict)
    module_name_cache: dict[tuple[Path, tuple[Path, ...], Path, Path | None], str] = (
        field(default_factory=dict)
    )
    module_name_context_key: tuple[tuple[Path, ...], Path, Path | None] | None = None
    module_name_context_cache: dict[Path, str] = field(default_factory=dict)
    stdlib_path_cache: dict[tuple[Path, Path], bool] = field(default_factory=dict)
    import_scan_cache: dict[
        tuple[Path, str | None, bool, ImportScanMode], tuple[str, ...]
    ] = field(default_factory=dict)
    path_stat_cache: dict[Path, os.stat_result] = field(default_factory=dict)
    path_stat_error_cache: dict[Path, OSError] = field(default_factory=dict)
    module_parts_cache: dict[str, tuple[str, ...]] = field(default_factory=dict)
    cpython_test_root_cache: Path | None = None
    cpython_test_root_cache_populated: bool = False

    def roots_for_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> list[Path]:
        candidate_roots = self.roots_cache.get(module_name)
        if candidate_roots is None:
            candidate_roots = _roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            self.roots_cache[module_name] = candidate_roots
        return candidate_roots

    def module_parts(self, module_name: str) -> tuple[str, ...]:
        cached = self.module_parts_cache.get(module_name)
        if cached is None:
            cached = tuple(module_name.split("."))
            self.module_parts_cache[module_name] = cached
        return cached

    def resolve_module(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> Path | None:
        cache_key = module_name
        if module_name.startswith("molt.stdlib."):
            cache_key = f"stdlib:{module_name}"
        if cache_key not in self.resolve_cache:
            if cache_key.startswith("stdlib:"):
                stdlib_candidate = module_name[len("molt.stdlib.") :]
                self.resolve_cache[cache_key] = _resolve_module_path_parts(
                    self.module_parts(stdlib_candidate), [stdlib_root]
                )
            else:
                candidate_roots = self.roots_for_module(
                    module_name, roots, stdlib_root, stdlib_allowlist
                )
                self.resolve_cache[cache_key] = _resolve_module_path_parts(
                    self.module_parts(module_name), candidate_roots
                )
        return self.resolve_cache[cache_key]

    def has_namespace_dir(
        self,
        module_name: str,
        roots: list[Path],
        stdlib_root: Path,
        stdlib_allowlist: set[str],
    ) -> bool:
        has_namespace_dir = self.namespace_dir_cache.get(module_name)
        if has_namespace_dir is None:
            candidate_roots = self.roots_for_module(
                module_name, roots, stdlib_root, stdlib_allowlist
            )
            has_namespace_dir = _has_namespace_dir(module_name, candidate_roots)
            self.namespace_dir_cache[module_name] = has_namespace_dir
        return has_namespace_dir

    def resolved_path(self, path: Path) -> Path:
        resolved = self.resolved_path_cache.get(path)
        if resolved is None:
            if path.is_absolute() and "." not in path.parts and ".." not in path.parts:
                resolved = path
            else:
                resolved = path.resolve()
            self.resolved_path_cache[path] = resolved
        return resolved

    def resolved_roots(self, roots: list[Path]) -> tuple[Path, ...]:
        roots_key = tuple(roots)
        resolved = self.resolved_roots_cache.get(roots_key)
        if resolved is None:
            resolved = tuple(self.resolved_path(root) for root in roots)
            self.resolved_roots_cache[roots_key] = resolved
        return resolved

    def module_name_from_path(
        self, path: Path, roots: list[Path], stdlib_root: Path
    ) -> str:
        resolved = self.resolved_path(path)
        resolved_roots = self.resolved_roots(roots)
        resolved_stdlib_root = self.resolved_path(stdlib_root)
        cpython_test_root = self.cpython_test_root()
        context_key = (resolved_roots, resolved_stdlib_root, cpython_test_root)
        if self.module_name_context_key == context_key:
            cached = self.module_name_context_cache.get(resolved)
            if cached is not None:
                return cached
        else:
            self.module_name_context_key = context_key
            self.module_name_context_cache.clear()
        cache_key = (
            resolved,
            resolved_roots,
            resolved_stdlib_root,
            cpython_test_root,
        )
        cached = self.module_name_cache.get(cache_key)
        if cached is not None:
            self.module_name_context_cache[resolved] = cached
            return cached
        module_name = _module_name_from_resolved_path(
            resolved,
            resolved_roots=resolved_roots,
            resolved_stdlib_root=resolved_stdlib_root,
            cpython_test_root=cpython_test_root,
        )
        self.module_name_cache[cache_key] = module_name
        self.module_name_context_cache[resolved] = module_name
        return module_name

    def cpython_test_root(self) -> Path | None:
        if not self.cpython_test_root_cache_populated:
            cpython_dir = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
            if cpython_dir:
                self.cpython_test_root_cache = self.resolved_path(
                    Path(cpython_dir) / "Lib" / "test"
                )
            self.cpython_test_root_cache_populated = True
        return self.cpython_test_root_cache

    def is_stdlib_path(self, path: Path, stdlib_root: Path) -> bool:
        resolved_path = self.resolved_path(path)
        resolved_stdlib_root = self.resolved_path(stdlib_root)
        cache_key = (resolved_path, resolved_stdlib_root)
        cached = self.stdlib_path_cache.get(cache_key)
        if cached is None:
            cached = _is_stdlib_resolved_path(resolved_path, resolved_stdlib_root)
            self.stdlib_path_cache[cache_key] = cached
        return cached

    def read_module_source(self, path: Path, *, retain: bool = True) -> str:
        cache_key = self.resolved_path(path)
        if not retain:
            return _read_module_source(path)
        source = self.source_cache.get(cache_key)
        if source is not None:
            return source
        cached_error = self.source_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            source = _read_module_source(path)
        except (OSError, SyntaxError, UnicodeDecodeError) as exc:
            self.source_error_cache[cache_key] = exc
            raise
        self.source_cache[cache_key] = source
        return source

    def path_stat(self, path: Path) -> os.stat_result:
        cache_key = self.resolved_path(path)
        cached = self.path_stat_cache.get(cache_key)
        if cached is not None:
            return cached
        cached_error = self.path_stat_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            stat_result = path.stat()
        except OSError as exc:
            self.path_stat_error_cache[cache_key] = exc
            raise
        self.path_stat_cache[cache_key] = stat_result
        return stat_result

    def parse_module_ast(
        self,
        path: Path,
        source: str,
        *,
        filename: str,
        target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
        retain: bool = True,
    ) -> ast.AST:
        cache_key = (self.resolved_path(path), filename, target_python.tag)
        if not retain:
            return _parse_source_for_target(
                source,
                filename=filename,
                target_python=target_python,
            )
        tree = self.ast_cache.get(cache_key)
        if tree is not None:
            return tree
        cached_error = self.ast_error_cache.get(cache_key)
        if cached_error is not None:
            raise cached_error
        try:
            tree = _parse_source_for_target(
                source,
                filename=filename,
                target_python=target_python,
            )
        except SyntaxError as exc:
            self.ast_error_cache[cache_key] = exc
            raise
        self.ast_cache[cache_key] = tree
        return tree

    def collect_imports(
        self,
        path: Path,
        tree: ast.AST,
        *,
        module_name: str | None = None,
        is_package: bool = False,
        import_scan_mode: ImportScanMode = "full",
    ) -> tuple[str, ...]:
        cache_key = (
            self.resolved_path(path),
            module_name,
            is_package,
            import_scan_mode,
        )
        cached = self.import_scan_cache.get(cache_key)
        if cached is not None:
            return cached
        imports = _collect_imports(
            tree,
            module_name,
            is_package,
            import_scan_mode=import_scan_mode,
        )
        cached_imports = tuple(imports)
        self.import_scan_cache[cache_key] = cached_imports
        return cached_imports

    def uses_runtime_import_protocol(
        self,
        path: Path,
        tree: ast.AST,
        *,
        module_name: str | None = None,
        is_package: bool = False,
        import_scan_mode: ImportScanMode = "full",
    ) -> bool:
        cache_key = (
            self.resolved_path(path),
            module_name,
            is_package,
            import_scan_mode,
        )
        cached = self.runtime_import_protocol_cache.get(cache_key)
        if cached is not None:
            return cached
        cached = _tree_uses_runtime_import_protocol(
            tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
        )
        self.runtime_import_protocol_cache[cache_key] = cached
        return cached


def _has_namespace_dir(module_name: str, roots: list[Path]) -> bool:
    rel = Path(*module_name.split("."))
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            return True
    return False


def _spec_parent(spec_name: str, is_package: bool) -> str:
    if is_package:
        return spec_name
    return spec_name.rpartition(".")[0]


def _is_modulespec_ctor(node: ast.AST) -> bool:
    if isinstance(node, ast.Name):
        return node.id == "ModuleSpec"
    if isinstance(node, ast.Attribute):
        return node.attr == "ModuleSpec"
    return False


def _parse_modulespec_override(
    value: ast.AST,
) -> tuple[str, bool | None] | None:
    if not isinstance(value, ast.Call):
        return None
    if not _is_modulespec_ctor(value.func):
        return None
    spec_name = None
    if value.args:
        first = value.args[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            spec_name = first.value
    for kw in value.keywords:
        if (
            kw.arg == "name"
            and spec_name is None
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, str)
        ):
            spec_name = kw.value.value
    if spec_name is None:
        return None
    is_package = None
    if len(value.args) >= 4:
        arg = value.args[3]
        if isinstance(arg, ast.Constant) and isinstance(arg.value, bool):
            is_package = arg.value
    for kw in value.keywords:
        if (
            kw.arg == "is_package"
            and isinstance(kw.value, ast.Constant)
            and isinstance(kw.value.value, bool)
        ):
            is_package = kw.value.value
    return spec_name, is_package


def _infer_module_overrides(
    tree: ast.AST,
) -> tuple[bool, str | None, bool, str | None, bool | None]:
    package_override_set = False
    package_override: str | None = None
    spec_override_set = False
    spec_override: str | None = None
    spec_override_is_package: bool | None = None
    for stmt in getattr(tree, "body", []):
        if isinstance(stmt, ast.Assign):
            targets = stmt.targets
            value = stmt.value
        elif isinstance(stmt, ast.AnnAssign) and stmt.value is not None:
            targets = [stmt.target]
            value = stmt.value
        else:
            continue
        for target in targets:
            if not isinstance(target, ast.Name):
                continue
            if target.id == "__package__":
                package_override_set = True
                if isinstance(value, ast.Constant) and isinstance(value.value, str):
                    package_override = value.value
                elif isinstance(value, ast.Constant) and value.value is None:
                    package_override = None
                else:
                    package_override = None
            elif target.id == "__spec__":
                if isinstance(value, ast.Constant) and value.value is None:
                    spec_override_set = False
                    spec_override = None
                    spec_override_is_package = None
                else:
                    parsed = _parse_modulespec_override(value)
                    if parsed is None:
                        continue
                    spec_override_set = True
                    spec_override, spec_override_is_package = parsed
    return (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    )


def _resolve_relative_import(
    module_name: str,
    *,
    is_package: bool,
    level: int,
    module: str | None,
    package_override: str | None = None,
    package_override_set: bool = False,
    spec_override: str | None = None,
    spec_override_set: bool = False,
    spec_override_is_package: bool | None = None,
) -> str | None:
    if level <= 0:
        return module
    package = ""
    if package_override_set:
        package = package_override or ""
    else:
        if spec_override_set and spec_override:
            override_is_package = (
                spec_override_is_package
                if spec_override_is_package is not None
                else is_package
            )
            package = _spec_parent(spec_override, override_is_package)
        else:
            if is_package:
                package = module_name
            elif "." in module_name:
                package = module_name.rsplit(".", 1)[0]
            else:
                package = ""
    if not package:
        return None
    parts = package.split(".")
    if level > len(parts):
        return None
    base_parts = parts[: len(parts) - (level - 1)]
    base_name = ".".join(base_parts)
    if module:
        if base_name:
            return f"{base_name}.{module}"
        return module
    return base_name or None


def _module_init_scan_nodes(tree: ast.AST) -> tuple[ast.AST, ...]:
    if not isinstance(tree, ast.Module):
        return tuple(ast.walk(tree))
    nodes: list[ast.AST] = []

    def visit(node: ast.AST) -> None:
        nodes.append(node)
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for decorator in node.decorator_list:
                visit(decorator)
            for default in list(node.args.defaults) + [
                default for default in node.args.kw_defaults if default is not None
            ]:
                visit(default)
            for arg in (
                list(node.args.posonlyargs)
                + list(node.args.args)
                + list(node.args.kwonlyargs)
            ):
                if arg.annotation is not None:
                    visit(arg.annotation)
            if node.args.vararg is not None and node.args.vararg.annotation is not None:
                visit(node.args.vararg.annotation)
            if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
                visit(node.args.kwarg.annotation)
            if node.returns is not None:
                visit(node.returns)
            for type_param in getattr(node, "type_params", ()):
                visit(type_param)
            return
        if isinstance(node, ast.Lambda):
            for default in list(node.args.defaults) + [
                default for default in node.args.kw_defaults if default is not None
            ]:
                visit(default)
            return
        for child in ast.iter_child_nodes(node):
            visit(child)

    for stmt in tree.body:
        visit(stmt)
    return tuple(nodes)


def _collect_imports(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
) -> list[str]:
    if import_scan_mode not in {"full", "module_init"}:
        raise ValueError(f"unknown import scan mode: {import_scan_mode}")
    imports: list[str] = []
    needs_typing = False
    needs_string_templatelib = False
    type_alias_cls = getattr(ast, "TypeAlias", None)
    template_str_cls = getattr(ast, "TemplateStr", None)
    module_string_constants: dict[str, str] = {}
    helper_string_functions: dict[str, tuple[list[str], ast.expr]] = {}
    helper_param_import_positions: dict[str, set[int]] = {}
    helper_import_arg_exprs: dict[str, tuple[list[str], set[str], list[ast.expr]]] = {}
    (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    ) = _infer_module_overrides(tree)
    module_body = list(getattr(tree, "body", []))
    function_walks: list[
        tuple[
            ast.FunctionDef | ast.AsyncFunctionDef,
            tuple[ast.AST, ...],
            "_ImportlibStaticBindings",
        ]
    ] = []

    class _ImportlibStaticBindings:
        def __init__(self) -> None:
            self.module_aliases: set[str] = {"importlib"}
            self.util_aliases: set[str] = set()
            self.import_module_aliases: set[str] = set()
            self.module_import_module_mutated = False
            self.module_util_mutated = False

        def fork(self) -> "_ImportlibStaticBindings":
            forked = _ImportlibStaticBindings()
            forked.module_aliases = set(self.module_aliases)
            forked.util_aliases = set(self.util_aliases)
            forked.import_module_aliases = set(self.import_module_aliases)
            forked.module_import_module_mutated = self.module_import_module_mutated
            forked.module_util_mutated = self.module_util_mutated
            return forked

        def record_aliases(self, node: ast.Import | ast.ImportFrom) -> None:
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if alias.name == "importlib":
                        self.module_aliases.add(alias.asname or "importlib")
                    elif alias.name == "importlib.util":
                        if alias.asname:
                            if not self.module_util_mutated:
                                self.util_aliases.add(alias.asname)
                        else:
                            self.module_aliases.add("importlib")
                    elif alias.name.startswith("importlib.") and not alias.asname:
                        self.module_aliases.add("importlib")
                return
            if node.level or node.module != "importlib":
                return
            for alias in node.names:
                bind_name = alias.asname or alias.name
                if alias.name == "import_module":
                    if not self.module_import_module_mutated:
                        self.import_module_aliases.add(bind_name)
                elif alias.name == "util":
                    if not self.module_util_mutated:
                        self.util_aliases.add(bind_name)

        def invalidate_name(self, name: str) -> None:
            self.module_aliases.discard(name)
            self.util_aliases.discard(name)
            self.import_module_aliases.discard(name)

        def record_rebinding_target(self, target: ast.expr) -> None:
            if isinstance(target, ast.Name):
                self.invalidate_name(target.id)
                return
            if (
                isinstance(target, ast.Attribute)
                and isinstance(target.value, ast.Name)
                and target.value.id in self.module_aliases
            ):
                if target.attr == "import_module":
                    self.module_import_module_mutated = True
                elif target.attr == "util":
                    self.module_util_mutated = True

        def target(self, func: ast.expr) -> str | None:
            if isinstance(func, ast.Name):
                if func.id in self.import_module_aliases:
                    return "importlib.import_module"
                return func.id
            if (
                isinstance(func, ast.Attribute)
                and func.attr == "import_module"
                and isinstance(func.value, ast.Name)
                and func.value.id in self.module_aliases
            ):
                if self.module_import_module_mutated:
                    return None
                return "importlib.import_module"
            if isinstance(func, ast.Attribute) and func.attr == "find_spec":
                if (
                    isinstance(func.value, ast.Name)
                    and func.value.id in self.util_aliases
                ):
                    return "importlib.util.find_spec"
                if (
                    isinstance(func.value, ast.Attribute)
                    and func.value.attr == "util"
                    and isinstance(func.value.value, ast.Name)
                    and func.value.value.id in self.module_aliases
                ):
                    if self.module_util_mutated:
                        return None
                    return "importlib.util.find_spec"
            if isinstance(func, ast.Attribute):
                parts: list[str] = []
                current: ast.expr | None = func
                while isinstance(current, ast.Attribute):
                    parts.append(current.attr)
                    current = current.value
                if isinstance(current, ast.Name):
                    parts.append(current.id)
                    return ".".join(reversed(parts))
            return None

    helper_importlib_bindings = _ImportlibStaticBindings()

    def _is_static_import_target(target: str | None) -> bool:
        return target in {
            "__import__",
            "builtins.__import__",
            "importlib.import_module",
            "importlib.util.find_spec",
            "_MOLT_IMPORTLIB_IMPORT_TRANSACTION",
            "molt_importlib_import_transaction",
        }

    def _resolve_string_sequence(
        node: ast.expr, bindings: dict[str, object], seen: set[str]
    ) -> list[str] | None:
        if isinstance(node, (ast.Tuple, ast.List)):
            out: list[str] = []
            for element in node.elts:
                value = _resolve_string_constant(element, bindings, seen)
                if value is None:
                    return None
                out.append(value)
            return out
        if isinstance(node, ast.Name):
            bound = bindings.get(node.id)
            if isinstance(bound, list) and all(isinstance(item, str) for item in bound):
                return list(cast(list[str], bound))
        return None

    def _resolve_string_constant(
        node: ast.expr,
        bindings: dict[str, object] | None = None,
        seen: set[str] | None = None,
    ) -> str | None:
        bindings = bindings or {}
        seen = seen or set()
        if isinstance(node, ast.Constant) and isinstance(node.value, str):
            return node.value
        if isinstance(node, ast.Name):
            bound = bindings.get(node.id)
            if isinstance(bound, str):
                return bound
            return module_string_constants.get(node.id)
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
            left = _resolve_string_constant(node.left, bindings, seen)
            right = _resolve_string_constant(node.right, bindings, seen)
            if left is not None and right is not None:
                return left + right
            return None
        if isinstance(node, ast.Call):
            target = helper_importlib_bindings.target(node.func)
            if (
                target
                in {
                    "_MOLT_IMPORTLIB_RESOLVE_NAME",
                    "molt_importlib_resolve_name",
                }
                and node.args
            ):
                resolved = _resolve_string_constant(node.args[0], bindings, seen)
                if resolved is None:
                    return None
                if not resolved.startswith("."):
                    return resolved
                if len(node.args) < 2:
                    return None
                package = _resolve_string_constant(node.args[1], bindings, seen)
                if package is None:
                    return None
                level = len(resolved) - len(resolved.lstrip("."))
                module = resolved[level:] or None
                return _resolve_relative_import(
                    package,
                    is_package=True,
                    level=level,
                    module=module,
                    package_override=package_override,
                    package_override_set=package_override_set,
                    spec_override=spec_override,
                    spec_override_set=spec_override_set,
                    spec_override_is_package=spec_override_is_package,
                )
            if (
                isinstance(node.func, ast.Attribute)
                and node.func.attr == "join"
                and len(node.args) == 1
            ):
                sep = _resolve_string_constant(node.func.value, bindings, seen)
                if sep is None:
                    return None
                items = _resolve_string_sequence(node.args[0], bindings, seen)
                if items is None:
                    return None
                return sep.join(items)
            if isinstance(node.func, ast.Name):
                func_name = node.func.id
                if func_name in seen:
                    return None
                helper = helper_string_functions.get(func_name)
                if helper is None:
                    return None
                params, expr = helper
                if len(node.args) != len(params) or node.keywords:
                    return None
                child_bindings: dict[str, object] = dict(bindings)
                for param, arg in zip(params, node.args):
                    scalar = _resolve_string_constant(arg, bindings, seen)
                    if scalar is not None:
                        child_bindings[param] = scalar
                        continue
                    seq = _resolve_string_sequence(arg, bindings, seen)
                    if seq is not None:
                        child_bindings[param] = seq
                        continue
                    return None
                return _resolve_string_constant(
                    expr, child_bindings, seen | {func_name}
                )
        return None

    def _function_required_param_names(
        stmt: ast.FunctionDef | ast.AsyncFunctionDef, params: list[str]
    ) -> set[str]:
        positional = list(stmt.args.posonlyargs) + list(stmt.args.args)
        required_positional_count = max(0, len(positional) - len(stmt.args.defaults))
        required = {arg.arg for arg in positional[:required_positional_count]}
        for arg, default in zip(stmt.args.kwonlyargs, stmt.args.kw_defaults):
            if default is None:
                required.add(arg.arg)
        return required.intersection(params)

    def _simple_function_local_expr_bindings(
        stmt: ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> dict[str, ast.expr]:
        values: dict[str, ast.expr] = {}
        repeated: set[str] = set()
        for node in ast.walk(stmt):
            assignment: tuple[ast.expr, ast.expr] | None = None
            if isinstance(node, ast.Assign) and len(node.targets) == 1:
                assignment = (node.targets[0], node.value)
            elif isinstance(node, ast.AnnAssign):
                if node.value is not None:
                    assignment = (node.target, node.value)
            if assignment is None:
                continue
            target, value = assignment
            if not isinstance(target, ast.Name):
                continue
            if target.id in values:
                repeated.add(target.id)
                continue
            values[target.id] = value
        for name in repeated:
            values.pop(name, None)
        return values

    def _resolve_local_expr_binding(
        expr: ast.expr, local_expr_bindings: dict[str, ast.expr]
    ) -> ast.expr:
        seen: set[str] = set()
        current = expr
        while isinstance(current, ast.Name) and current.id in local_expr_bindings:
            if current.id in seen:
                return expr
            seen.add(current.id)
            current = local_expr_bindings[current.id]
        return current

    def _bind_helper_call_arguments(
        call: ast.Call, params: list[str], required_params: set[str]
    ) -> dict[str, object] | None:
        if len(call.args) > len(params):
            return None
        bindings: dict[str, object] = {}
        for idx, arg in enumerate(call.args):
            param = params[idx]
            scalar = _resolve_string_constant(arg)
            if scalar is not None:
                bindings[param] = scalar
                continue
            seq = _resolve_string_sequence(arg, {}, set())
            if seq is not None:
                bindings[param] = seq
        for keyword in call.keywords:
            if keyword.arg is None or keyword.arg not in params:
                return None
            if keyword.arg in bindings:
                return None
            scalar = _resolve_string_constant(keyword.value)
            if scalar is not None:
                bindings[keyword.arg] = scalar
                continue
            seq = _resolve_string_sequence(keyword.value, {}, set())
            if seq is not None:
                bindings[keyword.arg] = seq
        if not required_params.issubset(bindings):
            return None
        return bindings

    module_import_helper_scan = isinstance(tree, ast.Module)

    if module_import_helper_scan:
        for stmt in module_body:
            if isinstance(stmt, (ast.Import, ast.ImportFrom)):
                helper_importlib_bindings.record_aliases(stmt)
            if isinstance(stmt, ast.Assign) and len(stmt.targets) == 1:
                target = stmt.targets[0]
                for rebind_target in stmt.targets:
                    helper_importlib_bindings.record_rebinding_target(rebind_target)
                if isinstance(target, ast.Name):
                    value = _resolve_string_constant(stmt.value)
                    if value is not None:
                        module_string_constants[target.id] = value
            elif isinstance(stmt, ast.AnnAssign) and isinstance(stmt.target, ast.Name):
                helper_importlib_bindings.record_rebinding_target(stmt.target)
                value = _resolve_string_constant(stmt.value) if stmt.value else None
                if value is not None:
                    module_string_constants[stmt.target.id] = value
            elif isinstance(stmt, ast.AugAssign):
                helper_importlib_bindings.record_rebinding_target(stmt.target)
            elif isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                stmt_nodes = tuple(ast.walk(stmt))
                function_walks.append(
                    (stmt, stmt_nodes, helper_importlib_bindings.fork())
                )
                if len(stmt.body) != 1 or not isinstance(stmt.body[0], ast.Return):
                    continue
                ret_expr = stmt.body[0].value
                if ret_expr is None:
                    continue
                params = [
                    arg.arg
                    for arg in (
                        list(stmt.args.posonlyargs)
                        + list(stmt.args.args)
                        + list(stmt.args.kwonlyargs)
                    )
                ]
                if stmt.args.vararg is not None or stmt.args.kwarg is not None:
                    continue
                helper_string_functions[stmt.name] = (params, ret_expr)

        for stmt, stmt_nodes, stmt_importlib_bindings in function_walks:
            params = [
                arg.arg
                for arg in (
                    list(stmt.args.posonlyargs)
                    + list(stmt.args.args)
                    + list(stmt.args.kwonlyargs)
                )
            ]
            if stmt.args.vararg is not None:
                params.append(stmt.args.vararg.arg)
            if stmt.args.kwarg is not None:
                params.append(stmt.args.kwarg.arg)
            if not params:
                continue
            param_set = set(params)
            param_positions = {name: idx for idx, name in enumerate(params)}
            required_params = _function_required_param_names(stmt, params)
            local_expr_bindings = _simple_function_local_expr_bindings(stmt)
            for node in stmt_nodes:
                if not isinstance(node, ast.Call) or not node.args:
                    continue
                target = stmt_importlib_bindings.target(node.func)
                if not _is_static_import_target(target):
                    continue
                first = _resolve_local_expr_binding(node.args[0], local_expr_bindings)
                helper_entry = helper_import_arg_exprs.get(stmt.name)
                if helper_entry is None:
                    helper_import_arg_exprs[stmt.name] = (
                        params,
                        required_params,
                        [first],
                    )
                else:
                    helper_entry[2].append(first)
                if isinstance(first, ast.Name) and first.id in param_set:
                    pos = param_positions[first.id]
                    helper_param_import_positions.setdefault(stmt.name, set()).add(pos)

    def _record_helper_call_imports(node: ast.Call) -> None:
        if module_import_helper_scan:
            if not isinstance(node.func, ast.Name):
                return
            positions = helper_param_import_positions.get(node.func.id)
            if positions:
                for pos in positions:
                    if pos < len(node.args):
                        resolved = _resolve_string_constant(node.args[pos])
                        if resolved is not None:
                            imports.append(resolved)
            helper_expr_entry = helper_import_arg_exprs.get(node.func.id)
            if helper_expr_entry is not None:
                params, required_params, exprs = helper_expr_entry
                call_bindings = _bind_helper_call_arguments(
                    node, params, required_params
                )
                if call_bindings is not None:
                    for expr in exprs:
                        resolved = _resolve_string_constant(expr, call_bindings, set())
                        if resolved is not None:
                            imports.append(resolved)

    def _record_import_statement(
        node: ast.Import | ast.ImportFrom, bindings: _ImportlibStaticBindings
    ) -> None:
        bindings.record_aliases(node)
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports.append(alias.name)
            return
        if node.level:
            if module_name:
                resolved = _resolve_relative_import(
                    module_name,
                    is_package=is_package,
                    level=node.level,
                    module=node.module,
                    package_override=package_override,
                    package_override_set=package_override_set,
                    spec_override=spec_override,
                    spec_override_set=spec_override_set,
                    spec_override_is_package=spec_override_is_package,
                )
                if resolved:
                    imports.append(resolved)
                    for alias in node.names:
                        if alias.name != "*":
                            imports.append(f"{resolved}.{alias.name}")
            return
        if node.module:
            imports.append(node.module)
            for alias in node.names:
                if alias.name != "*":
                    imports.append(f"{node.module}.{alias.name}")

    def _collect_import_call(
        node: ast.Call, bindings: _ImportlibStaticBindings
    ) -> None:
        _record_helper_call_imports(node)
        if _is_static_import_target(bindings.target(node.func)):
            resolved = _resolve_string_constant(node.args[0])
            if resolved is not None:
                imports.append(resolved)

    def _function_parameter_names(
        node: ast.Lambda | ast.FunctionDef | ast.AsyncFunctionDef,
    ) -> list[str]:
        args = node.args
        names = [arg.arg for arg in args.posonlyargs]
        names.extend(arg.arg for arg in args.args)
        names.extend(arg.arg for arg in args.kwonlyargs)
        if args.vararg is not None:
            names.append(args.vararg.arg)
        if args.kwarg is not None:
            names.append(args.kwarg.arg)
        return names

    def _visit_many(
        nodes: Iterable[ast.AST], bindings: _ImportlibStaticBindings
    ) -> None:
        for child in nodes:
            _visit(child, bindings)

    def _visit(node: ast.AST, bindings: _ImportlibStaticBindings) -> None:
        nonlocal needs_string_templatelib, needs_typing
        if isinstance(node, ast.Module):
            _visit_many(node.body, bindings)
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            _record_import_statement(node, bindings)
            return
        if isinstance(node, ast.Assign):
            _visit(node.value, bindings)
            _visit_many(node.targets, bindings)
            for target in node.targets:
                bindings.record_rebinding_target(target)
            return
        if isinstance(node, ast.AnnAssign):
            _visit(node.annotation, bindings)
            if node.value is not None:
                _visit(node.value, bindings)
            _visit(node.target, bindings)
            bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.AugAssign):
            _visit(node.target, bindings)
            _visit(node.value, bindings)
            bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, ast.Delete):
            _visit_many(node.targets, bindings)
            for target in node.targets:
                bindings.record_rebinding_target(target)
            return
        if isinstance(node, ast.NamedExpr):
            _visit(node.value, bindings)
            _visit(node.target, bindings)
            bindings.record_rebinding_target(node.target)
            return
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            if getattr(node, "type_params", None):
                needs_typing = True
            if isinstance(node, ast.ClassDef):
                _visit_many(node.decorator_list, bindings)
                _visit_many(node.bases, bindings)
                _visit_many(
                    [keyword.value for keyword in node.keywords if keyword.value],
                    bindings,
                )
                _visit_many(getattr(node, "type_params", ()), bindings)
                class_bindings = bindings.fork()
                _visit_many(node.body, class_bindings)
                bindings.module_import_module_mutated |= (
                    class_bindings.module_import_module_mutated
                )
                bindings.module_util_mutated |= class_bindings.module_util_mutated
                return
            _visit_many(node.decorator_list, bindings)
            _visit_many(list(node.args.defaults), bindings)
            _visit_many(
                [default for default in node.args.kw_defaults if default is not None],
                bindings,
            )
            for arg in (
                list(node.args.posonlyargs)
                + list(node.args.args)
                + list(node.args.kwonlyargs)
            ):
                if arg.annotation is not None:
                    _visit(arg.annotation, bindings)
            if node.args.vararg is not None and node.args.vararg.annotation is not None:
                _visit(node.args.vararg.annotation, bindings)
            if node.args.kwarg is not None and node.args.kwarg.annotation is not None:
                _visit(node.args.kwarg.annotation, bindings)
            if node.returns is not None:
                _visit(node.returns, bindings)
            _visit_many(getattr(node, "type_params", ()), bindings)
            if import_scan_mode == "full":
                function_bindings = bindings.fork()
                for name in _function_parameter_names(node):
                    function_bindings.invalidate_name(name)
                _visit_many(node.body, function_bindings)
            return
        if isinstance(node, ast.Lambda):
            _visit_many(list(node.args.defaults), bindings)
            _visit_many(
                [default for default in node.args.kw_defaults if default is not None],
                bindings,
            )
            if import_scan_mode == "full":
                lambda_bindings = bindings.fork()
                for name in _function_parameter_names(node):
                    lambda_bindings.invalidate_name(name)
                _visit(node.body, lambda_bindings)
            return
        if type_alias_cls is not None and isinstance(node, type_alias_cls):
            needs_typing = True
            return
        if template_str_cls is not None and isinstance(node, template_str_cls):
            # PEP 750 t-strings desugar to string.templatelib.{Template,Interpolation}
            # at the molt frontend layer, so the import must be reflected in the
            # module graph closure even though no `import` statement appears.
            needs_string_templatelib = True
            return
        if isinstance(node, ast.Call) and node.args:
            _collect_import_call(node, bindings)
        for child in ast.iter_child_nodes(node):
            _visit(child, bindings)

    _visit(tree, _ImportlibStaticBindings())
    if needs_typing:
        imports.append("typing")
    if needs_string_templatelib:
        imports.append("string.templatelib")
    return imports


def _source_may_use_runtime_import_protocol(source: str) -> bool:
    return any(marker in source for marker in _RUNTIME_IMPORT_PROTOCOL_MARKERS)


def _resolve_runtime_import_expr_name(
    expr: ast.expr,
    alias_bindings: Mapping[str, str],
) -> str | None:
    if isinstance(expr, ast.Name):
        return alias_bindings.get(expr.id, expr.id)
    if (
        isinstance(expr, ast.Call)
        and isinstance(expr.func, ast.Name)
        and expr.func.id == "getattr"
        and len(expr.args) >= 2
        and not expr.keywords
    ):
        base = _resolve_runtime_import_expr_name(expr.args[0], alias_bindings)
        attr_node = expr.args[1]
        if (
            base is not None
            and isinstance(attr_node, ast.Constant)
            and isinstance(attr_node.value, str)
        ):
            return f"{base}.{attr_node.value}"
        return None
    if isinstance(expr, ast.Attribute):
        base = _resolve_runtime_import_expr_name(expr.value, alias_bindings)
        if base is None:
            return None
        return f"{base}.{expr.attr}"
    return None


def _runtime_import_alias_bindings(
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    import_scan_mode: ImportScanMode = "full",
) -> dict[str, str]:
    bindings: dict[str, str] = {}
    scan_nodes = (
        tuple(ast.walk(tree))
        if import_scan_mode == "full"
        else _module_init_scan_nodes(tree)
    )

    def _register_binding(local_name: str, qualified_name: str) -> None:
        if local_name and qualified_name:
            bindings[local_name] = qualified_name

    for node in scan_nodes:
        if isinstance(node, ast.Import):
            for alias in node.names:
                local_name = alias.asname or alias.name.split(".", 1)[0]
                qualified_name = alias.name if alias.asname else local_name
                _register_binding(local_name, qualified_name)
            continue
        if not isinstance(node, ast.ImportFrom):
            continue
        if node.level:
            if module_name is None:
                continue
            resolved_module = _resolve_relative_import(
                module_name,
                is_package=is_package,
                level=node.level,
                module=node.module,
            )
        else:
            resolved_module = node.module
        if not resolved_module:
            continue
        for alias in node.names:
            if alias.name == "*":
                continue
            local_name = alias.asname or alias.name
            _register_binding(local_name, f"{resolved_module}.{alias.name}")

    for node in scan_nodes:
        value: ast.expr | None = None
        target_names: list[str] = []
        if isinstance(node, ast.Assign):
            value = node.value
            target_names = [
                target.id for target in node.targets if isinstance(target, ast.Name)
            ]
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            value = node.value
            target_names = [node.target.id]
        if value is None or not target_names:
            continue
        resolved_value = _resolve_runtime_import_expr_name(value, bindings)
        if resolved_value not in _RUNTIME_IMPORT_PROTOCOL_TARGETS:
            continue
        for target_name in target_names:
            _register_binding(target_name, resolved_value)
    return bindings


def _tree_uses_runtime_import_protocol(
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    import_scan_mode: ImportScanMode = "full",
) -> bool:
    alias_bindings = _runtime_import_alias_bindings(
        tree,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )
    scan_nodes = (
        tuple(ast.walk(tree))
        if import_scan_mode == "full"
        else _module_init_scan_nodes(tree)
    )
    for node in scan_nodes:
        if not isinstance(node, ast.Call):
            continue
        target = _resolve_runtime_import_expr_name(node.func, alias_bindings)
        if target in _RUNTIME_IMPORT_PROTOCOL_TARGETS:
            return True
    return False


def _is_stdlib_resolved_path(resolved: Path, resolved_stdlib_root: Path) -> bool:
    return (
        _relative_parts_if_within(resolved.parts, resolved_stdlib_root.parts)
        is not None
    )


def _relative_parts_if_within(
    candidate_parts: tuple[str, ...], root_parts: tuple[str, ...]
) -> tuple[str, ...] | None:
    if len(candidate_parts) < len(root_parts):
        return None
    if candidate_parts[: len(root_parts)] != root_parts:
        return None
    return candidate_parts[len(root_parts) :]


def _module_name_from_relative_parts(
    rel_parts: tuple[str, ...], *, fallback_parent: str
) -> str | None:
    if not rel_parts:
        return None
    if rel_parts[-1] == "__init__.py":
        package_parts = rel_parts[:-1]
        if package_parts:
            return ".".join(package_parts)
        return fallback_parent or None
    last = rel_parts[-1]
    if last.endswith(".py"):
        rel_parts = (*rel_parts[:-1], last[:-3])
    filtered = tuple(part for part in rel_parts if part)
    if not filtered:
        return fallback_parent or None
    return ".".join(filtered)


@dataclass(frozen=True)
class ModuleSyntaxErrorInfo:
    message: str
    filename: str
    lineno: int | None
    offset: int | None
    text: str | None


def _read_module_source(path: Path) -> str:
    def normalize_newlines(source: str) -> str:
        return source.replace("\r\n", "\n").replace("\r", "\n")

    with path.open("rb") as handle:
        first_line = handle.readline()
        second_line = handle.readline()
        has_utf8_bom = first_line.startswith(codecs.BOM_UTF8)
        _cookie_re = tokenize.cookie_re
        if isinstance(_cookie_re.pattern, bytes):
            cookie_re = cast(re.Pattern[bytes], _cookie_re)
            has_encoding_cookie = any(
                cookie_re.match(line) for line in (first_line, second_line) if line
            )
        else:
            has_encoding_cookie = any(
                _cookie_re.match(line.decode("latin-1", errors="ignore"))
                for line in (first_line, second_line)
                if line
            )
        if not has_utf8_bom and not has_encoding_cookie:
            return normalize_newlines(
                (first_line + second_line + handle.read()).decode("utf-8")
            )
    with tokenize.open(path) as handle:
        return normalize_newlines(handle.read())


def _is_stdlib_module(name: str, stdlib_allowlist: set[str]) -> bool:
    if name.startswith("molt."):
        return False
    if name in stdlib_allowlist:
        return True
    top = name.split(".", 1)[0]
    return top in stdlib_allowlist


def _roots_for_module(
    module_name: str,
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
) -> list[Path]:
    if _is_stdlib_module(module_name, stdlib_allowlist):
        if module_name == "test.tokenizedata" or module_name.startswith(
            "test.tokenizedata."
        ):
            return [stdlib_root] + [root for root in roots if root != stdlib_root]
        if module_name == "test" or module_name.startswith("test."):
            if os.environ.get("MOLT_REGRTEST_CPYTHON_DIR"):
                return roots
        return [stdlib_root]
    return roots


def _write_importer_module(output_dir: Path) -> Path:
    lines = [
        '"""Auto-generated import dispatcher for Molt-compiled modules."""',
        "",
        "from __future__ import annotations",
        "",
    ]
    lines.extend(
        [
            "from _intrinsics import require_intrinsic as _require_intrinsic",
            "",
            "_IMPORT_TRANSACTION = _require_intrinsic(",
            "    'molt_importlib_import_transaction', globals()",
            ")",
            "",
            "def _molt_import(name, globals=None, locals=None, fromlist=(), level=0):",
            "    return _IMPORT_TRANSACTION(name, globals, locals, fromlist, level)",
        ]
    )
    path = output_dir / f"{IMPORTER_MODULE_NAME}.py"
    _write_text_if_changed(path, "\n".join(lines) + "\n")
    return path


def _parse_static_import_modules(raw: str) -> tuple[frozenset[str], str | None]:
    modules: set[str] = set()
    for part in re.split(r"[\s,]+", raw.strip()):
        if not part:
            continue
        if not re.fullmatch(
            r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*",
            part,
        ):
            return frozenset(), (
                f"{STATIC_IMPORT_MODULES_ENV} must contain comma/space-separated "
                f"Python module names; invalid entry: {part!r}"
            )
        modules.add(part)
    return frozenset(modules), None


def _collect_namespace_parents(
    module_graph: Mapping[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: Collection[str] | None = None,
    *,
    resolver_cache: _ModuleResolutionCache | None = None,
) -> set[str]:
    namespace_parents: set[str] = set()
    resolution_cache = resolver_cache or _ModuleResolutionCache()

    def maybe_add(name: str) -> None:
        if name in module_graph:
            return
        if (
            resolution_cache.resolve_module(name, roots, stdlib_root, stdlib_allowlist)
            is not None
        ):
            return
        if resolution_cache.has_namespace_dir(
            name, roots, stdlib_root, stdlib_allowlist
        ):
            namespace_parents.add(name)

    for module_name in module_graph:
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            maybe_add(".".join(parts[:idx]))

    if explicit_imports:
        for name in explicit_imports:
            for candidate in _expand_module_chain_cached(name):
                maybe_add(candidate)
    return namespace_parents


def _namespace_paths(name: str, roots: list[Path]) -> list[str]:
    rel = Path(*name.split("."))
    paths: list[str] = []
    for root in roots:
        candidate = root / rel
        if candidate.exists() and candidate.is_dir():
            paths.append(str(candidate))
    return list(dict.fromkeys(paths))


def _write_namespace_module(name: str, paths: list[str], output_dir: Path) -> Path:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", name.replace(".", "_")).strip("_")
    if not safe:
        safe = "root"
    stub_path = output_dir / f"namespace_{safe}.py"
    lines = [
        '"""Auto-generated namespace package stub for Molt."""',
        "",
        f"__package__ = {name!r}",
        f"__path__ = {paths!r}",
        "try:",
        "    spec = __spec__",
        "except NameError:",
        "    spec = None",
        "if spec is not None:",
        "    try:",
        "        spec.submodule_search_locations = list(__path__)",
        "    except Exception:",
        "        pass",
        "",
    ]
    stub_path.parent.mkdir(parents=True, exist_ok=True)
    _write_text_if_changed(stub_path, "\n".join(lines))
    return stub_path


def _logical_generated_module_path(module_name: str) -> str:
    safe = re.sub(r"[^0-9A-Za-z_]+", "_", module_name).strip("_")
    if not safe:
        safe = "module"
    return f"/__molt_generated__/{safe}.py"


def _collect_package_parents(
    module_graph: dict[str, Path],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    *,
    resolver_cache: _ModuleResolutionCache | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
) -> set[str]:
    resolution_cache = resolver_cache or _ModuleResolutionCache()
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    pending: set[str] = set()
    added: set[str] = set()
    for module_name in list(module_graph):
        parts = module_name.split(".")
        for idx in range(1, len(parts)):
            pending.add(".".join(parts[:idx]))

    while pending:
        parent = pending.pop()
        if parent in module_graph:
            continue
        resolved = resolution_cache.resolve_module(
            parent, roots, stdlib_root, stdlib_allowlist
        )
        if resolved is None or resolved.name != "__init__.py":
            continue
        if not import_admission_policy.admits_package_parent(
            parent,
            resolved,
            existing_modules=module_graph.keys(),
        ):
            continue
        module_graph[parent] = resolved
        added.add(parent)
        parent_parts = parent.split(".")
        for idx in range(1, len(parent_parts)):
            ancestor = ".".join(parent_parts[:idx])
            if ancestor not in module_graph:
                pending.add(ancestor)
    return added


def _static_string_sequence(node: ast.expr) -> tuple[str, ...] | None:
    if not isinstance(node, (ast.Tuple, ast.List)):
        return None
    out: list[str] = []
    for item in node.elts:
        if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
            return None
        out.append(item.value)
    return tuple(out)


def _static_module_all_exports(tree: ast.AST) -> tuple[str, ...] | None:
    body = getattr(tree, "body", ())
    exports: tuple[str, ...] | None = None
    for stmt in body:
        if isinstance(stmt, ast.Assign):
            if not any(
                isinstance(target, ast.Name) and target.id == "__all__"
                for target in stmt.targets
            ):
                continue
            sequence = _static_string_sequence(stmt.value)
            if sequence is None:
                return None
            exports = sequence
            continue
        if isinstance(stmt, ast.AnnAssign):
            if not isinstance(stmt.target, ast.Name) or stmt.target.id != "__all__":
                continue
            if stmt.value is None:
                return None
            sequence = _static_string_sequence(stmt.value)
            if sequence is None:
                return None
            exports = sequence
            continue
        if isinstance(stmt, (ast.AugAssign, ast.Delete)):
            targets = [stmt.target] if isinstance(stmt, ast.AugAssign) else stmt.targets
            if any(
                isinstance(target, ast.Name) and target.id == "__all__"
                for target in targets
            ):
                return None
        if isinstance(stmt, ast.Expr) and isinstance(stmt.value, ast.Call):
            func = stmt.value.func
            if (
                isinstance(func, ast.Attribute)
                and func.attr
                in {"append", "extend", "insert", "remove", "pop", "clear"}
                and isinstance(func.value, ast.Name)
                and func.value.id == "__all__"
            ):
                return None
    return exports


def _collect_import_star_modules(
    tree: ast.AST,
    module_name: str | None = None,
    is_package: bool = False,
    *,
    import_scan_mode: ImportScanMode = "full",
) -> tuple[str, ...]:
    if import_scan_mode not in {"full", "module_init"}:
        raise ValueError(f"unknown import scan mode: {import_scan_mode}")
    (
        package_override_set,
        package_override,
        spec_override_set,
        spec_override,
        spec_override_is_package,
    ) = _infer_module_overrides(tree)
    scan_nodes = (
        tuple(ast.walk(tree))
        if import_scan_mode == "full"
        else _module_init_scan_nodes(tree)
    )
    out: list[str] = []
    seen: set[str] = set()
    for node in scan_nodes:
        if not isinstance(node, ast.ImportFrom):
            continue
        if not any(alias.name == "*" for alias in node.names):
            continue
        resolved: str | None
        if node.level:
            if not module_name:
                continue
            resolved = _resolve_relative_import(
                module_name,
                is_package=is_package,
                level=node.level,
                module=node.module,
                package_override=package_override,
                package_override_set=package_override_set,
                spec_override=spec_override,
                spec_override_set=spec_override_set,
                spec_override_is_package=spec_override_is_package,
            )
        else:
            resolved = node.module
        if resolved and resolved not in seen:
            seen.add(resolved)
            out.append(resolved)
    return tuple(out)


def _expand_imports_with_static_package_all_star_children(
    imports: Collection[str],
    tree: ast.AST,
    *,
    module_name: str | None,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolution_cache: "_ModuleResolutionCache",
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
) -> tuple[str, ...]:
    out: list[str] = []
    seen: set[str] = set()

    def add(name: str) -> None:
        if name and name not in seen:
            seen.add(name)
            out.append(name)

    for name in imports:
        add(name)
    star_modules = _collect_import_star_modules(
        tree,
        module_name,
        is_package,
        import_scan_mode=import_scan_mode,
    )
    if not star_modules:
        return tuple(out)

    roots_list = list(roots)
    for star_module in star_modules:
        package_path = resolution_cache.resolve_module(
            star_module,
            roots_list,
            stdlib_root,
            stdlib_allowlist,
        )
        if package_path is None or package_path.name != "__init__.py":
            continue
        try:
            package_source = resolution_cache.read_module_source(
                package_path,
                retain=False,
            )
            package_tree = resolution_cache.parse_module_ast(
                package_path,
                package_source,
                filename=str(package_path),
                retain=False,
                target_python=target_python,
            )
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        exports = _static_module_all_exports(package_tree)
        if exports is None:
            continue
        for export_name in exports:
            child_name = f"{star_module}.{export_name}"
            if (
                resolution_cache.resolve_module(
                    child_name,
                    roots_list,
                    stdlib_root,
                    stdlib_allowlist,
                )
                is not None
            ):
                add(child_name)
    return tuple(out)


def _explicit_imports_reference_generated_importer(
    explicit_imports: Collection[str],
) -> bool:
    return any(
        name == IMPORTER_MODULE_NAME or name.startswith(f"{IMPORTER_MODULE_NAME}.")
        for name in explicit_imports
    )


def _module_uses_runtime_import_protocol(
    *,
    module_name: str,
    module_path: Path,
    module_resolution_cache: "_ModuleResolutionCache",
    target_python: TargetPythonVersion,
    import_scan_mode: ImportScanMode = "full",
    tree: ast.AST | None = None,
) -> bool:
    if module_name in _RUNTIME_IMPORT_PROTOCOL_IMPLEMENTATION_MODULES:
        return False
    is_package = module_path.name == "__init__.py"
    if tree is None:
        try:
            source = module_resolution_cache.read_module_source(
                module_path, retain=False
            )
        except (OSError, SyntaxError, UnicodeDecodeError):
            # Keep runtime import support enabled when analysis cannot prove the
            # graph is fully static.
            return True
        if not _source_may_use_runtime_import_protocol(source):
            return False
        try:
            tree = module_resolution_cache.parse_module_ast(
                module_path,
                source,
                filename=str(module_path),
                retain=False,
                target_python=target_python,
            )
        except SyntaxError:
            return True
    scan_nodes = (
        tuple(ast.walk(tree))
        if import_scan_mode == "full"
        else _module_init_scan_nodes(tree)
    )
    for node in scan_nodes:
        if isinstance(node, ast.Import):
            if any(alias.name != "_intrinsics" for alias in node.names):
                return True
            continue
        if isinstance(node, ast.ImportFrom):
            if node.module == "__future__":
                continue
            if node.level == 0 and (
                node.module == "_intrinsics"
                or (node.module is not None and node.module.endswith("._intrinsics"))
            ):
                continue
            return True
    return module_resolution_cache.uses_runtime_import_protocol(
        module_path,
        tree,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )


def _module_graph_needs_runtime_import_support(
    *,
    module_graph: Mapping[str, Path],
    module_resolution_cache: "_ModuleResolutionCache",
    explicit_imports: Collection[str],
    entry_module: str,
    entry_path: Path,
    entry_tree: ast.AST,
    target_python: TargetPythonVersion,
) -> _RuntimeImportSupportPolicy:
    needs_generated_importer = _explicit_imports_reference_generated_importer(
        explicit_imports
    )
    if needs_generated_importer:
        return _RuntimeImportSupportPolicy(
            needs_generated_importer=True,
            needs_runtime_import_support=True,
        )
    for module_name, module_path in sorted(module_graph.items()):
        tree = (
            entry_tree
            if module_name == entry_module and module_path == entry_path
            else None
        )
        import_scan_mode: ImportScanMode = (
            "full"
            if module_name == entry_module and module_path == entry_path
            else "module_init"
        )
        if _module_uses_runtime_import_protocol(
            module_name=module_name,
            module_path=module_path,
            module_resolution_cache=module_resolution_cache,
            target_python=target_python,
            import_scan_mode=import_scan_mode,
            tree=tree,
        ):
            return _RuntimeImportSupportPolicy(
                needs_generated_importer=False,
                needs_runtime_import_support=True,
            )
    return _RuntimeImportSupportPolicy(
        needs_generated_importer=False,
        needs_runtime_import_support=False,
    )


def _build_module_lowering_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
) -> tuple[
    dict[str, str],
    dict[str, str | None],
    dict[str, bool],
    dict[str, bool],
]:
    logical_source_path_by_module: dict[str, str] = {}
    entry_override_by_module: dict[str, str | None] = {}
    module_is_namespace_by_module: dict[str, bool] = {}
    module_is_package_by_module: dict[str, bool] = {}
    namespace_modules = set(namespace_module_names)
    for module_name in sorted(module_graph):
        module_path = module_graph[module_name]
        logical_source_path_by_module[module_name] = generated_module_source_paths.get(
            module_name, str(module_path)
        )
        # Every lowered module needs to know the canonical entry module name.
        # The frontend uses `entry_module` together with `module_name` to
        # recognize the real entry module and emit __main__ cache/name
        # semantics for it. Passing `None` for the entry module itself causes
        # `__name__` to lower as the ordinary module name instead of "__main__".
        entry_override_by_module[module_name] = entry_module
        module_is_namespace_by_module[module_name] = module_name in namespace_modules
        module_is_package_by_module[module_name] = module_path.name == "__init__.py"
    return (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    )


@functools.lru_cache(maxsize=8)
def _stdlib_allowlist_cached(project_root_text: str | None) -> frozenset[str]:
    allowlist: set[str] = set()
    spec_path = Path("docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md")
    if not spec_path.exists():
        if project_root_text:
            spec_path = (
                Path(project_root_text)
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
        else:
            spec_path = (
                _compiler_root()
                / "docs/spec/areas/compat/surfaces/stdlib/stdlib_surface_matrix.md"
            )
    if not spec_path.exists():
        return frozenset(allowlist)
    for line in spec_path.read_text().splitlines():
        if not line.startswith("|"):
            continue
        if line.startswith("| ---"):
            continue
        parts = [part.strip() for part in line.strip().strip("|").split("|")]
        if not parts:
            continue
        module_name = parts[0]
        if not module_name or module_name == "Module":
            continue
        for entry in module_name.split("/"):
            entry = entry.strip()
            if entry:
                allowlist.add(entry)
    return frozenset(allowlist)


def _stdlib_allowlist() -> set[str]:
    project_root = os.environ.get("MOLT_PROJECT_ROOT")
    return set(_stdlib_allowlist_cached(project_root))


_INTRINSIC_CALL_NAMES = {
    "load_intrinsic",
    "require_intrinsic",
    "require_optional_intrinsic",
    "_load_intrinsic",
    "_intrinsic_load",
    "_intrinsics_require",
    "_intrinsic_require",
    "_require_intrinsic",
    "_require_callable_intrinsic",
}


_STDLIB_PROBE_INTRINSIC = "molt_stdlib_probe"


_STDLIB_POLICY_GATE_STATUS = "policy-gate"


def _is_fail_closed_import_policy_gate(text: str) -> bool:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return False
    body = list(tree.body)
    if (
        body
        and isinstance(body[0], ast.Expr)
        and isinstance(body[0].value, ast.Constant)
        and isinstance(body[0].value.value, str)
    ):
        body = body[1:]
    while (
        body and isinstance(body[0], ast.ImportFrom) and body[0].module == "__future__"
    ):
        body = body[1:]
    if len(body) != 1 or not isinstance(body[0], ast.Raise):
        return False
    exc = body[0].exc
    if isinstance(exc, ast.Call):
        exc = exc.func
    if isinstance(exc, ast.Name):
        return exc.id == "ImportError"
    if isinstance(exc, ast.Attribute):
        return exc.attr == "ImportError"
    return False


def _module_required_intrinsic_names(path: Path) -> frozenset[str]:
    """Return every ``molt_*`` intrinsic a module statically requires.

    Walks the module source for ``require_intrinsic`` / ``load_intrinsic`` /
    ``_lazy_intrinsic`` style calls (see ``_INTRINSIC_CALL_NAMES``) and collects
    the string-literal first argument when it names an intrinsic. This is the
    same extraction used to classify a module's intrinsic status *and* to decide
    whether a feature-gated module is buildable on the selected profile, so the
    two views never disagree. Returns an empty set on read/parse failure (a
    module that cannot be parsed requires no intrinsics from this analysis).
    """
    try:
        source = path.read_text(encoding="utf-8")
    except Exception:
        return frozenset()
    try:
        tree = ast.parse(source)
    except SyntaxError:
        return frozenset()

    intrinsic_names: set[str] = set()
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        call_name: str | None = None
        if isinstance(node.func, ast.Name):
            call_name = node.func.id
        elif isinstance(node.func, ast.Attribute):
            call_name = node.func.attr
        if call_name not in _INTRINSIC_CALL_NAMES and call_name != "_lazy_intrinsic":
            continue
        first: ast.expr | None = None
        if node.args:
            first = node.args[0]
        else:
            for keyword in node.keywords:
                if keyword.arg == "name":
                    first = keyword.value
                    break
        if not isinstance(first, ast.Constant) or not isinstance(first.value, str):
            continue
        name = first.value
        if name.startswith("molt_"):
            intrinsic_names.add(name)
    return frozenset(intrinsic_names)


def _stdlib_module_intrinsic_status(path: Path) -> str:
    if path.name == "_intrinsics.py":
        return "intrinsic-backed"
    try:
        text = path.read_text(encoding="utf-8")
    except Exception:
        return "python-only"

    intrinsic_names = _module_required_intrinsic_names(path)
    if not intrinsic_names:
        if _is_fail_closed_import_policy_gate(text):
            return _STDLIB_POLICY_GATE_STATUS
        return "python-only"
    if intrinsic_names == {_STDLIB_PROBE_INTRINSIC}:
        return "probe-only"
    return "intrinsic-backed"


def _enforce_intrinsic_stdlib(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    json_output: bool,
) -> int | None:
    missing: list[str] = []
    probe_only: list[str] = []
    stdlib_root = stdlib_root.resolve()
    for name, path in module_graph.items():
        if not path or not path.suffix == ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        status = _stdlib_module_intrinsic_status(path)
        if status == "python-only":
            missing.append(name)
        elif status == "probe-only":
            probe_only.append(name)
    if not missing:
        return None
    missing.sort()
    probe_only.sort()
    message = (
        "Intrinsic-only stdlib enforcement failed. These modules are Python-only "
        "and must be lowered to Rust intrinsics (or become thin intrinsic wrappers):\n"
        + "\n".join(f"  - {name}" for name in missing)
    )
    if probe_only:
        message += (
            "\n\nProbe-only modules in this build (thin wrappers + policy gate only):\n"
            + "\n".join(f"  - {name}" for name in probe_only)
        )
    return _fail(message, json_output, command="build")


def _profile_feature_gap_for_module(
    path: Path,
    enabled_features: frozenset[str],
) -> dict[str, list[str]]:
    """Map each excluded gating feature this module needs to its intrinsics.

    For every ``molt_*`` intrinsic *path* statically requires, resolve the
    Cargo feature whose absence would remove the runtime *symbol definition*
    from the linked archive (``None`` ⇒ always linked: an ungated symbol such as
    the deliberately-ungated ``molt_ssl_*`` ABI, OR a resolver-only feature
    whose ``#[unsafe(no_mangle)]`` definition is compiled unconditionally — see
    ``LINK_AFFECTING_FEATURES``). A link-affecting feature that is NOT in
    *enabled_features* would leave the intrinsic undefined at link. The result
    maps each such excluded feature to the sorted intrinsics that need it
    (empty ⇒ buildable on this profile).
    """
    gap: dict[str, set[str]] = {}
    for symbol in _module_required_intrinsic_names(path):
        feature = link_affecting_feature_gate_for_symbol(symbol)
        if feature is None or feature in enabled_features:
            continue
        gap.setdefault(feature, set()).add(symbol)
    return {feature: sorted(symbols) for feature, symbols in gap.items()}


def _enforce_profile_feature_availability(
    module_graph: dict[str, Path],
    stdlib_root: Path,
    stdlib_profile: str | None,
    target: str,
    json_output: bool,
) -> int | None:
    """Refuse, loudly and at compile time, builds whose import graph needs a
    runtime feature the selected profile excludes.

    Domain-feature-gated stdlib modules (``ast`` → ``stdlib_ast``, ``sqlite3``
    → ``sqlite``, …) call runtime intrinsics that are ``#[cfg(feature = ...)]``
    -gated. Feature selection is **profile-driven, not import-driven**
    (``_runtime_builtin_features_for_profile``): the native ``micro`` profile
    excludes the heavy domains to keep tiny binaries small. Without this gate,
    importing such a module on a profile that excludes its feature surfaces only
    as an opaque undefined-symbol **linker error** late in the build. This pass
    makes the loud-refusal doctrine executable: it detects the gap from the
    static import graph and refuses with an actionable remedy *before any link
    is attempted*.

    The enabled-feature set is computed with the exact function the runtime
    staticlib build uses, so the refusal can never disagree with what actually
    links.
    """
    # The wasm staticlib excludes a slightly different set on the micro profile;
    # derive the effective triple from the target the same way the build does so
    # the enabled-feature computation matches the linked runtime exactly.
    # `_runtime_builtin_features_for_profile` keys the wasm distinction on a
    # `wasm32`-prefixed triple, so map the symbolic wasm targets (and an explicit
    # wasm32 triple, if ever passed) to one.
    is_wasm = target in {"wasm", "wasm-freestanding"} or target.startswith("wasm32")
    effective_triple = "wasm32-wasip1" if is_wasm else None
    enabled_features = frozenset(
        _runtime_builtin_features_for_profile(
            stdlib_profile,
            target_triple=effective_triple,
        )
    )
    profile_name = stdlib_profile or "micro"
    stdlib_root = stdlib_root.resolve()

    # feature -> {module -> [intrinsics]} so one message can group every module
    # blocked by the same excluded feature.
    blocked: dict[str, dict[str, list[str]]] = {}
    for name, path in module_graph.items():
        if not path or path.suffix != ".py":
            continue
        try:
            path.resolve().relative_to(stdlib_root)
        except ValueError:
            continue
        gap = _profile_feature_gap_for_module(path, enabled_features)
        for feature, symbols in gap.items():
            blocked.setdefault(feature, {})[name] = symbols
    if not blocked:
        return None

    lines: list[str] = []
    for feature in sorted(blocked):
        modules = blocked[feature]
        module_list = ", ".join(repr(m) for m in sorted(modules))
        plural = "module" if len(modules) == 1 else "modules"
        lines.append(f"  {feature}: required by {plural} {module_list}")
        for module_name in sorted(modules):
            sample = ", ".join(modules[module_name][:4])
            more = len(modules[module_name]) - 4
            if more > 0:
                sample += f", … (+{more} more)"
            lines.append(f"      {module_name} → {sample}")

    excluded_features = sorted(blocked)
    feature_phrase = (
        f"the {excluded_features[0]!r} runtime feature"
        if len(excluded_features) == 1
        else "runtime features " + ", ".join(repr(f) for f in excluded_features)
    )
    message = (
        f"Profile '{profile_name}' excludes {feature_phrase} that this program's "
        f"import graph requires.\n"
        f"These statically-imported stdlib modules need a feature profile "
        f"'{profile_name}' does not build, so their runtime intrinsics would be "
        f"undefined at link:\n"
        + "\n".join(lines)
        + "\n\nFeature selection is profile-driven, not import-driven: the "
        "native 'micro' profile omits heavy domains (ast, crypto, "
        "compression, …) to keep small binaries small.\n"
        "Rebuild with the full stdlib profile, which includes these features:\n"
        "    --stdlib-profile full\n"
        "or set the environment knob the build reads as its canonical profile:\n"
        "    MOLT_STDLIB_PROFILE=full"
    )
    return _fail(message, json_output, command="build")


# Core modules always included in the module graph.  The micro profile
# restricts this to the absolute minimum needed to run pure-computation
# benchmarks (builtins + sys).  Everything else is still available via lazy
# initialisation if user code actually imports it.
_CORE_STDLIB_MODULES_FULL = (
    "builtins",
    "sys",
    "types",
    "importlib",
    "importlib.util",
    "importlib.machinery",
)


_CORE_STDLIB_MODULES_MICRO = (
    "builtins",
    "sys",
)


def _ensure_core_stdlib_modules(
    module_graph: dict[str, Path], stdlib_root: Path
) -> None:
    stdlib_profile = os.environ.get("MOLT_STDLIB_PROFILE", "micro")
    if stdlib_profile == "micro":
        core_modules = _CORE_STDLIB_MODULES_MICRO
    else:
        core_modules = _CORE_STDLIB_MODULES_FULL
    for name in core_modules:
        path = _resolve_module_path(name, [stdlib_root])
        if path is not None:
            module_graph.setdefault(name, path)


def _record_module_reason(
    module_reasons: MutableMapping[str, set[str]],
    module_name: str,
    reason: str,
) -> None:
    module_reasons.setdefault(module_name, set()).add(reason)


def _extend_module_graph_with_closure(
    module_graph: MutableMapping[str, Path],
    *,
    entry_paths: Sequence[Path],
    roots: Sequence[Path],
    module_roots: Sequence[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    resolver_cache: "_ModuleResolutionCache",
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> frozenset[str]:
    if not entry_paths:
        return frozenset()
    closure_graph, _ = _discover_module_graph_from_paths(
        entry_paths,
        list(roots),
        list(module_roots),
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if diagnostics_enabled:
        for name, path in closure_graph.items():
            _record_module_reason(module_reasons, name, reason)
            module_graph.setdefault(name, path)
        return frozenset(closure_graph)
    for name, path in closure_graph.items():
        module_graph.setdefault(name, path)
    return frozenset(closure_graph)


def _resolve_static_import_module_paths(
    *,
    module_names: Collection[str],
    roots: Sequence[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    resolver_cache: "_ModuleResolutionCache",
    import_admission_policy: _ImportAdmissionPolicy | None,
) -> tuple[dict[str, Path], list[str]]:
    resolved: dict[str, Path] = {}
    errors: list[str] = []
    for module_name in sorted(module_names):
        path = resolver_cache.resolve_module(
            module_name,
            list(roots),
            stdlib_root,
            stdlib_allowlist,
        )
        if path is None:
            errors.append(
                f"{STATIC_IMPORT_MODULES_ENV} module {module_name!r} was not found"
            )
            continue
        if import_admission_policy is not None and not (
            import_admission_policy.admits_import(
                module_name,
                path,
                from_entry_path=False,
            )
        ):
            errors.append(
                f"{STATIC_IMPORT_MODULES_ENV} module {module_name!r} resolves under "
                "an external root but is not within an admitted external static package"
            )
            continue
        resolved[module_name] = path
    return resolved, errors


def _extend_module_graph_with_static_import_modules(
    *,
    module_graph: MutableMapping[str, Path],
    explicit_imports: set[str],
    module_names: Collection[str],
    roots: Sequence[Path],
    module_roots: Sequence[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    resolver_cache: "_ModuleResolutionCache",
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    import_admission_policy: _ImportAdmissionPolicy | None,
    target_python: TargetPythonVersion,
    capability_config_digest: str = "",
) -> list[str]:
    if not module_names:
        return []
    resolved, errors = _resolve_static_import_module_paths(
        module_names=module_names,
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolver_cache,
        import_admission_policy=import_admission_policy,
    )
    if errors:
        return errors
    explicit_imports.update(module_names)
    _extend_module_graph_with_closure(
        module_graph,
        entry_paths=tuple(resolved.values()),
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=resolver_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        reason="explicit_static_import",
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=False,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    return []


def _record_new_module_reasons(
    module_graph: Mapping[str, Path],
    before_names: set[str],
    module_reasons: MutableMapping[str, set[str]],
    reason: str,
) -> None:
    for name in module_graph:
        if name in before_names:
            continue
        _record_module_reason(module_reasons, name, reason)


def _looks_like_stdlib_module_name(module_name: str) -> bool:
    if module_name == "molt.stdlib" or module_name.startswith("molt.stdlib."):
        return True
    root = module_name.split(".", 1)[0]
    return root in {
        "__future__",
        "_collections_abc",
        "abc",
        "builtins",
        "collections",
        "dataclasses",
        "importlib",
        "os",
        "pathlib",
        "runpy",
        "signal",
        "sys",
        "test",
        "typing",
        "warnings",
        "zipfile",
        "zipimport",
    }


def _runtime_owned_module_roots() -> tuple[Path, ...]:
    stdlib_root = _stdlib_root_path().resolve()
    package_root = stdlib_root.parent
    return (
        stdlib_root,
        package_root / "gpu",
        package_root / "lib",
    )


def _is_runtime_owned_module_path(module_path: Path) -> bool:
    return any(
        _is_path_within(module_path, root) for root in _runtime_owned_module_roots()
    )


def _build_module_source_catalog(
    module_graph: Mapping[str, Path],
    *,
    module_sources: Mapping[str, str] | None = None,
    path_stats: Mapping[str, os.stat_result | None] | None = None,
) -> _ModuleSourceCatalog:
    leases: dict[str, _ModuleSourceLease] = {}
    module_sources = module_sources or {}
    for module_name, module_path in module_graph.items():
        path_stat = path_stats.get(module_name) if path_stats is not None else None
        inline_source = module_sources.get(module_name)
        if inline_source is not None:
            leases[module_name] = _ModuleSourceLease.inline(
                module_path, inline_source, path_stat
            )
        else:
            leases[module_name] = _ModuleSourceLease.path_backed(module_path, path_stat)
    return _ModuleSourceCatalog(leases=leases)


def _build_frontend_module_costs(
    module_names: Collection[str],
    *,
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _ModuleSourceCatalog | None = None,
    module_graph: Mapping[str, Path] | None = None,
    module_deps: Mapping[str, set[str]],
) -> dict[str, float]:
    module_costs: dict[str, float] = {}
    for module_name in sorted(module_names):
        source_size = 0
        if module_source_catalog is not None:
            source_size = module_source_catalog.source_size(
                module_name,
                module_graph.get(module_name) if module_graph is not None else None,
            )
        elif module_sources is not None:
            source_size = len(module_sources.get(module_name, ""))
        source_cost = max(1.0, float(source_size))
        dep_cost = float(max(0, len(module_deps.get(module_name, set()))) * 512)
        module_costs[module_name] = source_cost + dep_cost
    return module_costs


def _build_stdlib_like_module_flags(
    module_graph: Mapping[str, Path],
) -> dict[str, bool]:
    return {
        module_name: (
            _is_runtime_owned_module_path(module_path)
            or _looks_like_stdlib_module_name(module_name)
        )
        for module_name, module_path in sorted(module_graph.items())
    }


def _build_module_graph_metadata(
    module_graph: Mapping[str, Path],
    *,
    generated_module_source_paths: Mapping[str, str],
    entry_module: str,
    namespace_module_names: Collection[str],
    module_sources: Mapping[str, str] | None = None,
    module_source_catalog: _ModuleSourceCatalog | None = None,
    module_deps: Mapping[str, set[str]] | None = None,
) -> _ModuleGraphMetadata:
    (
        logical_source_path_by_module,
        entry_override_by_module,
        module_is_namespace_by_module,
        module_is_package_by_module,
    ) = _build_module_lowering_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=namespace_module_names,
    )
    frontend_module_costs = None
    if module_deps is not None and (
        module_sources is not None or module_source_catalog is not None
    ):
        frontend_module_costs = _build_frontend_module_costs(
            module_graph,
            module_sources=module_sources,
            module_source_catalog=module_source_catalog,
            module_graph=module_graph,
            module_deps=module_deps,
        )
    stdlib_like_by_module = (
        _build_stdlib_like_module_flags(module_graph)
        if module_deps is not None
        else None
    )
    return _ModuleGraphMetadata(
        logical_source_path_by_module=MappingProxyType(logical_source_path_by_module),
        entry_override_by_module=MappingProxyType(entry_override_by_module),
        module_is_namespace_by_module=MappingProxyType(module_is_namespace_by_module),
        module_is_package_by_module=MappingProxyType(module_is_package_by_module),
        frontend_module_costs=(
            MappingProxyType(frontend_module_costs)
            if frontend_module_costs is not None
            else None
        ),
        stdlib_like_by_module=(
            MappingProxyType(stdlib_like_by_module)
            if stdlib_like_by_module is not None
            else None
        ),
    )


def _requires_spawn_entry_override(
    module_graph: Mapping[str, Path], explicit_imports: Collection[str]
) -> bool:
    names: set[str] = set(module_graph)
    names.update(explicit_imports)
    for name in names:
        if name == ENTRY_OVERRIDE_SPAWN or name.startswith("multiprocessing."):
            return True
        if name == "multiprocessing":
            return True
    return False


def _module_graph_import_scan_mode(
    *,
    path: Path,
    module_name: str,
    entry_paths: frozenset[Path],
    nested_scan_modules: Collection[str],
    resolution_cache: _ModuleResolutionCache,
) -> ImportScanMode:
    resolved_path = resolution_cache.resolved_path(path)
    if resolved_path in entry_paths or module_name in nested_scan_modules:
        return "full"
    return "module_init"


def _discover_module_graph_from_paths(
    entry_paths: Sequence[Path],
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _ModuleResolutionCache | None = None,
    precomputed_imports_by_path: Mapping[Path, Collection[str]] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[dict[str, Path], set[str]]:
    entry_paths = tuple(entry_paths)
    if not entry_paths:
        return {}, set()
    graph: dict[str, Path] = {}
    skip_modules = skip_modules or set()
    stub_parents = stub_parents or set()
    nested_stdlib_scan_modules = (
        STDLIB_NESTED_IMPORT_SCAN_MODULES
        if nested_stdlib_scan_modules is None
        else nested_stdlib_scan_modules
    )
    explicit_imports: set[str] = set()
    seen_import_names: set[str] = set()
    queue = list(reversed(entry_paths))
    queued_paths = set(entry_paths)
    resolution_cache = resolver_cache or _ModuleResolutionCache()
    import_admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    resolved_entry_paths = frozenset(
        resolution_cache.resolved_path(path) for path in entry_paths
    )

    persisted_graph_paths: dict[str, Path] = {}
    dirty_persisted_modules: set[str] = set()
    use_persisted_graph_cache = project_root is not None and len(entry_paths) == 1
    if use_persisted_graph_cache:
        cache_project_root = project_root
        assert cache_project_root is not None
        entry_path = entry_paths[0]
        persisted_graph = _read_persisted_module_graph(
            cache_project_root,
            entry_path,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            skip_modules=skip_modules,
            stub_parents=stub_parents,
            nested_stdlib_scan_modules=nested_stdlib_scan_modules,
            stdlib_allowlist=stdlib_allowlist,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=allow_entry_external_imports,
            resolution_cache=resolution_cache,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted_graph is not None:
            if not persisted_graph.dirty_modules:
                return persisted_graph.graph, persisted_graph.explicit_imports
            persisted_graph_paths = dict(persisted_graph.graph)
            dirty_persisted_modules = set(persisted_graph.dirty_modules)

    def resolve_candidate(candidate: str) -> Path | None:
        persisted_path = persisted_graph_paths.get(candidate)
        if persisted_path is not None and candidate not in dirty_persisted_modules:
            return persisted_path
        return resolution_cache.resolve_module(
            candidate, roots, stdlib_root, stdlib_allowlist
        )

    while queue:
        path = queue.pop()
        queued_paths.discard(path)
        module_name = resolution_cache.module_name_from_path(
            path, module_roots, stdlib_root
        )
        if module_name in graph:
            continue
        graph[module_name] = path
        is_package = path.name == "__init__.py"
        import_scan_mode = _module_graph_import_scan_mode(
            path=path,
            module_name=module_name,
            entry_paths=resolved_entry_paths,
            nested_scan_modules=nested_stdlib_scan_modules,
            resolution_cache=resolution_cache,
        )
        precomputed_imports = (
            precomputed_imports_by_path.get(path)
            if precomputed_imports_by_path is not None
            else None
        )
        if precomputed_imports is not None:
            imports = precomputed_imports
            if use_persisted_graph_cache:
                with contextlib.suppress(OSError):
                    _write_persisted_import_scan(
                        cache_project_root,
                        path,
                        module_name=module_name,
                        is_package=is_package,
                        import_scan_mode=import_scan_mode,
                        imports=imports,
                        target_python=target_python,
                        capability_config_digest=capability_config_digest,
                    )
        else:
            persisted_imports = None
            if project_root is not None:
                persisted_imports = _read_persisted_import_scan(
                    project_root,
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                )
            if persisted_imports is None:
                try:
                    source = resolution_cache.read_module_source(path)
                except (OSError, SyntaxError, UnicodeDecodeError):
                    continue
                try:
                    tree = resolution_cache.parse_module_ast(
                        path,
                        source,
                        filename=str(path),
                        target_python=target_python,
                    )
                except SyntaxError:
                    continue
                imports = _load_module_imports(
                    path,
                    module_name=module_name,
                    is_package=is_package,
                    import_scan_mode=import_scan_mode,
                    tree=tree,
                    resolution_cache=resolution_cache,
                    project_root=project_root,
                    roots=roots,
                    stdlib_root=stdlib_root,
                    stdlib_allowlist=stdlib_allowlist,
                    target_python=target_python,
                    capability_config_digest=capability_config_digest,
                )
            else:
                imports = persisted_imports
        for name in imports:
            if name in seen_import_names:
                continue
            seen_import_names.add(name)
            explicit_imports.add(name)
            for candidate in _expand_module_chain_cached(name):
                if candidate in stub_parents:
                    continue
                if candidate.split(".", 1)[0] in skip_modules:
                    continue
                if any(
                    candidate == prefix or candidate.startswith(prefix + ".")
                    for prefix in PLATFORM_EXCLUDED_SUBMODULES
                ):
                    continue
                resolved = resolve_candidate(candidate)
                if resolved is None or resolved in queued_paths:
                    continue
                from_entry_path = (
                    allow_entry_external_imports
                    and resolution_cache.resolved_path(path) in resolved_entry_paths
                )
                if not import_admission_policy.admits_import(
                    candidate,
                    resolved,
                    from_entry_path=from_entry_path,
                ):
                    continue
                if resolved not in queued_paths:
                    queued_paths.add(resolved)
                    queue.append(resolved)
    if use_persisted_graph_cache:
        with contextlib.suppress(OSError):
            _write_persisted_module_graph(
                cache_project_root,
                entry_paths[0],
                roots=roots,
                module_roots=module_roots,
                stdlib_root=stdlib_root,
                skip_modules=skip_modules,
                stub_parents=stub_parents,
                nested_stdlib_scan_modules=nested_stdlib_scan_modules,
                stdlib_allowlist=stdlib_allowlist,
                import_admission_policy=import_admission_policy,
                allow_entry_external_imports=allow_entry_external_imports,
                graph=graph,
                explicit_imports=explicit_imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return graph, explicit_imports


def _discover_module_graph(
    entry_path: Path,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    stdlib_allowlist: set[str],
    skip_modules: set[str] | None = None,
    stub_parents: set[str] | None = None,
    nested_stdlib_scan_modules: set[str] | None = None,
    resolver_cache: _ModuleResolutionCache | None = None,
    precomputed_imports: Collection[str] | None = None,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[dict[str, Path], set[str]]:
    precomputed_imports_by_path = (
        {entry_path: precomputed_imports} if precomputed_imports is not None else None
    )
    return _discover_module_graph_from_paths(
        (entry_path,),
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        resolver_cache=resolver_cache,
        precomputed_imports_by_path=precomputed_imports_by_path,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )


@functools.lru_cache(maxsize=4096)
def _resolved_module_cache_key(path_str: str, *parts: str) -> str:
    return hashlib.sha256(
        "|".join((str(Path(path_str).resolve()), *parts)).encode("utf-8")
    ).hexdigest()[:24]


_MODULE_GRAPH_CACHE_SCHEMA_VERSION = 7


_IMPORT_SCAN_CACHE_SCHEMA_VERSION = 7


def _module_graph_policy_digest(
    stdlib_allowlist: Collection[str],
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    *,
    allow_entry_external_imports: bool = True,
) -> str:
    admission_policy = import_admission_policy or _ImportAdmissionPolicy()
    payload = json.dumps(
        {
            "stdlib_allowlist": sorted(stdlib_allowlist),
            "import_admission": admission_policy.digest_payload(),
            "allow_entry_external_imports": allow_entry_external_imports,
        },
        sort_keys=True,
        separators=(",", ":"),
    )
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()[:24]


@functools.lru_cache(maxsize=1024)
def _module_graph_cache_key(
    entry_path: str,
    roots: tuple[str, ...],
    module_roots: tuple[str, ...],
    stdlib_root: str,
    skip_modules: tuple[str, ...],
    stub_parents: tuple[str, ...],
    nested_stdlib_scan_modules: tuple[str, ...],
    stdlib_allowlist_digest: str,
    compiler_fingerprint: str,
    target_python_tag: str = _DEFAULT_TARGET_PYTHON_VERSION.tag,
    capability_config_digest: str = "",
) -> str:
    payload: dict[str, Any] = {
        "version": _MODULE_GRAPH_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": compiler_fingerprint,
        "entry_path": str(Path(entry_path).resolve()),
        "roots": [str(Path(path).resolve()) for path in roots],
        "module_roots": [str(Path(path).resolve()) for path in module_roots],
        "stdlib_root": str(Path(stdlib_root).resolve()),
        "skip_modules": list(skip_modules),
        "stub_parents": list(stub_parents),
        "nested_stdlib_scan_modules": list(nested_stdlib_scan_modules),
        "stdlib_allowlist_digest": stdlib_allowlist_digest,
        "target_python": target_python_tag,
    }
    if capability_config_digest:
        payload["capability_config_digest"] = capability_config_digest
    return hashlib.sha256(
        json.dumps(
            payload,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    ).hexdigest()[:24]


def _import_scan_cache_path(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        "import_scan_cache",
    )
    key_parts = [
        module_name,
        "pkg" if is_package else "mod",
        import_scan_mode,
        target_python.tag,
        _cache_tooling_fingerprint(),
    ]
    if capability_config_digest:
        key_parts.append(f"capability_config={capability_config_digest}")
    cache_key = _resolved_module_cache_key(
        os.fspath(path),
        *key_parts,
    )
    return root / f"{path.stem}.{cache_key}.json"


def _module_graph_cache_path(
    project_root: Path,
    entry_path: Path,
    *,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    skip_modules: set[str],
    stub_parents: set[str],
    nested_stdlib_scan_modules: set[str],
    stdlib_allowlist: set[str],
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        "module_graph_cache",
    )
    cache_key = _module_graph_cache_key(
        os.fspath(entry_path),
        tuple(os.fspath(path) for path in roots),
        tuple(os.fspath(path) for path in module_roots),
        os.fspath(stdlib_root),
        tuple(sorted(skip_modules)),
        tuple(sorted(stub_parents)),
        tuple(sorted(nested_stdlib_scan_modules)),
        _module_graph_policy_digest(
            stdlib_allowlist,
            import_admission_policy,
            allow_entry_external_imports=allow_entry_external_imports,
        ),
        _cache_tooling_fingerprint(),
        target_python.tag,
        capability_config_digest=capability_config_digest,
    )
    return root / f"{entry_path.stem}.{cache_key}.json"


def _read_persisted_module_graph(
    project_root: Path,
    entry_path: Path,
    *,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    skip_modules: set[str],
    stub_parents: set[str],
    nested_stdlib_scan_modules: set[str],
    stdlib_allowlist: set[str],
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    resolution_cache: _ModuleResolutionCache | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> _PersistedModuleGraphState | None:
    cache_path = _module_graph_cache_path(
        project_root,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        stdlib_allowlist=stdlib_allowlist,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    payload = _read_cached_json_object(cache_path)
    if payload is None:
        return None
    if (
        not isinstance(payload, dict)
        or payload.get("version") != _MODULE_GRAPH_CACHE_SCHEMA_VERSION
        or payload.get("compiler_fingerprint") != _cache_tooling_fingerprint()
        or payload.get("capability_config_digest", "") != capability_config_digest
    ):
        return None
    raw_modules = payload.get("modules")
    if not isinstance(raw_modules, list):
        return None
    graph: dict[str, Path] = {}
    dirty_modules: set[str] = set()
    for item in raw_modules:
        if not isinstance(item, dict):
            return None
        module_name = item.get("module")
        path_text = item.get("path")
        size = item.get("size")
        mtime_ns = item.get("mtime_ns")
        source_sha256 = item.get("source_sha256")
        if (
            not isinstance(module_name, str)
            or not isinstance(path_text, str)
            or not isinstance(size, int)
            or not isinstance(mtime_ns, int)
            or not isinstance(source_sha256, str)
        ):
            return None
        path = Path(path_text)
        if not _case_exact_file(path):
            dirty_modules.add(module_name)
            graph[module_name] = path
            continue
        try:
            stat = (
                resolution_cache.path_stat(path)
                if resolution_cache is not None
                else path.stat()
            )
        except OSError:
            dirty_modules.add(module_name)
            graph[module_name] = path
            continue
        if (
            stat.st_size != size
            or stat.st_mtime_ns != mtime_ns
            or _source_content_sha256(path, stat) != source_sha256
        ):
            dirty_modules.add(module_name)
        graph[module_name] = path
    raw_explicit_imports = payload.get("explicit_imports", [])
    if not isinstance(raw_explicit_imports, list) or not all(
        isinstance(name, str) for name in raw_explicit_imports
    ):
        return None
    return _PersistedModuleGraphState(
        graph=graph,
        explicit_imports=set(cast(list[str], raw_explicit_imports)),
        dirty_modules=dirty_modules,
    )


def _write_persisted_module_graph(
    project_root: Path,
    entry_path: Path,
    *,
    roots: list[Path],
    module_roots: list[Path],
    stdlib_root: Path,
    skip_modules: set[str],
    stub_parents: set[str],
    nested_stdlib_scan_modules: set[str],
    stdlib_allowlist: set[str],
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    allow_entry_external_imports: bool = True,
    graph: dict[str, Path],
    explicit_imports: set[str],
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> None:
    modules: list[dict[str, Any]] = []
    for module_name, path in sorted(graph.items()):
        if not _case_exact_file(path):
            return
        stat = path.stat()
        source_sha256 = _source_content_sha256(path, stat)
        if source_sha256 is None:
            return
        modules.append(
            {
                "module": module_name,
                "path": str(path),
                "size": stat.st_size,
                "mtime_ns": stat.st_mtime_ns,
                "source_sha256": source_sha256,
            }
        )
    payload = {
        "version": _MODULE_GRAPH_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": _cache_tooling_fingerprint(),
        "capability_config_digest": capability_config_digest,
        "modules": modules,
        "explicit_imports": sorted(explicit_imports),
    }
    cache_path = _module_graph_cache_path(
        project_root,
        entry_path,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        skip_modules=skip_modules,
        stub_parents=stub_parents,
        nested_stdlib_scan_modules=nested_stdlib_scan_modules,
        stdlib_allowlist=stdlib_allowlist,
        import_admission_policy=import_admission_policy,
        allow_entry_external_imports=allow_entry_external_imports,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_cached_json_object(cache_path, payload)


def _read_persisted_import_scan(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    path_stat: os.stat_result | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[str, ...] | None:
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    payload = _read_artifact_sync_state(cache_path)
    if payload is None:
        return None
    if (
        payload.get("version") != _IMPORT_SCAN_CACHE_SCHEMA_VERSION
        or payload.get("compiler_fingerprint") != _cache_tooling_fingerprint()
        or payload.get("import_scan_mode") != import_scan_mode
        or payload.get("capability_config_digest", "") != capability_config_digest
    ):
        return None
    if path_stat is None:
        try:
            path_stat = path.stat()
        except OSError:
            return None
    imports = payload.get("imports")
    if not isinstance(imports, list) or not all(
        isinstance(item, str) for item in imports
    ):
        return None
    if not _payload_source_matches(payload, path, path_stat):
        return None
    return tuple(imports)


def _write_persisted_import_scan(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    imports: Iterable[str],
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> None:
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    stat = path.stat()
    source_sha256 = _source_content_sha256(path, stat)
    if source_sha256 is None:
        return
    payload = {
        "version": _IMPORT_SCAN_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": _cache_tooling_fingerprint(),
        "capability_config_digest": capability_config_digest,
        "module_name": module_name,
        "is_package": is_package,
        "import_scan_mode": import_scan_mode,
        "target_python": target_python.tag,
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "source_sha256": source_sha256,
        "imports": list(imports),
    }
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_artifact_sync_payload(cache_path, payload)


def _load_module_imports(
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    tree: ast.AST,
    resolution_cache: _ModuleResolutionCache,
    project_root: Path | None,
    roots: Sequence[Path] | None = None,
    stdlib_root: Path | None = None,
    stdlib_allowlist: set[str] | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[str, ...]:
    if project_root is not None:
        persisted_imports = _read_persisted_import_scan(
            project_root,
            path,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if persisted_imports is not None:
            return persisted_imports
    imports = resolution_cache.collect_imports(
        path,
        tree,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
    )
    if roots is not None and stdlib_root is not None and stdlib_allowlist is not None:
        imports = _expand_imports_with_static_package_all_star_children(
            imports,
            tree,
            module_name=module_name,
            is_package=is_package,
            import_scan_mode=import_scan_mode,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolution_cache=resolution_cache,
            target_python=target_python,
        )
    if project_root is not None:
        with contextlib.suppress(OSError):
            _write_persisted_import_scan(
                project_root,
                path,
                module_name=module_name,
                is_package=is_package,
                import_scan_mode=import_scan_mode,
                imports=imports,
                target_python=target_python,
                capability_config_digest=capability_config_digest,
            )
    return imports


def _augment_support_modules(
    *,
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    roots: list[Path],
    stdlib_root: Path,
    stdlib_allowlist: set[str],
    explicit_imports: Collection[str],
    resolver_cache: "_ModuleResolutionCache",
    artifacts_root: Path,
    stub_parents: Collection[str],
    entry_module: str,
    needs_generated_importer: bool,
    diagnostics_enabled: bool,
) -> _SupportModuleAugmentation:
    namespace_parents = _collect_namespace_parents(
        module_graph,
        roots,
        stdlib_root,
        stdlib_allowlist,
        explicit_imports,
        resolver_cache=resolver_cache,
    )
    namespace_modules: dict[str, Path] = {}
    if namespace_parents:
        for name in sorted(namespace_parents):
            paths = _namespace_paths(
                name,
                _roots_for_module(name, roots, stdlib_root, stdlib_allowlist),
            )
            if not paths:
                continue
            stub_path = _write_namespace_module(name, paths, artifacts_root)
            namespace_modules[name] = stub_path
        if namespace_modules:
            module_graph.update(namespace_modules)
            if diagnostics_enabled:
                for name in namespace_modules:
                    _record_module_reason(module_reasons, name, "namespace_stub")
    generated_module_source_paths: dict[str, str] = {
        name: _logical_generated_module_path(name) for name in namespace_modules
    }
    for stub in stub_parents:
        if stub != entry_module and stub in namespace_modules:
            module_graph.pop(stub, None)
    if needs_generated_importer and IMPORTER_MODULE_NAME not in module_graph:
        importer_path = _write_importer_module(artifacts_root)
        module_graph[IMPORTER_MODULE_NAME] = importer_path
        if diagnostics_enabled:
            _record_module_reason(
                module_reasons, IMPORTER_MODULE_NAME, "importer_generated"
            )
    if needs_generated_importer and IMPORTER_MODULE_NAME in module_graph:
        generated_module_source_paths.setdefault(
            IMPORTER_MODULE_NAME, _logical_generated_module_path(IMPORTER_MODULE_NAME)
        )
    return _SupportModuleAugmentation(
        namespace_module_names=frozenset(namespace_modules),
        generated_module_source_paths=generated_module_source_paths,
    )


def _materialize_import_plan(
    *,
    prepared_module_graph: _PreparedEntryModuleGraph,
    module_reasons: MutableMapping[str, set[str]],
    stdlib_root: Path,
    artifacts_root: Path,
    entry_module: str,
    diagnostics_enabled: bool,
) -> _ImportPlan:
    module_graph = dict(prepared_module_graph.module_graph)
    stdlib_allowlist = set(prepared_module_graph.stdlib_allowlist)
    stub_parents = set(prepared_module_graph.stub_parents)
    support_modules = _augment_support_modules(
        module_graph=module_graph,
        module_reasons=module_reasons,
        roots=list(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        explicit_imports=prepared_module_graph.explicit_imports,
        resolver_cache=prepared_module_graph.module_resolution_cache,
        artifacts_root=artifacts_root,
        stub_parents=stub_parents,
        entry_module=entry_module,
        needs_generated_importer=(
            prepared_module_graph.runtime_import_support_policy.needs_generated_importer
        ),
        diagnostics_enabled=diagnostics_enabled,
    )
    namespace_module_names = support_modules.namespace_module_names
    generated_module_source_paths = dict(support_modules.generated_module_source_paths)
    known_modules = frozenset(module_graph)
    stdlib_allowlist.update(STUB_MODULES)
    stdlib_allowlist.update(stub_parents)
    stdlib_allowlist.add("molt.stdlib")
    module_graph_metadata = _build_module_graph_metadata(
        module_graph,
        generated_module_source_paths=generated_module_source_paths,
        entry_module=entry_module,
        namespace_module_names=set(namespace_module_names),
    )
    return _ImportPlan(
        stdlib_allowlist=frozenset(stdlib_allowlist),
        roots=tuple(prepared_module_graph.roots),
        stdlib_root=stdlib_root,
        module_resolution_cache=prepared_module_graph.module_resolution_cache,
        module_graph=MappingProxyType(dict(module_graph)),
        explicit_imports=frozenset(prepared_module_graph.explicit_imports),
        runtime_import_dispatch_roots=frozenset(
            prepared_module_graph.runtime_import_dispatch_roots
        ),
        stub_parents=frozenset(stub_parents),
        spawn_enabled=prepared_module_graph.spawn_enabled,
        runtime_import_support_policy=prepared_module_graph.runtime_import_support_policy,
        namespace_module_names=namespace_module_names,
        generated_module_source_paths=MappingProxyType(generated_module_source_paths),
        known_modules=known_modules,
        known_modules_sorted=tuple(sorted(known_modules)),
        stdlib_allowlist_sorted=tuple(sorted(stdlib_allowlist)),
        module_graph_metadata=module_graph_metadata,
        native_artifact_plan=prepared_module_graph.native_artifact_plan,
    )


def _augment_module_graph_for_entry_and_runtime(
    *,
    source_path: Path,
    entry_module: str,
    module_roots: Sequence[Path],
    stdlib_root: Path,
    roots: Sequence[Path],
    project_root: Path | None,
    stdlib_allowlist: set[str],
    entry_imports: Collection[str],
    module_resolution_cache: "_ModuleResolutionCache",
    module_graph: MutableMapping[str, Path],
    module_reasons: MutableMapping[str, set[str]],
    diagnostics_enabled: bool,
    json_output: bool,
    target: str,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[_ModuleGraphAugmentation, _CliFailure | None]:
    roots = list(roots)
    module_roots = list(module_roots)
    entry_imports = set(entry_imports)
    explicit_imports = set(entry_imports)
    stub_skip_modules = STUB_MODULES - entry_imports
    stub_parents = STUB_PARENT_MODULES - entry_imports
    stdlib_profile = os.environ.get("MOLT_STDLIB_PROFILE", "micro")
    if stdlib_profile == "micro":
        core_module_names = _CORE_STDLIB_MODULES_MICRO
    else:
        core_module_names = _CORE_STDLIB_MODULES_FULL
    core_paths = [
        path
        for name in core_module_names
        if (path := module_graph.get(name)) is not None
    ]
    _extend_module_graph_with_closure(
        module_graph,
        entry_paths=core_paths,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=module_resolution_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        reason="core_closure",
        skip_modules=stub_skip_modules,
        stub_parents=stub_parents,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    spawn_enabled = False
    spawn_required = target != "wasm" and _requires_spawn_entry_override(
        module_graph, explicit_imports
    )
    if spawn_required:
        spawn_path = module_resolution_cache.resolve_module(
            ENTRY_OVERRIDE_SPAWN,
            roots,
            stdlib_root,
            stdlib_allowlist,
        )
        if spawn_path is None:
            return _ModuleGraphAugmentation(
                spawn_enabled=False,
                explicit_imports=explicit_imports,
                stub_parents=stub_parents,
            ), _fail(
                (
                    f"Missing required stdlib module: {ENTRY_OVERRIDE_SPAWN}. "
                    "multiprocessing spawn entry override cannot be lowered."
                ),
                json_output,
                command="build",
            )
        spawn_enabled = True
        _extend_module_graph_with_closure(
            module_graph,
            entry_paths=[spawn_path],
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            project_root=project_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="spawn_closure",
            skip_modules=stub_skip_modules,
            stub_parents=stub_parents,
            import_admission_policy=import_admission_policy,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
    return _ModuleGraphAugmentation(
        spawn_enabled=spawn_enabled,
        explicit_imports=explicit_imports,
        stub_parents=stub_parents,
    ), None


def _prepare_entry_module_graph(
    *,
    source_path: Path,
    entry_module: str,
    module_roots: list[Path],
    stdlib_root: Path,
    project_root: Path | None,
    entry_tree: ast.AST,
    diagnostics_enabled: bool,
    module_reasons: MutableMapping[str, set[str]],
    json_output: bool,
    target: str,
    import_admission_policy: _ImportAdmissionPolicy | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> tuple[_PreparedEntryModuleGraph | None, _CliFailure | None]:
    stdlib_allowlist = _stdlib_allowlist()
    roots = module_roots + [stdlib_root]
    module_resolution_cache = _ModuleResolutionCache()
    entry_is_package = source_path.name == "__init__.py"
    entry_imports = _expand_imports_with_static_package_all_star_children(
        tuple(_collect_imports(entry_tree, entry_module, entry_is_package)),
        entry_tree,
        module_name=entry_module,
        is_package=entry_is_package,
        import_scan_mode="full",
        roots=roots,
        stdlib_root=stdlib_root,
        stdlib_allowlist=stdlib_allowlist,
        resolution_cache=module_resolution_cache,
        target_python=target_python,
    )
    module_graph, explicit_imports = _discover_module_graph(
        source_path,
        roots,
        module_roots,
        stdlib_root,
        project_root,
        stdlib_allowlist,
        skip_modules=STUB_MODULES,
        stub_parents=STUB_PARENT_MODULES,
        resolver_cache=module_resolution_cache,
        precomputed_imports=entry_imports,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if diagnostics_enabled:
        for name in module_graph:
            _record_module_reason(module_reasons, name, "entry_closure")
    static_import_modules, static_import_error = _parse_static_import_modules(
        os.environ.get(STATIC_IMPORT_MODULES_ENV, "")
    )
    if static_import_error is not None:
        return None, _fail(static_import_error, json_output, command="build")
    static_import_errors = _extend_module_graph_with_static_import_modules(
        module_graph=module_graph,
        explicit_imports=explicit_imports,
        module_names=static_import_modules,
        roots=roots,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        resolver_cache=module_resolution_cache,
        diagnostics_enabled=diagnostics_enabled,
        module_reasons=module_reasons,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if static_import_errors:
        return None, _fail(
            "; ".join(static_import_errors),
            json_output,
            command="build",
        )
    while True:
        package_before = set(module_graph)
        added_package_parents = _collect_package_parents(
            module_graph,
            roots=roots,
            stdlib_root=stdlib_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            import_admission_policy=import_admission_policy,
        )
        if diagnostics_enabled:
            _record_new_module_reasons(
                module_graph,
                package_before,
                module_reasons,
                "package_parent",
            )
        if not added_package_parents:
            break
        package_parent_paths = [
            module_graph[name]
            for name in sorted(added_package_parents)
            if name in module_graph
        ]
        before_parent_closure = set(module_graph)
        _extend_module_graph_with_closure(
            module_graph,
            entry_paths=package_parent_paths,
            roots=roots,
            module_roots=module_roots,
            stdlib_root=stdlib_root,
            project_root=project_root,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="package_parent_closure",
            skip_modules=STUB_MODULES,
            stub_parents=STUB_PARENT_MODULES,
            import_admission_policy=import_admission_policy,
            allow_entry_external_imports=False,
            target_python=target_python,
            capability_config_digest=capability_config_digest,
        )
        if diagnostics_enabled:
            _record_new_module_reasons(
                module_graph,
                before_parent_closure,
                module_reasons,
                "package_parent_closure",
            )
    core_before = set(module_graph)
    _ensure_core_stdlib_modules(module_graph, stdlib_root)
    if diagnostics_enabled:
        _record_new_module_reasons(
            module_graph,
            core_before,
            module_reasons,
            "core_required",
        )
    intrinsic_enforced = _enforce_intrinsic_stdlib(
        module_graph, stdlib_root, json_output
    )
    if intrinsic_enforced is not None:
        return None, intrinsic_enforced
    # MOLT_STDLIB_PROFILE is the single canonical profile signal the module-graph
    # construction and the runtime staticlib build both read (see the
    # `--stdlib-profile` propagation note); use the same source here so the
    # feature-availability refusal matches the profile that will actually link.
    feature_availability_enforced = _enforce_profile_feature_availability(
        module_graph,
        stdlib_root,
        os.environ.get("MOLT_STDLIB_PROFILE", "micro"),
        target,
        json_output,
    )
    if feature_availability_enforced is not None:
        return None, feature_availability_enforced
    augmentation, augmentation_error = _augment_module_graph_for_entry_and_runtime(
        source_path=source_path,
        entry_module=entry_module,
        module_roots=module_roots,
        stdlib_root=stdlib_root,
        roots=roots,
        project_root=project_root,
        stdlib_allowlist=stdlib_allowlist,
        entry_imports=explicit_imports,
        module_resolution_cache=module_resolution_cache,
        module_graph=module_graph,
        module_reasons=module_reasons,
        diagnostics_enabled=diagnostics_enabled,
        json_output=json_output,
        target=target,
        import_admission_policy=import_admission_policy,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    if augmentation_error is not None:
        return None, augmentation_error
    runtime_import_dispatch_roots: set[str] = set(augmentation.explicit_imports)
    runtime_import_support_policy = _module_graph_needs_runtime_import_support(
        module_graph=module_graph,
        module_resolution_cache=module_resolution_cache,
        explicit_imports=augmentation.explicit_imports,
        entry_module=entry_module,
        entry_path=source_path,
        entry_tree=entry_tree,
        target_python=target_python,
    )
    if runtime_import_support_policy.needs_runtime_import_support:
        import_support_paths: list[Path] = []
        for module_name in _RUNTIME_IMPORT_SUPPORT_ROOT_MODULES:
            module_path = _resolve_module_path(module_name, [stdlib_root])
            if module_path is None:
                return None, _fail(
                    f"Missing required stdlib support module: {module_name}",
                    json_output,
                    command="build",
                )
            import_support_paths.append(module_path)
        before_support = set(module_graph)
        support_closure_modules = _extend_module_graph_with_closure(
            module_graph,
            entry_paths=import_support_paths,
            roots=[stdlib_root],
            module_roots=[stdlib_root],
            stdlib_root=stdlib_root,
            project_root=None,
            stdlib_allowlist=stdlib_allowlist,
            resolver_cache=module_resolution_cache,
            diagnostics_enabled=diagnostics_enabled,
            module_reasons=module_reasons,
            reason="runtime_import_support",
            import_admission_policy=import_admission_policy,
            target_python=target_python,
        )
        runtime_import_dispatch_roots.update(support_closure_modules)
        if diagnostics_enabled:
            _record_new_module_reasons(
                module_graph,
                before_support,
                module_reasons,
                "runtime_import_support",
            )
    return _PreparedEntryModuleGraph(
        stdlib_allowlist=stdlib_allowlist,
        roots=roots,
        module_resolution_cache=module_resolution_cache,
        module_graph=dict(module_graph),
        explicit_imports=augmentation.explicit_imports,
        runtime_import_dispatch_roots=frozenset(runtime_import_dispatch_roots),
        stub_parents=augmentation.stub_parents,
        spawn_enabled=augmentation.spawn_enabled,
        runtime_import_support_policy=runtime_import_support_policy,
        native_artifact_plan=(
            import_admission_policy.native_artifact_plan
            if import_admission_policy is not None
            else _EMPTY_EXTERNAL_PACKAGE_NATIVE_ARTIFACT_PLAN
        ),
    ), None

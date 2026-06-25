from __future__ import annotations

from collections.abc import Collection, Iterable
import functools
import hashlib
import json
import os
from pathlib import Path
from typing import Any, NamedTuple, cast

from molt.cli.artifact_state import _build_state_subdir_cached
from molt.cli.backend_cache import (
    _read_artifact_sync_state,
    _write_artifact_sync_payload,
)
from molt.cli.cache_fingerprints import _cache_tooling_fingerprint
from molt.cli.json_cache import _read_cached_json_object, _write_cached_json_object
from molt.cli import module_resolution as _module_resolution
from molt.cli import module_source as _module_source
from molt.cli.models import (
    ImportScanMode,
    _ImportAdmissionPolicy,
)
from molt.cli.runtime_paths import _build_state_root
from molt.cli.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
)


class _PersistedModuleGraphState(NamedTuple):
    graph: dict[str, Path]
    explicit_imports: set[str]
    dirty_modules: set[str]


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
    resolution_cache: _module_resolution._ModuleResolutionCache | None = None,
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
        if not _module_resolution._case_exact_file(path):
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
            or _module_source._source_content_sha256(path, stat) != source_sha256
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
        if not _module_resolution._case_exact_file(path):
            return
        stat = path.stat()
        source_sha256 = _module_source._source_content_sha256(path, stat)
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
    if not _module_source._payload_source_matches(payload, path, path_stat):
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
    source_sha256 = _module_source._source_content_sha256(path, stat)
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

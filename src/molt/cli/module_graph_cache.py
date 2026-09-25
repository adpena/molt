from __future__ import annotations

import functools
import hashlib
import os
from pathlib import Path
from typing import Any

from molt.cli.artifact_state import _build_state_subdir_cached
from molt.cli.artifact_sync import (
    _read_artifact_sync_state,
    _write_artifact_sync_payload,
)
from molt.cli.cache_fingerprints import (
    _frontend_semantic_tooling_fingerprint,
    _source_tree_fingerprint_transaction,
)
from molt.cli import module_source as _module_source
from molt.cli.models import (
    ImportScanMode,
    _ImportScanRequests,
    _StaticSourceExecutionRequest,
    _StaticSourcePath,
)
from molt.cli.runtime_paths import _build_state_root
from molt.target_python import TargetPythonVersion, _DEFAULT_TARGET_PYTHON_VERSION


@functools.lru_cache(maxsize=4096)
def _resolved_module_cache_key(path_str: str, *parts: str) -> str:
    return hashlib.sha256(
        "|".join((str(Path(path_str).resolve()), *parts)).encode("utf-8")
    ).hexdigest()[:24]


# Completed projections from older schemas are never admitted as source requests.
_IMPORT_SCAN_CACHE_SCHEMA_VERSION = 11


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
        _frontend_semantic_tooling_fingerprint(),
    ]
    if capability_config_digest:
        key_parts.append(f"capability_config={capability_config_digest}")
    cache_key = _resolved_module_cache_key(
        os.fspath(path),
        *key_parts,
    )
    return root / f"{path.stem}.{cache_key}.json"


def _encode_source_path(path: str | _StaticSourcePath) -> Any:
    if isinstance(path, str):
        return path
    return {
        "operation": path.operation,
        "parts": [_encode_source_path(part) for part in path.parts],
    }


def _decode_source_path(payload: Any) -> str | _StaticSourcePath:
    if isinstance(payload, str):
        return payload
    if not isinstance(payload, dict):
        raise ValueError("invalid source path request")
    operation = payload.get("operation")
    parts = payload.get("parts")
    if (
        not isinstance(operation, str)
        or operation
        not in {
            "path",
            "join",
            "resolve",
            "absolute",
            "os_join",
            "posix_join",
            "nt_join",
        }
        or not isinstance(parts, list)
        or not parts
        or operation not in {"join", "os_join", "posix_join", "nt_join"}
        and len(parts) != 1
    ):
        raise ValueError("invalid source path operation")
    return _StaticSourcePath(
        operation, tuple(_decode_source_path(part) for part in parts)
    )


@_source_tree_fingerprint_transaction()
def _read_persisted_import_scan_record(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    path_stat: os.stat_result | None = None,
    snapshot: _module_source.PythonSourceSnapshot | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> _ImportScanRequests | None:
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
        or payload.get("compiler_fingerprint")
        != _frontend_semantic_tooling_fingerprint()
        or payload.get("module_name") != module_name
        or payload.get("is_package") != is_package
        or payload.get("target_python") != target_python.tag
        or payload.get("import_scan_mode") != import_scan_mode
        or payload.get("capability_config_digest", "") != capability_config_digest
    ):
        return None
    if snapshot is not None:
        if (
            snapshot.path != path
            or payload.get("size") != len(snapshot.content)
            or payload.get("source_sha256") != snapshot.sha256
        ):
            return None
    else:
        try:
            if path_stat is None:
                path_stat = path.stat()
        except OSError:
            return None
        if not _module_source._payload_source_matches(payload, path, path_stat):
            return None
    fields = ("imports", "star_modules", "dynamic_relative_import_candidates")
    for field in fields:
        values = payload.get(field)
        if not isinstance(values, list) or not all(
            isinstance(item, str) for item in values
        ):
            return None
    requires_anchor = payload.get("requires_runtime_package_anchor")
    raw_executions = payload.get("source_executions")
    if not isinstance(requires_anchor, bool) or not isinstance(raw_executions, list):
        return None
    executions: list[_StaticSourceExecutionRequest] = []
    for item in raw_executions:
        if not isinstance(item, dict):
            return None
        name = item.get("module")
        if name is not None and not isinstance(name, str):
            return None
        try:
            request_path = _decode_source_path(item.get("path"))
        except (ValueError, RecursionError):
            return None
        executions.append(_StaticSourceExecutionRequest(name, request_path))
    return _ImportScanRequests(
        tuple(payload["imports"]),
        tuple(executions),
        tuple(payload["star_modules"]),
        tuple(payload["dynamic_relative_import_candidates"]),
        requires_anchor,
    )


@_source_tree_fingerprint_transaction()
def _write_persisted_import_scan(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    scan: _ImportScanRequests,
    snapshot: _module_source.PythonSourceSnapshot,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    capability_config_digest: str = "",
) -> None:
    if snapshot.path != path:
        raise ValueError("source scan snapshot path mismatch")
    identity = {"size": len(snapshot.content), "source_sha256": snapshot.sha256}
    # A publication is for the captured generation, never a later pathname hash.
    if not _module_source._payload_source_matches(identity, path, path.stat()):
        return
    payload = {
        "version": _IMPORT_SCAN_CACHE_SCHEMA_VERSION,
        "compiler_fingerprint": _frontend_semantic_tooling_fingerprint(),
        "capability_config_digest": capability_config_digest,
        "module_name": module_name,
        "is_package": is_package,
        "import_scan_mode": import_scan_mode,
        "target_python": target_python.tag,
        **identity,
        "imports": list(scan.imports),
        "star_modules": list(scan.star_modules),
        "source_executions": [
            {"module": request.module_name, "path": _encode_source_path(request.path)}
            for request in scan.source_executions
        ],
        "dynamic_relative_import_candidates": list(
            scan.dynamic_relative_import_candidates
        ),
        "requires_runtime_package_anchor": scan.requires_runtime_package_anchor,
    }
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        target_python=target_python,
        capability_config_digest=capability_config_digest,
    )
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_artifact_sync_payload(cache_path, payload)

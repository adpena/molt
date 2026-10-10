from __future__ import annotations

import functools
import hashlib
import os
import sys
from collections.abc import Mapping, MutableMapping
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import Any, cast, get_args

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
from molt import stdlib_intrinsic_policy as _intrinsic_policy
from molt.compiler_analysis.python_imports import (
    ImportResolutionError,
    StaticImportPlan,
    StaticImportRequest,
    UnresolvedStaticImportError,
)


def _encode_intrinsic_source_facts(
    facts: _intrinsic_policy.StdlibModuleIntrinsicFacts,
) -> dict[str, Any]:
    evidence = facts.import_evidence
    use = facts.intrinsic_use
    return {
        "status": facts.status,
        "modules": sorted(evidence.proven_modules),
        "private_imports": [list(item) for item in sorted(evidence.private_imports)],
        "used": sorted(use.used),
        "unread": [
            {"name": binding.name, "intrinsic": binding.intrinsic, "line": binding.line}
            for binding in use.unread_bindings
        ],
        "discarded": [list(item) for item in use.discarded],
        "unresolved": [
            {
                "line": line,
                "name": request.name,
                "level": request.level,
                "fromlist": list(request.fromlist),
                "modules": list(plan.modules),
                "errors": list(plan.errors),
                "runtime": plan.requires_runtime,
                "execution": plan.requires_runtime_execution,
            }
            for line, request, plan in evidence.unresolved_sites
        ],
        "facade": (
            [
                {
                    "export": binding.export_name,
                    "owner": binding.owner_module,
                    "imported": binding.imported_name,
                    "line": binding.line,
                }
                for binding in evidence.facade.bindings
            ]
            if evidence.facade is not None
            else None
        ),
    }


def _decode_intrinsic_source_facts(
    payload: Any, path: Path
) -> _intrinsic_policy.StdlibModuleIntrinsicFacts:
    """Admit only source facts; graph-resolved facade children are never stored."""

    def strings(value: Any) -> tuple[str, ...]:
        if not isinstance(value, list) or any(type(item) is not str for item in value):
            raise ValueError("invalid intrinsic source string sequence")
        return tuple(value)

    def record(value: Any, keys: set[str]) -> dict[str, Any]:
        if not isinstance(value, dict) or set(value) != keys:
            raise ValueError("invalid intrinsic source fact record")
        return value

    def line(value: Any) -> int:
        if type(value) is not int or value < 1:
            raise ValueError("invalid intrinsic source line")
        return value

    def pair(value: Any) -> tuple[str, str]:
        items = strings(value)
        if len(items) != 2:
            raise ValueError("invalid intrinsic source pair")
        return items[0], items[1]

    value = record(
        payload,
        {
            "status",
            "modules",
            "private_imports",
            "used",
            "unread",
            "discarded",
            "unresolved",
            "facade",
        },
    )
    if value["status"] not in (
        _intrinsic_policy.STATUS_INTRINSIC,
        _intrinsic_policy.STATUS_POLICY_GATE,
        _intrinsic_policy.STATUS_PYTHON_COMPILED,
        _intrinsic_policy.STATUS_STUB,
    ):
        raise ValueError("invalid intrinsic source status")
    modules = strings(value["modules"])
    if not isinstance(value["private_imports"], list):
        raise ValueError("invalid intrinsic private imports")
    private_imports = frozenset(pair(item) for item in value["private_imports"])
    if not isinstance(value["unread"], list) or not isinstance(
        value["discarded"], list
    ):
        raise ValueError("invalid intrinsic use record")
    unread = []
    for raw in value["unread"]:
        item = record(raw, {"name", "intrinsic", "line"})
        if type(item["name"]) is not str or type(item["intrinsic"]) is not str:
            raise ValueError("invalid intrinsic binding")
        unread.append(
            _intrinsic_policy.StdlibIntrinsicBinding(
                item["name"], item["intrinsic"], line(item["line"])
            )
        )
    discarded = []
    for raw in value["discarded"]:
        if not isinstance(raw, list) or len(raw) != 2 or type(raw[0]) is not str:
            raise ValueError("invalid discarded intrinsic requirement")
        discarded.append((raw[0], line(raw[1])))
    use = _intrinsic_policy.StdlibModuleIntrinsicUse(
        frozenset(strings(value["used"])), tuple(unread), tuple(discarded)
    )
    if not isinstance(value["unresolved"], list):
        raise ValueError("invalid intrinsic unresolved sites")
    unresolved = []
    for raw in value["unresolved"]:
        item = record(
            raw,
            {
                "line",
                "name",
                "level",
                "fromlist",
                "modules",
                "errors",
                "runtime",
                "execution",
            },
        )
        if (
            type(item["line"]) is not int
            or item["line"] < 0
            or type(item["level"]) is not int
            or item["level"] < 0
            or type(item["name"]) is not str
            or type(item["runtime"]) is not bool
            or type(item["execution"]) is not bool
        ):
            raise ValueError("invalid intrinsic unresolved site")
        errors = strings(item["errors"])
        if any(error not in get_args(ImportResolutionError) for error in errors):
            raise ValueError("invalid intrinsic import error")
        if not item["runtime"] and not errors:
            raise ValueError("intrinsic unresolved site has no obligation")
        unresolved.append(
            (
                item["line"],
                StaticImportRequest.statement(
                    item["name"],
                    level=item["level"],
                    fromlist=strings(item["fromlist"]),
                ),
                StaticImportPlan(
                    strings(item["modules"]),
                    cast(tuple[ImportResolutionError, ...], errors),
                    item["runtime"],
                    item["execution"],
                ),
            )
        )
    facade = None
    if value["facade"] is not None:
        if not isinstance(value["facade"], list) or not value["facade"]:
            raise ValueError("invalid intrinsic facade")
        bindings = []
        for raw in value["facade"]:
            item = record(raw, {"export", "owner", "imported", "line"})
            if (
                type(item["export"]) is not str
                or type(item["imported"]) is not str
                or item["owner"] is not None
                and type(item["owner"]) is not str
                or type(item["line"]) is not int
                or item["line"] < 1
            ):
                raise ValueError("invalid intrinsic facade binding")
            bindings.append(
                _intrinsic_policy.StdlibFacadeBinding(
                    item["export"], item["owner"], item["imported"], item["line"]
                )
            )
        facade = _intrinsic_policy.StdlibFacadeEvidence(tuple(bindings))
    return _intrinsic_policy.StdlibModuleIntrinsicFacts(
        value["status"],
        _intrinsic_policy.StdlibModuleImportEvidence(
            path, frozenset(modules), tuple(unresolved), facade, private_imports
        ),
        use,
    )


@_source_tree_fingerprint_transaction()
def _stdlib_intrinsic_source_facts(
    project_root: Path,
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
    operation_counts: MutableMapping[str, int] | None = None,
) -> _intrinsic_policy.StdlibModuleIntrinsicFacts:
    """Reuse a per-module source analysis, never a resolved graph or verdict."""

    def count(event: str) -> None:
        if operation_counts is not None:
            key = f"intrinsic_source_{event}"
            operation_counts[key] = operation_counts.get(key, 0) + 1

    count("requests")
    try:
        snapshot = _module_source.PythonSourceSnapshot.capture(path)
    except OSError as exc:
        raise UnresolvedStaticImportError(
            f"stdlib intrinsic import evidence ({module_name}: {path}, "
            f"Python {target_python.short}) cannot read source: {exc}"
        ) from exc
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=path.name == "__init__.py",
        import_scan_mode="full",
        target_python=target_python,
    ).with_suffix(".intrinsic.json")
    identity = {
        "schema": "molt.stdlib-intrinsic-source.v4",
        "source_sha256": snapshot.sha256,
        "compiler_fingerprint": _frontend_semantic_tooling_fingerprint(),
        "module_name": module_name,
        "is_package": path.name == "__init__.py",
        "target_python": target_python.tag,
        "parser": [sys.implementation.name, sys.version],
    }
    payload = _read_artifact_sync_state(cache_path)
    if payload is not None and payload.get("identity") == identity:
        try:
            facts = _decode_intrinsic_source_facts(payload.get("facts"), path)
        except ValueError:
            # A malformed cache is a miss. Source analysis errors still escape.
            count("rejected")
        else:
            count("hits")
            return facts
    count("misses")
    facts = _intrinsic_policy.stdlib_module_intrinsic_facts(
        module_name, path, target_python=target_python, source=snapshot.content
    )
    # The key and result describe the same captured bytes even if the pathname
    # changes during analysis. A subsequent read admits only its fresh digest.
    try:
        _write_artifact_sync_payload(
            cache_path,
            {"identity": identity, "facts": _encode_intrinsic_source_facts(facts)},
        )
    except OSError as exc:
        count("publication_failures")
        print(
            f"molt: warning: cannot cache intrinsic source facts for {path}: {exc}",
            file=sys.stderr,
        )
    return facts


# A worker costs about 0.3 s to start and import the analysis; a module costs
# about 25 ms to analyze. At 32 modules a worker, start-up stays under a third.
_INTRINSIC_FACTS_MODULES_PER_WORKER = 32
# One worker holds one module analysis; a whole serial run peaks near 300 MB.
_INTRINSIC_FACTS_BYTES_PER_WORKER = 256 * 1024 * 1024
_INTRINSIC_FACTS_MEMORY_HEADROOM_BYTES = 1024 * 1024 * 1024


def _warm_stdlib_intrinsic_source_facts_batch(
    project_root: str,
    batch: tuple[tuple[str, str], ...],
    target_python: TargetPythonVersion,
) -> None:
    # One transaction captures the tooling fingerprint once for the batch.
    with _source_tree_fingerprint_transaction():
        for module_name, path in batch:
            _stdlib_intrinsic_source_facts(
                Path(project_root), module_name, Path(path), target_python=target_python
            )


def warm_stdlib_intrinsic_source_facts(
    project_root: Path,
    module_paths: Mapping[str, Path],
    *,
    target_python: TargetPythonVersion,
) -> None:
    """Fill the per-module facts cache in parallel when the set is large.

    Each module's facts depend only on its own bytes, so workers may compute
    them in any order; the caller then classifies from the cache. A set too
    small to amortize worker start-up is left to the caller's serial pass.
    """
    from molt.dx import _memory_bounded_worker_count

    work = [
        (name, os.fspath(path))
        for name, path in sorted(module_paths.items())
        if path.suffix == ".py"
    ]
    workers = min(
        len(work) // _INTRINSIC_FACTS_MODULES_PER_WORKER,
        _memory_bounded_worker_count(
            bytes_per_worker=_INTRINSIC_FACTS_BYTES_PER_WORKER,
            headroom_bytes=_INTRINSIC_FACTS_MEMORY_HEADROOM_BYTES,
        ),
    )
    if workers <= 1:
        return
    # One interleaved batch a worker spreads the large modules across workers.
    batches = [tuple(work[index::workers]) for index in range(workers)]
    with ProcessPoolExecutor(max_workers=workers) as pool:
        for _ in pool.map(
            functools.partial(
                _warm_stdlib_intrinsic_source_facts_batch,
                os.fspath(project_root),
                target_python=target_python,
            ),
            batches,
        ):
            pass


@functools.lru_cache(maxsize=4096)
def _resolved_module_cache_key(path_str: str, *parts: str) -> str:
    return hashlib.sha256(
        "|".join((str(Path(path_str).resolve()), *parts)).encode("utf-8")
    ).hexdigest()[:24]


# Completed projections from older schemas are never admitted as source requests.
_IMPORT_SCAN_CACHE_SCHEMA_VERSION = 23


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
    fields = (
        "imports",
        "star_modules",
        "dynamic_relative_import_candidates",
        "dynamic_star_modules",
    )
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
        tuple(payload["dynamic_star_modules"]),
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
        "dynamic_star_modules": list(scan.dynamic_star_modules),
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

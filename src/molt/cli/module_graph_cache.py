from __future__ import annotations
from molt import input_report as _input_report
from molt.compiler_analysis.python_function_kind import FunctionKind
from molt.compiler_analysis.python_binding_facts import (
    PythonElementProtocol,
    PythonFunctionResult,
    PythonSourceElement,
)

import functools
import hashlib
import os
import sys
from dataclasses import asdict
from molt.compiler_analysis.python_imports import (
    RuntimeCatalogDeferral,
    SourceImportSite,
    SourceAllExports,
    SourceDemandPhase,
)
from collections.abc import Mapping, MutableMapping
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from itertools import pairwise
from typing import Any, cast, get_args

from molt.cli.artifact_state import _build_state_subdir_cached
from molt.cli.json_cache import (
    SOURCE_ANALYSIS_CACHE_MAX_BYTES,
    _read_cached_json_object,
    _write_cached_json_object,
)
from molt.cli.cache_fingerprints import (
    _frontend_semantic_tooling_fingerprint,
    _operation_resolved_path,
    _source_tree_fingerprint_transaction,
)
from molt.cli import module_source as _module_source
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    decode_python_source_key,
)
from molt.compiler_analysis.python_namespace_graph import PythonNamespacePremise
from molt.compiler_analysis.python_identity_catalog_generated import ALL_INVALID_MEMBERS
from molt.compiler_analysis.python_namespace_writes import (
    SUMMARY_SCHEMA,
    PythonNamespaceSummary,
    decode_python_namespace_write,
    python_namespace_write_payload,
    python_namespace_summary,
    python_package_name,
)
from molt.cli.models import ImportScanMode
from molt.compiler_analysis.python_imports import (
    _ImportScanRequests,
    _SourceValueDemand,
    _SourceImportBinding,
    _SourceValueSummary,
    _StaticSourceExecutionRequest,
    _StaticSourcePath,
)
from molt.cli.runtime_paths import _build_state_root
from molt.target_python import (
    TargetPythonVersion,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_source_for_target,
)
from molt import stdlib_intrinsic_policy as _intrinsic_policy
from molt.compiler_analysis.python_imports import (
    ImportResolutionError,
    ImportOperationKind,
    StaticValueKind,
    StaticMetadataValue,
    ModuleImportState,
    StaticImportPlan,
    StaticImportRequest,
    UnresolvedStaticImportError,
)


def _source_cache_strings(value: Any) -> tuple[str, ...]:
    if not isinstance(value, list) or any(type(item) is not str for item in value):
        raise ValueError("invalid intrinsic source string sequence")
    return tuple(value)


def _source_cache_sorted_strings(value: Any) -> tuple[str, ...]:
    names = _source_cache_strings(value)
    if any(left >= right for left, right in pairwise(names)):
        raise ValueError("invalid sorted source string sequence")
    return names


def _source_cache_list(value: Any) -> list[Any]:
    if not isinstance(value, list):
        raise ValueError("invalid source fact sequence")
    return value


def _source_cache_rows(value: Any, width: int) -> list[list[Any]]:
    rows = _source_cache_list(value)
    if any(not isinstance(row, list) or len(row) != width for row in rows):
        raise ValueError("invalid source fact rows")
    return rows


def _source_cache_origins(value: Any) -> frozenset[PythonSourceKey]:
    return frozenset(
        decode_python_source_key(item) for item in _source_cache_list(value)
    )


def _source_cache_record(value: Any, keys: set[str]) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError("invalid intrinsic source fact record")
    return value


def _source_cache_line(value: Any) -> int:
    if type(value) is not int or value < 1:
        raise ValueError("invalid intrinsic source line")
    return value


def _source_cache_pair(value: Any) -> tuple[str, str]:
    items = _source_cache_strings(value)
    if len(items) != 2:
        raise ValueError("invalid intrinsic source pair")
    return items[0], items[1]


def _source_cache_decode_plan(raw: Any) -> StaticImportPlan:
    item = _source_cache_record(raw, {"modules", "errors", "runtime", "execution"})
    errors = _source_cache_strings(item["errors"])
    if (
        any(error not in get_args(ImportResolutionError) for error in errors)
        or type(item["runtime"]) is not bool
        or type(item["execution"]) is not bool
    ):
        raise ValueError("invalid intrinsic import plan")
    return StaticImportPlan(
        _source_cache_strings(item["modules"]),
        cast(tuple[ImportResolutionError, ...], errors),
        item["runtime"],
        item["execution"],
    )


def _source_cache_metadata(raw: Any) -> StaticMetadataValue | None:
    if raw is None:
        return None
    item = _source_cache_record(raw, {"kind", "value"})
    if item["kind"] not in get_args(StaticValueKind) or (
        type(item["value"]) is not str
        if item["kind"] == "known"
        else item["value"] is not None
    ):
        raise ValueError("invalid import metadata")
    return StaticMetadataValue(item["kind"], item["value"])


def _source_cache_import_state(raw: Any) -> ModuleImportState | None:
    if raw is None:
        return None
    item = _source_cache_record(raw, {"package", "spec_parent", "name", "has_path"})
    package, spec_parent, name = (
        _source_cache_metadata(item[key]) for key in ("package", "spec_parent", "name")
    )
    if (
        package is None
        or spec_parent is None
        or name is None
        or (item["has_path"] is not None and type(item["has_path"]) is not bool)
    ):
        raise ValueError("invalid import state")
    return ModuleImportState(package, spec_parent, name, item["has_path"])


def _decode_source_request(raw):
    request = None
    if raw is not None:
        req = _source_cache_record(
            raw,
            {
                "kind",
                "name",
                "level",
                "fromlist",
                "package_argument",
                "globals_state",
                "globals_were_supplied",
                "level_is_invalid",
            },
        )
        if (
            req["kind"] not in get_args(ImportOperationKind)
            or type(req["name"]) is not str
            or type(req["level"]) is not int
            or type(req["globals_were_supplied"]) is not bool
            or type(req["level_is_invalid"]) is not bool
            or req["kind"] == "statement"
            and req["level"] < 0
        ):
            raise ValueError("invalid intrinsic import request")
        request = StaticImportRequest(
            req["kind"],
            req["name"],
            req["level"],
            _source_cache_strings(req["fromlist"]),
            _source_cache_metadata(req["package_argument"]),
            _source_cache_import_state(req["globals_state"]),
            req["globals_were_supplied"],
            req["level_is_invalid"],
        )
    return request


def _encode_source_plan(plan):
    return {
        "modules": list(plan.modules),
        "errors": list(plan.errors),
        "runtime": plan.requires_runtime,
        "execution": plan.requires_runtime_execution,
    }


def _decode_optional_source_key(raw):
    return None if raw is None else decode_python_source_key(raw)


def _encode_source_site(site):
    return {
        "source": site.source,
        "request": asdict(site.request) if site.request is not None else None,
        "plan": _encode_source_plan(site.plan),
        "owner_plan": _encode_source_plan(site.owner_plan),
        "bindings": site.bindings,
    }


def _decode_source_site(raw):
    item = _source_cache_record(
        raw,
        {"source", "request", "plan", "owner_plan", "bindings"},
    )
    request = _decode_source_request(item["request"])
    plan, owner = (
        _source_cache_decode_plan(item[name]) for name in ("plan", "owner_plan")
    )
    if request is None and (
        plan != StaticImportPlan((), (), True, True) or owner != plan
    ):
        raise ValueError("opaque import lost runtime obligation")
    if request is not None and not request.fromlist and plan != owner:
        raise ValueError("ordinary import has inconsistent owner plan")
    source = decode_python_source_key(item["source"])
    if not isinstance(item["bindings"], list):
        raise ValueError("invalid source import bindings")
    bindings = tuple(_source_cache_pair(pair) for pair in item["bindings"])
    if bindings and source[-1] != "ImportFrom":
        raise ValueError("export bindings lack from-import source")
    return SourceImportSite(
        source,
        request,
        plan,
        owner,
        bindings,
    )


def _decode_source_all(raw):
    item = _source_cache_record(raw, {"status", "names", "reason"})
    names = _source_cache_strings(item["names"])
    return SourceAllExports(item["status"], names, item["reason"])


def _encode_namespace_summary(summary: PythonNamespaceSummary) -> dict[str, Any]:
    return {
        "schema": SUMMARY_SCHEMA,
        "own_names": None if summary.own_names is None else sorted(summary.own_names),
        "foreign_writes": [
            python_namespace_write_payload(write) for write in summary.foreign_writes
        ],
        "own_reflection_line": summary.own_reflection_line,
    }


def _validate_import_scan_namespace_summary(
    summary: PythonNamespaceSummary | None, *, source_owner: PythonSourceKey | None
) -> None:
    if source_owner is None and summary is None:
        raise ValueError("persisted module import scan requires a namespace summary")


def _decode_namespace_summary(raw: Any) -> PythonNamespaceSummary:
    item = _source_cache_record(
        raw, {"schema", "own_names", "foreign_writes", "own_reflection_line"}
    )
    if type(item["schema"]) is not int or item["schema"] != SUMMARY_SCHEMA:
        raise ValueError("invalid namespace summary schema")
    own_names = item["own_names"]
    if own_names is not None:
        own_names = frozenset(_source_cache_sorted_strings(own_names))
    line = item["own_reflection_line"]
    if type(line) is not int or line < 0 or (own_names is None) != (line > 0):
        raise ValueError("invalid namespace reflection line")
    writes = [
        decode_python_namespace_write(row)
        for row in _source_cache_rows(item["foreign_writes"], 5)
    ]
    if len(set(writes)) != len(writes):
        raise ValueError("repeated foreign namespace write")
    return PythonNamespaceSummary(own_names, tuple(writes), line)


def _encode_intrinsic_source_facts(
    facts: _intrinsic_policy.StdlibModuleIntrinsicFacts,
) -> dict[str, Any]:
    evidence = facts.import_evidence
    use = facts.intrinsic_use

    return {
        "status": facts.status,
        "namespace_summary": _encode_namespace_summary(facts.namespace_summary),
        "private_imports": [list(item) for item in sorted(evidence.private_imports)],
        "used": sorted(use.used),
        "unread": [
            {"name": binding.name, "intrinsic": binding.intrinsic, "line": binding.line}
            for binding in use.unread_bindings
        ],
        "discarded": [list(item) for item in use.discarded],
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
    payload: Any, path: Path, source_scan: _ImportScanRequests
) -> _intrinsic_policy.StdlibModuleIntrinsicFacts:
    """Admit only source facts; graph-resolved facade children are never stored."""

    value = _source_cache_record(
        payload,
        {
            "status",
            "namespace_summary",
            "private_imports",
            "used",
            "unread",
            "discarded",
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
    if not isinstance(value["private_imports"], list):
        raise ValueError("invalid intrinsic private imports")
    private_imports = frozenset(
        _source_cache_pair(item) for item in value["private_imports"]
    )
    if not isinstance(value["unread"], list) or not isinstance(
        value["discarded"], list
    ):
        raise ValueError("invalid intrinsic use record")
    unread = []
    for raw in value["unread"]:
        item = _source_cache_record(raw, {"name", "intrinsic", "line"})
        if type(item["name"]) is not str or type(item["intrinsic"]) is not str:
            raise ValueError("invalid intrinsic binding")
        unread.append(
            _intrinsic_policy.StdlibIntrinsicBinding(
                item["name"], item["intrinsic"], _source_cache_line(item["line"])
            )
        )
    discarded = []
    for raw in value["discarded"]:
        if not isinstance(raw, list) or len(raw) != 2 or type(raw[0]) is not str:
            raise ValueError("invalid discarded intrinsic requirement")
        discarded.append((raw[0], _source_cache_line(raw[1])))
    use = _intrinsic_policy.StdlibModuleIntrinsicUse(
        frozenset(_source_cache_strings(value["used"])), tuple(unread), tuple(discarded)
    )

    facade = None
    if value["facade"] is not None:
        if not isinstance(value["facade"], list) or not value["facade"]:
            raise ValueError("invalid intrinsic facade")
        bindings = []
        for raw in value["facade"]:
            item = _source_cache_record(raw, {"export", "owner", "imported", "line"})
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
            path, source_scan, facade, private_imports
        ),
        use,
        _decode_namespace_summary(value["namespace_summary"]),
    )


def _namespace_premise_cache_identity(
    premise: PythonNamespacePremise | None,
) -> dict[str, object] | None:
    if premise is None:
        return None
    return {
        "foreign_writes": (
            None if premise.foreign_writes is None else list(premise.foreign_writes)
        ),
        "member_writes": premise.member_writes,
    }


def _decode_namespace_premise_cache_identity(
    raw: Any,
) -> PythonNamespacePremise | None:
    if raw is None:
        return None
    item = _source_cache_record(raw, {"foreign_writes", "member_writes"})
    foreign = item["foreign_writes"]
    if foreign is not None:
        foreign = _source_cache_sorted_strings(foreign)
    members = item["member_writes"]
    if type(members) is not int or members < 0 or members & ~ALL_INVALID_MEMBERS:
        raise ValueError("invalid namespace premise member mask")
    return PythonNamespacePremise(foreign, members)


def _intrinsic_source_identity(
    snapshot: _module_source.PythonSourceSnapshot,
    module_name: str,
    target_python: TargetPythonVersion,
    namespace_premise: PythonNamespacePremise | None,
) -> dict[str, Any]:
    return {
        "schema": "molt.stdlib-intrinsic-source.v8",
        "source_sha256": snapshot.sha256,
        "compiler_fingerprint": _frontend_semantic_tooling_fingerprint(),
        "module_name": module_name,
        "is_package": snapshot.path.name == "__init__.py",
        "target_python": target_python.tag,
        "namespace_premise": _namespace_premise_cache_identity(namespace_premise),
        "parser": [sys.implementation.name, sys.version],
    }


@_source_tree_fingerprint_transaction()
def _stdlib_intrinsic_namespace_summary(
    project_root: Path,
    module_name: str,
    path: Path,
    *,
    target_python: TargetPythonVersion,
) -> PythonNamespaceSummary:
    """Read syntax-only facts without requiring or inventing a graph premise."""
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
    expected = _intrinsic_source_identity(snapshot, module_name, target_python, None)
    payload = _read_cached_json_object(
        cache_path, max_bytes=SOURCE_ANALYSIS_CACHE_MAX_BYTES
    )
    if payload is not None:
        identity = payload.get("identity")
        if (
            isinstance(identity, dict)
            and identity.keys() == expected.keys()
            and all(
                identity[key] == value
                for key, value in expected.items()
                if key != "namespace_premise"
            )
        ):
            try:
                facts = payload.get("facts")
                if not isinstance(facts, dict):
                    raise ValueError("invalid intrinsic source facts")
                return _decode_namespace_summary(facts.get("namespace_summary"))
            except ValueError:
                # Malformed syntax facts are a miss, never an empty summary.
                pass
    try:
        tree = _parse_source_for_target(
            snapshot.content, filename=str(path), target_python=target_python
        )
    except SyntaxError as exc:
        raise UnresolvedStaticImportError(
            f"stdlib intrinsic import evidence ({module_name}: {path}:{exc.lineno}, "
            f"Python {target_python.short}) cannot parse source: {exc.msg}"
        ) from exc
    return python_namespace_summary(
        tree,
        package=python_package_name(module_name, is_package=path.name == "__init__.py"),
        target_python=target_python.feature_version,
    )


@_source_tree_fingerprint_transaction()
def _stdlib_intrinsic_source_facts(
    project_root: Path,
    module_name: str,
    path: Path,
    namespace_premise: PythonNamespacePremise | None = None,
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
    identity = _intrinsic_source_identity(
        snapshot, module_name, target_python, namespace_premise
    )
    payload = _read_cached_json_object(
        cache_path, max_bytes=SOURCE_ANALYSIS_CACHE_MAX_BYTES
    )
    if payload is not None and payload.get("identity") == identity:
        try:
            if (
                _decode_namespace_premise_cache_identity(
                    payload["identity"]["namespace_premise"]
                )
                != namespace_premise
            ):
                raise ValueError("intrinsic metadata namespace premise mismatch")
            scan = _read_persisted_import_scan_record(
                project_root,
                path,
                module_name=module_name,
                is_package=path.name == "__init__.py",
                import_scan_mode="full",
                namespace_premise=namespace_premise,
                snapshot=snapshot,
                target_python=target_python,
            )
            if scan is None:
                raise ValueError("intrinsic metadata lacks current source requests")
            facts = _decode_intrinsic_source_facts(payload.get("facts"), path, scan)
        except ValueError:
            # A malformed cache is a miss. Source analysis errors still escape.
            count("rejected")
        else:
            count("hits")
            return facts
    count("misses")
    facts = _intrinsic_policy.stdlib_module_intrinsic_facts(
        module_name,
        path,
        target_python=target_python,
        source=snapshot.content,
        namespace_premise=namespace_premise,
    )
    # The key and result describe the same captured bytes even if the pathname
    # changes during analysis. A subsequent read admits only its fresh digest.
    try:
        _write_persisted_import_scan(
            project_root,
            path,
            module_name=module_name,
            is_package=path.name == "__init__.py",
            import_scan_mode="full",
            namespace_premise=namespace_premise,
            scan=facts.import_evidence.source_scan,
            snapshot=snapshot,
            target_python=target_python,
        )
        _write_cached_json_object(
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
    batch: tuple[tuple[str, str, PythonNamespacePremise | None], ...],
    target_python: TargetPythonVersion,
) -> None:
    # One transaction captures the tooling fingerprint once for the batch.
    with _source_tree_fingerprint_transaction():
        for module_name, path, namespace_premise in batch:
            _stdlib_intrinsic_source_facts(
                Path(project_root),
                module_name,
                Path(path),
                target_python=target_python,
                namespace_premise=namespace_premise,
            )


def warm_stdlib_intrinsic_source_facts(
    project_root: Path,
    module_paths: Mapping[str, Path],
    *,
    target_python: TargetPythonVersion,
    namespace_premises: Mapping[str, PythonNamespacePremise] | None = None,
) -> None:
    """Fill the per-module facts cache in parallel when the set is large.

    Each module's facts bind its bytes and verified namespace premise, so workers
    may compute them in any order; the caller then classifies from the cache. A
    set too small to amortize worker start-up is left to the caller's serial pass.
    """
    from molt.dx import _memory_bounded_worker_count

    work = [
        (
            name,
            os.fspath(path),
            None if namespace_premises is None else namespace_premises.get(name),
        )
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
    with ProcessPoolExecutor(
        max_workers=workers,
        initializer=_input_report.worker_initializer,
        initargs=(_input_report.worker_log_dir(),),
    ) as pool:
        for _ in pool.map(
            functools.partial(
                _warm_stdlib_intrinsic_source_facts_batch,
                os.fspath(project_root),
                target_python=target_python,
            ),
            batches,
        ):
            pass


def _resolved_module_cache_key(path_str: str, *parts: str) -> str:
    return hashlib.sha256(
        "|".join((_operation_resolved_path(path_str), *parts)).encode("utf-8")
    ).hexdigest()[:24]


# Completed projections from older schemas are never admitted as source requests.
_IMPORT_SCAN_CACHE_SCHEMA_VERSION = 34


def _import_scan_cache_path(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    namespace_premise: PythonNamespacePremise | None = None,
    native_support_function_roots: tuple[str, ...] = (),
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_catalog_owner: bool = False,
) -> Path:
    root = _build_state_subdir_cached(
        os.fspath(_build_state_root(project_root)),
        "import_scan_cache",
    )
    key_parts = [
        module_name,
        "pkg" if is_package else "mod",
        import_scan_mode,
        "runtime-owner" if runtime_catalog_owner else "strict",
        repr(source_owner),
        repr(source_arguments),
        repr(native_support_function_roots),
        repr(_namespace_premise_cache_identity(namespace_premise)),
        target_python.tag,
        _frontend_semantic_tooling_fingerprint(),
    ]
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
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    namespace_premise: PythonNamespacePremise | None = None,
    native_support_function_roots: tuple[str, ...] = (),
    path_stat: os.stat_result | None = None,
    snapshot: _module_source.PythonSourceSnapshot | None = None,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_catalog_owner: bool = False,
) -> _ImportScanRequests | None:
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        namespace_premise=namespace_premise,
        native_support_function_roots=native_support_function_roots,
        target_python=target_python,
        runtime_catalog_owner=runtime_catalog_owner,
    )
    payload = _read_cached_json_object(
        cache_path, max_bytes=SOURCE_ANALYSIS_CACHE_MAX_BYTES
    )
    if payload is None:
        return None
    # Source records are a closed versioned protocol. Retired projections and
    # partial migrations cannot be admitted under a current schema marker.
    if set(payload) != {
        "version",
        "parser",
        "compiler_fingerprint",
        "module_name",
        "is_package",
        "import_scan_mode",
        "source_owner",
        "source_arguments",
        "namespace_premise",
        "native_support_function_roots",
        "source_demands",
        "source_sites",
        "all_exports",
        "source_bindings",
        "source_values",
        "target_python",
        "size",
        "source_sha256",
        "imports",
        "star_modules",
        "dynamic_star_modules",
        "source_executions",
        "dynamic_relative_import_candidates",
        "requires_runtime_package_anchor",
        "namespace_summary",
        "runtime_catalog",
    }:
        return None
    try:
        runtime_catalog = _decode_runtime_catalog_deferral(payload["runtime_catalog"])
    except ValueError:
        return None
    if (runtime_catalog is not None) != runtime_catalog_owner:
        return None
    try:
        namespace_summary = (
            None
            if payload["namespace_summary"] is None
            else _decode_namespace_summary(payload["namespace_summary"])
        )
        _validate_import_scan_namespace_summary(
            namespace_summary, source_owner=source_owner
        )
        cached_premise = _decode_namespace_premise_cache_identity(
            payload["namespace_premise"]
        )
    except (KeyError, ValueError):
        return None
    if (
        payload.get("version") != _IMPORT_SCAN_CACHE_SCHEMA_VERSION
        or payload.get("compiler_fingerprint")
        != _frontend_semantic_tooling_fingerprint()
        or payload.get("module_name") != module_name
        or payload.get("is_package") != is_package
        or payload.get("native_support_function_roots")
        != list(native_support_function_roots)
        or payload.get("target_python") != target_python.tag
        or payload.get("parser") != [sys.implementation.name, sys.version]
        or payload.get("import_scan_mode") != import_scan_mode
        or payload.get("source_owner")
        != (list(source_owner) if source_owner is not None else None)
        or payload.get("source_arguments") != list(source_arguments)
        or cached_premise != namespace_premise
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
    try:

        def rows(field: str, width: int) -> list[list[Any]]:
            return _source_cache_rows(payload[field], width)

        demands = tuple(
            _SourceValueDemand(
                decode_python_source_key(source),
                phase,
            )
            for source, phase in rows("source_demands", 2)
        )
        if any(item.phase not in get_args(SourceDemandPhase) for item in demands):
            return None
        sites = tuple(
            _decode_source_site(item)
            for item in _source_cache_list(payload["source_sites"])
        )
        exports = _decode_source_all(payload["all_exports"])
        bindings = tuple(
            _SourceImportBinding(
                decode_python_source_key(source),
                _source_cache_strings(modules),
                attribute,
                deferred,
            )
            for source, modules, attribute, deferred in rows("source_bindings", 4)
        )
        if any(
            not all(isinstance(name, str) for name in item.modules)
            or item.attribute is not None
            and not isinstance(item.attribute, str)
            or type(item.runtime_catalog) is not bool
            or item.runtime_catalog
            and runtime_catalog is None
            for item in bindings
        ):
            return None
        raw_values = _source_cache_record(
            payload["source_values"],
            {
                "namespaces",
                "attributes",
                "elements",
                "calls",
                "parameters",
                "functions",
                "bases",
                "completions",
            },
        )
        values = _SourceValueSummary(
            tuple(
                (
                    decode_python_source_key(owner),
                    tuple(
                        (name, _source_cache_origins(origins))
                        for name, origins in _source_cache_rows(names, 2)
                    ),
                    _source_cache_origins(exposed),
                )
                for owner, names, exposed in _source_cache_rows(
                    raw_values["namespaces"], 3
                )
            ),
            tuple(
                (
                    decode_python_source_key(owner),
                    name,
                    _source_cache_origins(origins),
                )
                for owner, name, origins in _source_cache_rows(
                    raw_values["attributes"], 3
                )
            ),
            tuple(
                PythonSourceElement(
                    decode_python_source_key(source),
                    PythonElementProtocol(protocol),
                    _source_cache_origins(origins),
                )
                for source, protocol, origins in _source_cache_rows(
                    raw_values["elements"], 3
                )
            ),
            tuple(
                (decode_python_source_key(source), _source_cache_origins(origins))
                for source, origins in _source_cache_rows(raw_values["calls"], 2)
            ),
            tuple(
                (
                    decode_python_source_key(owner),
                    tuple(
                        decode_python_source_key(arg)
                        for arg in _source_cache_list(positional)
                    ),
                    _decode_optional_source_key(vararg),
                )
                for owner, positional, vararg in _source_cache_rows(
                    raw_values["parameters"], 3
                )
            ),
            tuple(
                PythonFunctionResult(
                    decode_python_source_key(owner),
                    FunctionKind(factory),
                    _source_cache_origins(origins),
                    _source_cache_origins(exposed),
                    _source_cache_origins(yielded),
                    _source_cache_origins(yielded_exposed),
                )
                for owner, factory, origins, exposed, yielded, yielded_exposed in _source_cache_rows(
                    raw_values["functions"], 6
                )
            ),
            tuple(
                (decode_python_source_key(owner), _source_cache_origins(origins))
                for owner, origins in _source_cache_rows(raw_values["bases"], 2)
            ),
            tuple(
                (decode_python_source_key(owner), _source_cache_origins(origins))
                for owner, origins in _source_cache_rows(raw_values["completions"], 2)
            ),
        )
        if any(owner[-1] != "ClassDef" for owner, _ in values.bases) or len(
            {owner for owner, _ in values.bases}
        ) != len(values.bases):
            return None
        if len({item.owner for item in values.elements}) != len(values.elements):
            return None
        if any(
            owner[-1] not in {"FunctionDef", "AsyncFunctionDef", "Lambda"}
            or any(arg[-1] != "arg" for arg in positional)
            or vararg is not None
            and vararg[-1] != "arg"
            or len(set((*positional, *((vararg,) if vararg is not None else ()))))
            != len(positional) + (vararg is not None)
            for owner, positional, vararg in values.parameters
        ) or len({owner for owner, _, _ in values.parameters}) != len(
            values.parameters
        ):
            return None
        if (
            any(source[-1] != "Call" for source, _ in values.calls)
            or any(
                source[-1] not in {"Await", "YieldFrom"}
                for source, _ in values.completions
            )
            or any(
                item.owner[-1]
                not in {"FunctionDef", "AsyncFunctionDef", "Lambda", "GeneratorExp"}
                for item in values.functions
            )
            or len({item.owner for item in values.functions}) != len(values.functions)
            or len({owner for owner, _ in values.completions})
            != len(values.completions)
        ):
            return None
        if any(
            not isinstance(name, str)
            for _, names, _ in values.namespaces
            for name, _ in names
        ) or any(not isinstance(name, str) for _, name, _ in values.attributes):
            return None
    except (KeyError, TypeError, ValueError, IndexError):
        return None
    return _ImportScanRequests(
        tuple(payload["imports"]),
        tuple(executions),
        tuple(payload["star_modules"]),
        tuple(payload["dynamic_relative_import_candidates"]),
        requires_anchor,
        tuple(payload["dynamic_star_modules"]),
        demands,
        bindings,
        values,
        sites,
        exports,
        namespace_summary,
        runtime_catalog,
    )


def _encode_runtime_catalog_deferral(
    deferral: RuntimeCatalogDeferral | None,
) -> dict[str, Any] | None:
    if deferral is None:
        return None
    return {
        "source_ast_digest": deferral.source_ast_digest,
        "imports": deferral.imports,
        "star_modules": deferral.star_modules,
    }


def _decode_runtime_catalog_deferral(value: object) -> RuntimeCatalogDeferral | None:
    """A closed record; any other shape makes the scan a miss."""
    if value is None:
        return None
    if (
        not isinstance(value, dict)
        or set(value) != {"source_ast_digest", "imports", "star_modules"}
        or not isinstance(value["source_ast_digest"], str)
        or not value["source_ast_digest"]
        or type(value["imports"]) is not bool
        or type(value["star_modules"]) is not bool
    ):
        raise ValueError("invalid runtime catalog deferral")
    return RuntimeCatalogDeferral(
        value["source_ast_digest"], value["imports"], value["star_modules"]
    )


@_source_tree_fingerprint_transaction()
def _write_persisted_import_scan(
    project_root: Path,
    path: Path,
    *,
    module_name: str,
    is_package: bool,
    import_scan_mode: ImportScanMode,
    source_owner: PythonSourceKey | None = None,
    source_arguments: tuple[str, ...] = (),
    namespace_premise: PythonNamespacePremise | None = None,
    native_support_function_roots: tuple[str, ...] = (),
    scan: _ImportScanRequests,
    snapshot: _module_source.PythonSourceSnapshot,
    target_python: TargetPythonVersion = _DEFAULT_TARGET_PYTHON_VERSION,
    runtime_catalog_owner: bool = False,
) -> None:
    _validate_import_scan_namespace_summary(
        scan.namespace_summary, source_owner=source_owner
    )
    if (scan.runtime_catalog is not None) != runtime_catalog_owner or (
        scan.runtime_catalog is None
        and any(binding.runtime_catalog for binding in scan.source_bindings)
    ):
        raise ValueError("a runtime catalog deferral belongs only to an owner scan")
    if snapshot.path != path:
        raise ValueError("source scan snapshot path mismatch")
    identity = {"size": len(snapshot.content), "source_sha256": snapshot.sha256}
    # A publication is for the captured generation, never a later pathname hash.
    if not _module_source._payload_source_matches(identity, path, path.stat()):
        return
    payload = {
        "version": _IMPORT_SCAN_CACHE_SCHEMA_VERSION,
        "parser": [sys.implementation.name, sys.version],
        "compiler_fingerprint": _frontend_semantic_tooling_fingerprint(),
        "module_name": module_name,
        "is_package": is_package,
        "native_support_function_roots": list(native_support_function_roots),
        "import_scan_mode": import_scan_mode,
        "source_owner": source_owner,
        "source_arguments": source_arguments,
        "namespace_premise": _namespace_premise_cache_identity(namespace_premise),
        "source_demands": [
            [
                item.source,
                item.phase,
            ]
            for item in scan.source_demands
        ],
        "source_sites": [_encode_source_site(site) for site in scan.source_sites],
        "all_exports": asdict(scan.all_exports),
        "namespace_summary": (
            None
            if scan.namespace_summary is None
            else _encode_namespace_summary(scan.namespace_summary)
        ),
        "source_bindings": scan.source_bindings,
        "source_values": {
            "namespaces": [
                (
                    owner,
                    [(name, sorted(origins)) for name, origins in names],
                    sorted(exposed),
                )
                for owner, names, exposed in scan.source_values.namespaces
            ],
            "attributes": [
                (owner, name, sorted(origins))
                for owner, name, origins in scan.source_values.attributes
            ],
            "elements": [
                (item.owner, item.protocol, sorted(item.origins))
                for item in scan.source_values.elements
            ],
            "calls": [
                (source, sorted(origins))
                for source, origins in scan.source_values.calls
            ],
            "parameters": scan.source_values.parameters,
            "functions": [
                (
                    item.owner,
                    item.kind,
                    sorted(item.returned),
                    sorted(item.returned_exposed),
                    sorted(item.yielded),
                    sorted(item.yielded_exposed),
                )
                for item in scan.source_values.functions
            ],
            "bases": [
                (owner, sorted(origins)) for owner, origins in scan.source_values.bases
            ],
            "completions": [
                (owner, sorted(origins))
                for owner, origins in scan.source_values.completions
            ],
        },
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
        "runtime_catalog": _encode_runtime_catalog_deferral(scan.runtime_catalog),
    }
    cache_path = _import_scan_cache_path(
        project_root,
        path,
        module_name=module_name,
        is_package=is_package,
        import_scan_mode=import_scan_mode,
        source_owner=source_owner,
        source_arguments=source_arguments,
        namespace_premise=namespace_premise,
        native_support_function_roots=native_support_function_roots,
        target_python=target_python,
        runtime_catalog_owner=runtime_catalog_owner,
    )
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    _write_cached_json_object(cache_path, payload)

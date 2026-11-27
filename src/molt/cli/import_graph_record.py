"""Content-verified replay of a source-backed, settled import graph."""

from __future__ import annotations

import hashlib
import os
import sys
from collections.abc import Hashable
from dataclasses import asdict, dataclass, replace
from pathlib import Path
from types import MappingProxyType
from typing import Mapping, Sequence, TypeVar, cast

from molt.compilation_target import Target, target_name
from molt.cli import module_graph_cache as codec
from molt.cli.cache_fingerprints import (
    _frontend_semantic_tooling_fingerprint,
)
from molt.cli.frontend_record_files import (
    frontend_record_files,
    is_record_digest,
    record_digest,
)
from molt.exact_json import decode_frontend_payload, encode_frontend_payload
from molt.cli.models import (
    ImportScanEvidence,
    PersistedImportScanObservation,
    ImportScanQuery,
    ModuleReason,
    RuntimeImportMachinery,
    _CompleteImportScan,
    _ImportAdmissionPolicy,
    _ImportPlan,
    _ModuleGraphScanAuthority,
    _ModuleSourceScanAuthority,
    _ResolvedBuildEntry,
    _RuntimeImportScanCustody,
    _RuntimeImportSupportPolicy,
    _SettledImportGraph,
)
from molt.cli.module_resolution import _ModuleResolutionCache
from molt.cli.module_source import PythonSourceSnapshot, PythonSourceChangedError
from molt.cli.python_import_topology import PythonImportTopology
from molt.compiler_analysis.native_support_slice import ModuleBodySelection
from molt.compiler_analysis.python_import_scan import SourceImportPurpose
from molt.compiler_analysis.python_imports import ImportScanMode, _ImportScanRequests
from molt.compiler_analysis.python_namespace_graph import (
    PythonNamespacePremise,
    python_namespace_name_universe,
)
from molt.compiler_analysis.python_source_keys import (
    PythonSourceKey,
    decode_python_source_key,
)
from molt.compiler_analysis.python_import_redirects import (
    ImportRedirects,
    ImportWriter,
)
from molt.compiler_analysis.python_string_facts import (
    PythonStringBound,
)
from molt.target_python import TargetPythonVersion

_MAX_BYTES = 16 * 1024 * 1024
_SCHEMA = 6
_KIND = "import-graph"
_T = TypeVar("_T")


def _map(value: object) -> Mapping[str, object]:
    if not isinstance(value, dict) or any(type(key) is not str for key in value):
        raise ValueError("invalid graph record object")
    return cast(Mapping[str, object], value)


def _list(value: object) -> list[object]:
    if not isinstance(value, list):
        raise ValueError("invalid graph record rows")
    return cast(list[object], value)


def _text(value: object) -> str:
    if not isinstance(value, str):
        raise ValueError("invalid graph record string")
    return value


def _texts(value: object) -> tuple[str, ...]:
    return tuple(_text(item) for item in _list(value))


def _bool(value: object) -> bool:
    if type(value) is not bool:
        raise ValueError("invalid graph record boolean")
    return value


def _int(value: object) -> int:
    if type(value) is not int:
        raise ValueError("invalid graph record integer")
    return value


def _table_row(rows: Sequence[_T], index: object) -> _T:
    position = _int(index)
    if not 0 <= position < len(rows):
        raise ValueError("graph record table index is out of bounds")
    return rows[position]


def _bytes(value: object) -> bytes:
    return encode_frontend_payload(value, sort_keys=True, allow_nan=False)


class _RecordingTable[T: Hashable]:
    def __init__(self) -> None:
        self.rows: list[T] = []
        self._indices: dict[T, int] = {}

    def index(self, value: T) -> int:
        index = self._indices.get(value)
        if index is None:
            index = len(self.rows)
            self._indices[value] = index
            self.rows.append(value)
        return index


@dataclass(frozen=True, slots=True)
class _RecordedQuerySource:
    path: Path
    sha256: str
    size: int
    module: str
    package: bool
    selection: ModuleBodySelection

    def record(self) -> tuple[object, ...]:
        return (
            str(self.path),
            self.sha256,
            self.size,
            self.module,
            self.package,
            self.selection.payload(),
        )

    @classmethod
    def decode(cls, raw: object) -> _RecordedQuerySource:
        path, sha, size, module, package, selection = _list(raw)
        sha256 = _text(sha)
        source_size = _int(size)
        if not is_record_digest(sha256) or source_size < 0:
            raise ValueError("invalid graph record source content identity")
        return cls(
            Path(_text(path)),
            sha256,
            source_size,
            _text(module),
            _bool(package),
            ModuleBodySelection.from_payload(selection),
        )


def _custody(value: _RuntimeImportScanCustody | None) -> object:
    return (
        None
        if value is None
        else {
            "owners": [[name, str(path)] for name, path in value.owners],
            "catalog": [[name, str(path)] for name, path in value.catalog],
            "digests": value.owner_ast_digests,
        }
    )


def _read_custody(raw: object) -> _RuntimeImportScanCustody | None:
    if raw is None:
        return None
    value = _map(raw)

    def paths(rows: object) -> tuple[tuple[str, Path], ...]:
        return tuple(
            (_text(_list(row)[0]), Path(_text(_list(row)[1]))) for row in _list(rows)
        )

    return _RuntimeImportScanCustody(
        paths(value["owners"]),
        paths(value["catalog"]),
        tuple(
            (_text(_list(row)[0]), _text(_list(row)[1]))
            for row in _list(value["digests"])
        ),
    )


@dataclass(frozen=True, slots=True)
class _QueryContextKey:
    # The wire keeps path spellings even where Path compares case-insensitively.
    roots: tuple[str, ...] | None
    stdlib: str | None
    allowlist: frozenset[str] | None
    aliases: tuple[tuple[str, str], ...]
    custody: bytes | None


@dataclass(frozen=True, slots=True)
class _RecordedQueryContext:
    roots: tuple[Path, ...] | None
    stdlib: Path | None
    allowlist: frozenset[str] | None
    custody: _RuntimeImportScanCustody | None

    @classmethod
    def decode(
        cls, raw: object, expected_aliases: tuple[tuple[str, Path], ...]
    ) -> _RecordedQueryContext:
        row = _map(raw)
        if set(row) != {"roots", "stdlib", "allowlist", "aliases", "custody"}:
            raise ValueError("invalid graph query context")
        aliases = tuple(
            (_text(_list(item)[0]), Path(_text(_list(item)[1])))
            for item in _list(row["aliases"])
        )
        if aliases != expected_aliases:
            raise ValueError("graph query aliases changed")
        return cls(
            None
            if row["roots"] is None
            else tuple(Path(path) for path in _texts(row["roots"])),
            None if row["stdlib"] is None else Path(_text(row["stdlib"])),
            None if row["allowlist"] is None else frozenset(_texts(row["allowlist"])),
            _read_custody(row["custody"]),
        )


def _redirects(raw: object) -> ImportRedirects:
    value = _map(raw)

    def writer(item: object) -> ImportWriter:
        row = _map(item)
        return ImportWriter(_text(row["site"]), _text(row["construct"]))

    def bound(item: object) -> PythonStringBound:
        row = _map(item)
        return PythonStringBound(
            _text(row["prefix"]), _text(row["suffix"]), _int(row["minimum"])
        )

    def key(item: object) -> str | PythonStringBound | None:
        return None if item is None else item if isinstance(item, str) else bound(item)

    return ImportRedirects(
        global_writers=tuple(writer(row) for row in _list(value["global_writers"])),
        key_writers=tuple(
            (_text(_list(row)[0]), writer(_list(row)[1]))
            for row in _list(value["key_writers"])
        ),
        bound_writers=tuple(
            (bound(_list(row)[0]), writer(_list(row)[1]))
            for row in _list(value["bound_writers"])
        ),
        guarded=tuple(
            (key(_list(row)[0]), writer(_list(row)[1]), _texts(_list(row)[2]))
            for row in _list(value["guarded"])
        ),
        spec_writers=tuple(
            (
                None if _list(row)[0] is None else _text(_list(row)[0]),
                writer(_list(row)[1]),
            )
            for row in _list(value["spec_writers"])
        ),
        spec_member_stores=tuple(
            writer(row) for row in _list(value["spec_member_stores"])
        ),
        escaped=None if value["escaped"] is None else _texts(value["escaped"]),
        import_hook_writers=tuple(
            writer(row) for row in _list(value["import_hook_writers"])
        ),
    )


@dataclass(frozen=True)
class ImportGraphContext:
    entry: _ResolvedBuildEntry
    admission: _ImportAdmissionPolicy
    target: Target
    target_python: TargetPythonVersion
    target_sys_platform: str | None
    stdlib_root: Path
    stdlib_profile: str
    include_modules: tuple[str, ...]
    runtime_machinery: RuntimeImportMachinery
    stdlib_allowlist: frozenset[str]
    normal_none_intrinsics: int

    def payload(self) -> dict[str, object]:
        assert self.entry.image_scope is not None
        cpython_suite = os.environ.get("MOLT_REGRTEST_CPYTHON_DIR")
        return {
            "host_python": sys.version,
            "cpython_suite": str(Path(cpython_suite).resolve())
            if cpython_suite
            else None,
            "aliases": [
                (name, str(path)) for name, path in self.entry.module_root_aliases
            ],
            "stdlib_allowlist": sorted(self.stdlib_allowlist),
            "normal_none_intrinsics": self.normal_none_intrinsics,
            "source": str(self.entry.source_path),
            "entry": self.entry.entry_module,
            "roots": [str(path) for path in self.entry.module_roots],
            "stdlib": str(self.stdlib_root),
            "profile": self.stdlib_profile,
            "includes": self.include_modules,
            "target": target_name(self.target),
            "python": asdict(self.target_python),
            "platform": self.target_sys_platform,
            "machinery": self.runtime_machinery.value,
            "image": self.entry.image_scope.diagnostic_payload(),
            "admission": self.admission.digest_payload(),
        }


@dataclass(frozen=True, slots=True)
class _SourceRequestCoordinate:
    module_name: str
    is_package: bool
    mode: ImportScanMode
    owner: PythonSourceKey | None
    arguments: tuple[str, ...]
    body_roots: tuple[str, ...]
    target_python: TargetPythonVersion
    target_sys_platform: str | None
    purpose: SourceImportPurpose


class ImportResultDigests:
    """Reuse the source projection across one publication or replay operation."""

    def __init__(self) -> None:
        # Retain each request object for the lifetime of its identity key.
        self._sources: dict[
            tuple[int, _SourceRequestCoordinate], tuple[_ImportScanRequests, str]
        ] = {}

    def digest(self, query: ImportScanQuery, result: _CompleteImportScan) -> str:
        """Digest every graph-facing output independently of the input generation."""
        lease = query.source_lease
        assert lease is not None
        coordinate = _SourceRequestCoordinate(
            query.module_name,
            query.is_package,
            query.mode,
            query.source_owner,
            query.source_arguments,
            query.body_selection.roots,
            query.target_python,
            query.target_sys_platform,
            query.purpose,
        )
        requests = result.source_requests
        key = (id(requests), coordinate)
        source = self._sources.get(key)
        if source is None:
            payload = codec._import_scan_payload(
                module_name=coordinate.module_name,
                is_package=coordinate.is_package,
                import_scan_mode=coordinate.mode,
                source_owner=coordinate.owner,
                source_arguments=coordinate.arguments,
                native_support_function_roots=coordinate.body_roots,
                scan=requests,
                source_sha256=lease.source_sha256,
                source_size=lease.source_size,
                target_python=coordinate.target_python,
                target_sys_platform=coordinate.target_sys_platform,
                runtime_catalog_owner=requests.runtime_catalog is not None,
                purpose=coordinate.purpose,
            )
            payload.pop("source_sha256")
            payload.pop("size")
            source = (requests, hashlib.sha256(_bytes(payload)).hexdigest())
            self._sources[key] = source
        resolved = {
            "source_digest": source[1],
            "imports": result.imports,
            "executions": [
                [name, str(path)] for name, path in result.source_executions
            ],
            "candidates": result.dynamic_relative_import_candidates,
            "anchor": result.requires_runtime_package_anchor,
            "sites": [codec._encode_source_site(site) for site in result.source_sites],
            "exports": [
                [name, asdict(exports)] for name, exports in result.star_exports
            ],
        }
        return hashlib.sha256(_bytes(resolved)).hexdigest()


def _complete_observers(
    authority: _ModuleGraphScanAuthority,
    observations: Sequence[ImportScanEvidence],
) -> bool:
    """Every admitted module and demanded body needs its actual query witness."""
    by_module: dict[str, list[ImportScanQuery]] = {}
    for row in observations:
        by_module.setdefault(row.query.module_name, []).append(row.query)
    for source in authority.sources:
        queries = by_module.get(source.module_name, [])
        if source.source_sha256 is None or not any(
            query.source_owner is None for query in queries
        ):
            return False
        full_owner = any(
            query.mode == "full"
            and query.runtime_custody is not None
            and query.runtime_custody.owns(source.module_name, source.source_path)
            for query in queries
        )
        owners = {(query.source_owner, query.source_arguments) for query in queries}
        if not full_owner and not source.deferred_owners <= owners:
            return False
    return True


def publish_import_graph(plan: _ImportPlan, context: ImportGraphContext) -> None:
    """Cache publication failure never changes a successfully compiled program."""
    try:
        _publish_import_graph(plan, context)
    except PythonSourceChangedError:
        raise
    except (OSError, ValueError, TypeError, RecursionError):
        return


def _publish_import_graph(plan: _ImportPlan, context: ImportGraphContext) -> None:
    if plan.native_artifact_plan.artifacts:
        return
    graph = plan.settled_graph
    if not _complete_observers(
        graph.scan_authority, plan.import_query_closure.observations
    ):
        return
    rows: list[object] = []
    sources = _RecordingTable[_RecordedQuerySource]()
    contexts: list[object] = []
    context_ids: dict[_QueryContextKey, int] = {}
    # Observations keep each immutable custody object alive for this publication.
    custodies: dict[int, tuple[bytes, object]] = {}
    premises = _RecordingTable[PythonNamespacePremise | None]()
    result_digests = _RecordingTable[str]()
    digests = ImportResultDigests()
    for observation in plan.import_query_closure.observations:
        query = observation.query
        lease = query.source_lease
        if (
            lease is None
            or not lease.path_backed_source
            or query.supplied_ast_digest is not None
        ):
            return
        custody_payload = None
        custody_key = None
        if query.runtime_custody is not None:
            owner = id(query.runtime_custody)
            recorded = custodies.get(owner)
            if recorded is None:
                custody_payload = _custody(query.runtime_custody)
                recorded = (_bytes(custody_payload), custody_payload)
                custodies[owner] = recorded
            custody_key, custody_payload = recorded
        identity = _QueryContextKey(
            None if query.roots is None else tuple(map(str, query.roots)),
            None if query.stdlib_root is None else str(query.stdlib_root),
            query.stdlib_allowlist,
            tuple((name, str(path)) for name, path in query.module_root_aliases),
            custody_key,
        )
        index = context_ids.get(identity)
        if index is None:
            index = len(contexts)
            context_ids[identity] = index
            contexts.append(
                {
                    "roots": identity.roots,
                    "stdlib": identity.stdlib,
                    "allowlist": None
                    if identity.allowlist is None
                    else sorted(identity.allowlist),
                    "aliases": identity.aliases,
                    "custody": custody_payload,
                }
            )
        premise_index = premises.index(query.namespace_premise)
        source = _RecordedQuerySource(
            query.source_path,
            lease.source_sha256,
            lease.source_size,
            query.module_name,
            query.is_package,
            query.body_selection,
        )
        source_index = sources.index(source)
        rows.append(
            (
                source_index,
                query.mode,
                query.purpose.value,
                query.source_owner,
                query.source_arguments,
                premise_index,
                index,
                result_digests.index(
                    observation.result_digest
                    if isinstance(observation, PersistedImportScanObservation)
                    else digests.digest(query, observation.result)
                ),
            )
        )
    authority = graph.scan_authority.payload()
    for row, source in zip(
        authority,
        sorted(graph.scan_authority.sources, key=lambda item: item.module_name),
    ):
        row["namespace_premise"] = premises.index(source.namespace_premise)
    # A program can carry the same escape-writer relation in every module
    # and in many of the narrower premises under which queries were observed.
    escape_writes = _RecordingTable[tuple[tuple[str, str], ...]]()
    premise_rows = []
    for premise in premises.rows:
        row = codec._namespace_premise_cache_identity(premise)
        if premise is not None and row is not None:
            row["escape_writes"] = escape_writes.index(premise.escape_writes)
        premise_rows.append(row)
    coordinate = context.payload()
    payload = {
        "schema": _SCHEMA,
        "producer": _frontend_semantic_tooling_fingerprint(),
        "context": coordinate,
        "contexts": contexts,
        "premises": premise_rows,
        "escape_writes": escape_writes.rows,
        "result_digests": result_digests.rows,
        "query_sources": [source.record() for source in sources.rows],
        "queries": rows,
        "topology": plan.import_query_closure.import_topology.record(),
        "graph": [[name, str(path)] for name, path in graph.module_graph.items()],
        "authority": authority,
        "authority_order": [
            source.module_name for source in graph.scan_authority.sources
        ],
        "reasons": {
            name: sorted(reason.value for reason in reasons)
            for name, reasons in graph.module_reasons.items()
        },
        "explicit": sorted(graph.explicit_imports),
        "support": sorted(graph.support_explicit_imports),
        "dispatch": sorted(graph.runtime_import_dispatch_roots),
        "stubs": sorted(graph.stub_parents),
        "spawn": graph.spawn_enabled,
        "runtime": graph.runtime_import_support_policy.needs_runtime_import_support,
        "custody": _custody(graph.runtime_import_scan_custody),
        "redirects": asdict(graph.namespace_import_redirects),
        "declared": sorted(graph.declared_root_modules),
        "allowlist": sorted(graph.stdlib_allowlist),
    }
    _store_import_graph(record_digest(_bytes(coordinate)), plan, payload)


def replay_import_graph(context: ImportGraphContext) -> _ImportPlan | None:
    try:
        return _replay(context)
    except (
        OSError,
        ValueError,
        TypeError,
        KeyError,
        IndexError,
        SyntaxError,
        RecursionError,
    ):
        # A cache candidate grants no source or diagnostic authority on refusal.
        return None


def _store_import_graph(
    key: str, plan: _ImportPlan, payload: Mapping[str, object]
) -> None:
    encoded = _bytes(payload)
    if len(encoded) <= _MAX_BYTES:
        plan.import_query_closure.validate(plan.scan_authority)
        frontend_record_files().put(_KIND, key, encoded)


def _replay(context: ImportGraphContext) -> _ImportPlan | None:
    from molt.cli.module_graph import _assemble_import_plan
    from molt.cli.module_graph_discovery import _load_module_import_scan
    from molt.cli.external_native import _resolve_external_package_native_artifact_plan

    coordinate = _bytes(context.payload())
    key = record_digest(coordinate)
    record = frontend_record_files().get(_KIND, key, max_bytes=_MAX_BYTES)
    if record is None:
        return None
    value = _map(decode_frontend_payload(record.payload))
    if set(value) != {
        "schema",
        "producer",
        "context",
        "contexts",
        "premises",
        "escape_writes",
        "result_digests",
        "query_sources",
        "queries",
        "topology",
        "graph",
        "authority",
        "authority_order",
        "reasons",
        "explicit",
        "support",
        "dispatch",
        "stubs",
        "spawn",
        "runtime",
        "custody",
        "redirects",
        "declared",
        "allowlist",
    }:
        return None
    if (
        value.get("schema") != _SCHEMA
        or value.get("producer") != _frontend_semantic_tooling_fingerprint()
        or _bytes(value["context"]) != coordinate
    ):
        return None
    graph = {
        _text(_list(row)[0]): Path(_text(_list(row)[1]))
        for row in _list(value["graph"])
    }
    # Externally selected native objects remain owned by their current admission.
    native, errors = _resolve_external_package_native_artifact_plan(
        external_module_roots=context.admission.external_roots,
        admitted_packages=context.admission.admitted_external_packages,
        target=context.target,
        target_python=context.target_python,
        required_modules=set(graph)
        | set(_texts(value["dispatch"]))
        | set(_texts(value["explicit"])),
    )
    if errors or native is None or native.artifacts:
        return None
    context.entry.import_topology.validate()
    topology = PythonImportTopology.from_record(value["topology"])
    resolver = _ModuleResolutionCache(
        target_sys_platform=context.target_sys_platform,
        import_topology=topology,
        module_root_aliases=context.entry.module_root_aliases,
        normal_none_intrinsics=context.normal_none_intrinsics,
    )
    snapshots: dict[Path, PythonSourceSnapshot] = {}
    query_contexts = tuple(
        _RecordedQueryContext.decode(raw, context.entry.module_root_aliases)
        for raw in _list(value["contexts"])
    )
    escape_writes = _list(value["escape_writes"])
    premises: list[PythonNamespacePremise | None] = []
    for raw in _list(value["premises"]):
        if raw is None:
            premises.append(None)
            continue
        premise = _map(raw)
        premises.append(
            codec._decode_namespace_premise_cache_identity(
                {
                    **premise,
                    "escape_writes": _table_row(
                        escape_writes, premise["escape_writes"]
                    ),
                }
            )
        )
    result_digests = _texts(value["result_digests"])
    if not all(map(is_record_digest, result_digests)):
        return None
    name_universes: dict[Path, frozenset[str] | None] = {}
    updated_sources = _list(value["query_sources"])
    query_sources = tuple(_RecordedQuerySource.decode(raw) for raw in updated_sources)
    changed_sources: set[Path] = set()
    changed_source_rows: dict[int, PythonSourceSnapshot] = {}
    digests = ImportResultDigests()
    for raw in _list(value["queries"]):
        (
            source_id,
            raw_mode,
            raw_purpose,
            raw_owner,
            arguments,
            premise_id,
            context_id,
            digest,
        ) = _list(raw)
        source = _table_row(query_sources, source_id)
        path = source.path
        snapshot = snapshots.get(path)
        if snapshot is None:
            snapshot = PythonSourceSnapshot.capture(path)
            snapshots[path] = snapshot
        common = _table_row(query_contexts, context_id)
        premise = _table_row(premises, premise_id)
        changed = snapshot.sha256 != source.sha256
        if not changed and len(snapshot.content) != source.size:
            return None
        if changed and premise is not None and premise.foreign_universe is not None:
            if path not in name_universes:
                name_universes[path] = python_namespace_name_universe(snapshot.text)
            if name_universes[path] != premise.foreign_universe:
                return None
        owner = None if raw_owner is None else decode_python_source_key(raw_owner)
        mode = cast(ImportScanMode, _text(raw_mode))
        if mode not in ("full", "module_init"):
            raise ValueError("invalid graph scan mode")
        purpose = SourceImportPurpose(_text(raw_purpose))
        result_digest = _table_row(result_digests, digest)
        query = ImportScanQuery(
            path,
            resolver.import_query_source_lease(snapshot),
            None,
            source.module,
            source.package,
            mode,
            purpose,
            owner,
            _texts(arguments),
            context.target_python,
            context.target_sys_platform,
            premise,
            source.selection,
            common.roots,
            common.stdlib,
            common.allowlist,
            context.entry.module_root_aliases,
            common.custody,
        )
        if changed:
            loaded = _load_module_import_scan(
                path,
                module_name=query.module_name,
                is_package=query.is_package,
                import_scan_mode=mode,
                purpose=purpose,
                source_owner=owner,
                source_arguments=query.source_arguments,
                resolution_cache=resolver,
                project_root=context.entry.source_path.parent,
                source_snapshot=snapshot,
                roots=common.roots,
                stdlib_root=common.stdlib,
                stdlib_allowlist=None
                if common.allowlist is None
                else set(common.allowlist),
                target_python=context.target_python,
                runtime_import_custody=common.custody,
                namespace_premise=None
                if premise is None
                else replace(premise, foreign_universe=None),
                native_support_function_roots=source.selection.roots,
            )
            if digests.digest(query, loaded.scan) != result_digest:
                return None
            changed_sources.add(path)
            changed_source_rows[_int(source_id)] = snapshot
        else:
            resolver.observe_persisted_import_scan(query, result_digest)
    sources: list[_ModuleSourceScanAuthority] = []
    updated_authority = _list(value["authority"])
    for row_index, raw in enumerate(updated_authority):
        row = _map(raw)
        name = _text(row["module"])
        path = Path(_text(row["path"]))
        snapshot = snapshots.get(path)
        if snapshot is None or row["source_sha256"] is None:
            return None
        if path in changed_sources:
            updated_authority[row_index] = {**row, "source_sha256": snapshot.sha256}
        sources.append(
            _ModuleSourceScanAuthority(
                name,
                path,
                cast(ImportScanMode, _text(row["mode"])),
                body_selection=ModuleBodySelection.from_payload(row["body_selection"]),
                is_package=_bool(row["is_package"]),
                requires_runtime_package_anchor=_bool(
                    row["requires_runtime_package_anchor"]
                ),
                uses_runtime_import_protocol=_bool(row["uses_runtime_import_protocol"]),
                deferred_owners=frozenset(
                    (
                        decode_python_source_key(_map(item)["source"]),
                        _texts(_map(item)["arguments"]),
                    )
                    for item in _list(row["deferred_bodies"])
                ),
                read_callables=frozenset(
                    decode_python_source_key(item)
                    for item in _list(row["read_callables"])
                ),
                namespace_premise=_table_row(premises, row["namespace_premise"]),
                namespace_own_names=None
                if row["namespace_own_names"] is None
                else frozenset(_texts(row["namespace_own_names"])),
                namespace_own_line=None
                if row["namespace_own_line"] is None
                else _int(row["namespace_own_line"]),
                source_sha256=snapshot.sha256,
                package_paths=tuple(
                    Path(item) for item in _texts(row["package_paths"])
                ),
            )
        )
    by_module = {source.module_name: source for source in sources}
    authority = _ModuleGraphScanAuthority(
        tuple(by_module[name] for name in _texts(value["authority_order"]))
    )
    if not _complete_observers(authority, resolver.import_query_closure().observations):
        return None
    assert context.entry.image_scope is not None
    settled = _SettledImportGraph(
        resolver.import_query_closure(),
        context.entry.image_scope.with_root_modules(
            (context.entry.entry_module, *context.include_modules)
        ),
        frozenset(_texts(value["declared"])),
        tuple([*context.entry.module_roots, context.stdlib_root]),
        frozenset(_texts(value["allowlist"])),
        MappingProxyType(graph),
        authority,
        frozenset(_texts(value["explicit"])),
        frozenset(_texts(value["support"])),
        frozenset(_texts(value["dispatch"])),
        frozenset(_texts(value["stubs"])),
        _bool(value["spawn"]),
        _RuntimeImportSupportPolicy(_bool(value["runtime"])),
        _read_custody(value["custody"]),
        context.runtime_machinery,
        native,
        _redirects(value["redirects"]),
        MappingProxyType(
            {
                name: frozenset(ModuleReason(reason) for reason in _texts(reasons))
                for name, reasons in _map(value["reasons"]).items()
            }
        ),
        MappingProxyType({"import_graph_replay": 1}),
        context.target,
        context.target_python,
    )
    plan = _assemble_import_plan(
        settled,
        resolver=resolver,
        stdlib_root=context.stdlib_root,
        entry_module=context.entry.entry_module,
    )
    if changed_sources:
        # Every query that used the old content has matched. Advance the
        # shared source rows only after verifying all their observations.
        for source_index, snapshot in changed_source_rows.items():
            updated_sources[source_index] = replace(
                query_sources[source_index],
                sha256=snapshot.sha256,
                size=len(snapshot.content),
            ).record()
        try:
            _store_import_graph(
                key,
                plan,
                {
                    **value,
                    "query_sources": updated_sources,
                    "authority": updated_authority,
                    "topology": plan.import_query_closure.import_topology.record(),
                },
            )
        except OSError:
            # The admitted plan remains usable when a cache write fails.
            pass
    else:
        record.refresh()
    return plan

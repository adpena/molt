"""Content-verified replay of a source-backed, settled import graph."""

from __future__ import annotations

import hashlib
import os
import sys
from collections.abc import Hashable
from contextlib import AbstractContextManager, nullcontext
from dataclasses import asdict, dataclass, field, replace
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
from molt.cli.module_resolution import (
    _ImportGraphStage,
    _ImportScanDiagnostics,
    _ModuleResolutionCache,
)
from molt.cli.module_source import PythonSourceSnapshot, PythonSourceChangedError
from molt.cli.python_import_topology import PythonImportTopology
from molt.compiler_analysis.native_support_slice import ModuleBodySelection
from molt.compiler_analysis.python_import_scan import SourceImportPurpose
from molt.compiler_analysis.python_imports import ImportScanMode, _ImportScanRequests
from molt.compiler_analysis.python_namespace_graph import (
    PythonModulePublications,
    PythonNamespacePremise,
    python_module_publications,
    python_namespace_name_universe,
)
from molt.compiler_analysis.python_source_keys import (
    PythonAstLineIdentity,
    PythonSourceKey,
    PythonSourceLocations,
    decode_python_source_key,
    python_ast_line_identity,
)
from molt.compiler_analysis.python_namespace_writes import python_package_name
from molt.compiler_analysis.python_import_redirects import (
    ImportRedirects,
    ImportWriter,
    import_redirects,
)
from molt.compiler_analysis.python_string_facts import (
    PythonStringBound,
    SourceStringArguments,
    decode_source_string_arguments,
)
from molt.target_python import TargetPythonVersion, _parse_source_for_target

_MAX_BYTES = 16 * 1024 * 1024
_SCHEMA = 10
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


def _publication_digest(
    facts: PythonModulePublications, locations: PythonSourceLocations
) -> str:
    # Qualified argument transfer reads these projections independently of the
    # source scan. Both the caller and provider can change the answer.
    return hashlib.sha256(
        _bytes(
            (
                dict(facts.leading_imports),
                {name: locations.key(key) for name, key in facts.functions.items()},
                [
                    (locations.key(key), pair)
                    for key, pair in sorted(facts.qualified_calls.items())
                ],
                facts.module_getattr,
            )
        )
    ).hexdigest()


def _source_line_identity(
    snapshot: PythonSourceSnapshot, target_python: TargetPythonVersion
) -> PythonAstLineIdentity | None:
    try:
        # Use the admitted source parser before canonical rewrites attach
        # metadata outside the raw AST's fields.
        tree = _parse_source_for_target(
            snapshot.text, filename=str(snapshot.path), target_python=target_python
        )
        return python_ast_line_identity(tree)
    except (SyntaxError, ValueError, RecursionError):
        return None


class _ReplayLocations:
    """Reattach recorded source positions through equal AST line slots."""

    def __init__(self) -> None:
        self.by_module: dict[str, PythonSourceLocations] = {}

    def line(self, module: str, old: int) -> int:
        locations = self.by_module.get(module)
        return old if locations is None else locations.line(old)

    def key(self, module: str, key: PythonSourceKey) -> PythonSourceKey:
        locations = self.by_module.get(module)
        return key if locations is None else locations.key(key)

    def site(self, site: str) -> str:
        module, _, line = site.rpartition(":")
        if module not in self.by_module or not line.isdecimal():
            return site
        return f"{module}:{self.line(module, int(line))}"

    def premise(
        self, module: str, premise: PythonNamespacePremise | None
    ) -> PythonNamespacePremise | None:
        if premise is None or module not in self.by_module:
            return premise
        opaque = premise.opaque_activation_sources
        return replace(
            premise,
            opaque_activation_sources=None
            if opaque is None
            else tuple(self.key(module, key) for key in opaque),
            exact_receivers=tuple(
                (self.key(module, method), self.key(module, receiver))
                for method, receiver in premise.exact_receivers
            ),
        )


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


def _redirects(raw: object, locations: _ReplayLocations) -> ImportRedirects:
    value = _map(raw)

    def writer(item: object) -> ImportWriter:
        row = _map(item)
        return ImportWriter(locations.site(_text(row["site"])), _text(row["construct"]))

    def bound(item: object) -> PythonStringBound:
        row = _map(item)
        return PythonStringBound(
            _text(row["prefix"]), _text(row["suffix"]), _int(row["minimum"])
        )

    def key(item: object) -> str | PythonStringBound | None:
        return None if item is None else item if isinstance(item, str) else bound(item)

    return import_redirects(
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
    scan_diagnostics: _ImportScanDiagnostics | None = field(
        default=None, compare=False, repr=False
    )

    def measure(self, stage: _ImportGraphStage) -> AbstractContextManager[None]:
        diagnostics = self.scan_diagnostics
        return nullcontext() if diagnostics is None else diagnostics.graph_stage(stage)

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
    arguments: SourceStringArguments
    body_roots: tuple[str, ...]
    target_python: TargetPythonVersion
    target_sys_platform: str | None
    purpose: SourceImportPurpose


class ImportResultDigests:
    """Reuse the source projection across one publication or replay operation."""

    def __init__(self) -> None:
        # Retain each request object for the lifetime of its identity key.
        self._sources: dict[
            tuple[int, _SourceRequestCoordinate, PythonSourceLocations],
            tuple[_ImportScanRequests, str],
        ] = {}

    def digest(
        self,
        query: ImportScanQuery,
        result: _CompleteImportScan,
        locations: PythonSourceLocations = PythonSourceLocations(),
    ) -> str:
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
        if (
            locations.slots is not None
            and (catalog := requests.runtime_catalog) is not None
        ):
            # The query holds absolute owner identity separately from the
            # location-independent result and its catalog deferrals.
            custody = query.runtime_custody
            if custody is None or not custody.admits_scan(
                query.module_name, query.source_path, catalog.source_ast_digest
            ):
                raise ValueError("import result lost runtime catalog custody")
        key = (id(requests), coordinate, locations)
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
                locations=locations,
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
            "sites": [
                codec._encode_source_site(site, locations)
                for site in result.source_sites
            ],
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
        with context.measure("publication"):
            _publish_import_graph(plan, context)
    except PythonSourceChangedError:
        raise
    except (OSError, ValueError, TypeError, RecursionError):
        return


def _publish_import_graph(
    plan: _ImportPlan,
    context: ImportGraphContext,
    source_lines: dict[str, PythonAstLineIdentity | None] | None = None,
    *,
    retained_publications: Sequence[list[str]] = (),
) -> None:
    if plan.native_artifact_plan.artifacts:
        return
    graph = plan.settled_graph
    resolver = plan.module_resolution_cache
    if not _complete_observers(
        graph.scan_authority, plan.import_query_closure.observations
    ):
        return
    rows: list[object] = []
    sources = _RecordingTable[_RecordedQuerySource]()
    if source_lines is None:
        source_lines = {}
    recorded_lines: dict[str, object] = {}
    result_locations: dict[str, PythonSourceLocations] = {}
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
        if source.sha256 not in recorded_lines:
            if source.sha256 not in source_lines:
                snapshot = PythonSourceSnapshot.capture(source.path)
                if snapshot.sha256 != source.sha256:
                    raise PythonSourceChangedError(
                        f"source changed before graph publication: {source.path}"
                    )
                lines = None
                if (
                    resolver.is_stdlib_path(snapshot.path, context.stdlib_root)
                    and (summary := resolver.installed_stdlib_source_summaries)
                    is not None
                ):
                    lines = summary.line_identity(
                        snapshot,
                        package=python_package_name(
                            source.module, is_package=source.package
                        ),
                        target=context.target_python,
                    )
                source_lines[source.sha256] = lines or _source_line_identity(
                    snapshot, context.target_python
                )
            lines = source_lines[source.sha256]
            recorded_lines[source.sha256] = None if lines is None else lines.record()
            result_locations[source.sha256] = (
                PythonSourceLocations() if lines is None else lines.locations()
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
                    else digests.digest(
                        query, observation.result, result_locations[source.sha256]
                    )
                ),
            )
        )
    authority = graph.scan_authority.payload()
    older_foreign_writes: dict[str, list[str]] = {}
    for module, names in resolver.import_foreign_writes.items():
        settled = graph.scan_authority.by_module.get(module)
        basis = settled.namespace_premise if settled is not None else None
        settled_writes = basis.foreign_writes if basis is not None else None
        if settled_writes is None:
            older_foreign_writes[module] = sorted(names)
        elif extra := names.difference(settled_writes):
            older_foreign_writes[module] = sorted(extra)
    for row, source in zip(
        authority,
        sorted(graph.scan_authority.sources, key=lambda item: item.module_name),
    ):
        row["namespace_premise"] = premises.index(source.namespace_premise)
    # A program can carry the same escape-writer relation in every module
    # and in many of the narrower premises under which queries were observed.
    escape_writes = _RecordingTable[tuple[tuple[str, str], ...]]()
    lexical_universes = _RecordingTable[frozenset[str] | None]()
    premise_rows = []
    for premise in premises.rows:
        if premise is None:
            premise_rows.append(None)
            continue
        row = codec._namespace_premise_cache_identity(
            replace(premise, foreign_universe=None)
        )
        assert row is not None
        row["escape_writes"] = escape_writes.index(premise.escape_writes)
        row["foreign_universe"] = lexical_universes.index(premise.foreign_universe)
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
        "lexical_universes": [
            None if names is None else sorted(names) for names in lexical_universes.rows
        ],
        "older_foreign_writes": older_foreign_writes,
        "coalesced_arguments": [
            [str(path), sha]
            for path, sha in sorted(resolver.coalesced_import_arguments)
        ],
        "publications": [
            [str(path), sha, _publication_digest(facts, result_locations[sha])]
            for (path, sha), facts in sorted(resolver.import_publications.items())
        ]
        + list(retained_publications),
        "query_sources": [source.record() for source in sources.rows],
        "source_lines": recorded_lines,
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
        with context.measure("replay"):
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
        records = frontend_record_files()
        if records.put(_KIND, key, encoded):
            # The checked graph is a complete stage product. Commit its index
            # before lowering or codegen can fail or interrupt the operation.
            records.save()


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
        "lexical_universes",
        "older_foreign_writes",
        "coalesced_arguments",
        "publications",
        "query_sources",
        "source_lines",
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
        import_scan_diagnostics=context.scan_diagnostics,
        module_root_aliases=context.entry.module_root_aliases,
        normal_none_intrinsics=context.normal_none_intrinsics,
    )
    query_sources = tuple(
        _RecordedQuerySource.decode(raw) for raw in _list(value["query_sources"])
    )
    recorded_lines = _map(value["source_lines"])
    source_lines = dict(
        zip(
            recorded_lines,
            PythonAstLineIdentity.read_records(recorded_lines.values()),
            strict=True,
        )
    )
    if source_lines.keys() != {source.sha256 for source in query_sources}:
        return None
    snapshots: dict[Path, PythonSourceSnapshot] = {}
    source_changed = False
    locations = _ReplayLocations()
    relocated: set[tuple[str, Path]] = set()
    corresponding: set[tuple[str, Path]] = set()
    for source in query_sources:
        snapshot = snapshots.get(source.path)
        if snapshot is None:
            snapshot = PythonSourceSnapshot.capture(source.path)
            snapshots[source.path] = snapshot
        if snapshot.sha256 == source.sha256:
            if len(snapshot.content) != source.size:
                return None
            continue
        source_changed = True
        old = source_lines[source.sha256]
        if snapshot.sha256 not in source_lines:
            lines = _source_line_identity(snapshot, context.target_python)
            source_lines[snapshot.sha256] = lines
        new = source_lines[snapshot.sha256]
        if old is None or new is None:
            continue
        same_ast = old.digest == new.digest
        if not same_ast and old.lines != new.lines:
            continue
        translation = old.relocation_to(new)
        if translation is None:
            continue
        prior = locations.by_module.setdefault(source.module, translation)
        if (
            prior.slots != translation.slots
            or prior.columns != translation.columns
            or prior.target_columns != translation.target_columns
        ):
            return None
        owner = (source.module, source.path)
        corresponding.add(owner)
        if same_ast:
            relocated.add(owner)
    if any(
        source.module in locations.by_module
        and (source.module, source.path) not in corresponding
        for source in query_sources
    ):
        return None
    source_leases = {
        path: resolver.import_query_source_lease(snapshot)
        for path, snapshot in snapshots.items()
    }
    result_locations = {
        sha: PythonSourceLocations() if lines is None else lines.locations()
        for sha, lines in source_lines.items()
    }
    for raw in _list(value["coalesced_arguments"]):
        path_text, source_sha = _texts(raw)
        path = Path(path_text)
        snapshot = snapshots.get(path)
        if snapshot is None or source_sha not in source_lines:
            return None
        if snapshot.sha256 != source_sha:
            old, current = source_lines[source_sha], source_lines[snapshot.sha256]
            # A representative query cannot reprove its collapsed arguments
            # after a semantic change to the classification's source.
            if old is None or current is None or old.digest != current.digest:
                return None
        resolver.coalesced_import_arguments.add((path, snapshot.sha256))
    retained_publications: list[list[str]] = []
    for raw in _list(value["publications"]):
        path_text, source_sha, digest = _texts(raw)
        path = Path(path_text)
        snapshot = snapshots.get(path)
        if (
            snapshot is None
            or source_sha not in source_lines
            or not is_record_digest(digest)
        ):
            return None
        if snapshot.sha256 == source_sha:
            retained_publications.append([path_text, source_sha, digest])
            continue
        facts = python_module_publications(snapshot.tree)
        if _publication_digest(facts, result_locations[snapshot.sha256]) != digest:
            return None
        resolver.import_publications[path, snapshot.sha256] = facts
    for module, raw in _map(value["older_foreign_writes"]).items():
        resolver.import_foreign_writes[module] = set(_texts(raw))

    relocated_roots: dict[tuple[str, Path], set[tuple[str, ...]]] = {}
    for source in query_sources:
        owner = (source.module, source.path)
        if owner in relocated:
            relocated_roots.setdefault(owner, set()).add(source.selection.roots)
    current_owner_digests: dict[tuple[str, Path], str] = {}

    def current_custody(
        custody: _RuntimeImportScanCustody | None,
    ) -> _RuntimeImportScanCustody | None:
        if custody is None:
            return None
        digests = dict(custody.owner_ast_digests)
        for module, path in custody.owners:
            owner = (module, path)
            roots = relocated_roots.get(owner)
            if roots is None:
                continue
            if owner not in current_owner_digests:
                (native_roots,) = roots
                current_owner_digests[owner] = resolver.module_ast_digest(
                    path, native_roots=native_roots, target_python=context.target_python
                )
            digests[module] = current_owner_digests[owner]
        new = tuple(digests.items())
        return (
            custody
            if new == custody.owner_ast_digests
            else replace(custody, owner_ast_digests=new)
        )

    query_contexts = []
    for raw in _list(value["contexts"]):
        decoded = _RecordedQueryContext.decode(raw, context.entry.module_root_aliases)
        query_contexts.append(
            replace(decoded, custody=current_custody(decoded.custody))
        )
    escape_writes = _list(value["escape_writes"])
    if locations.by_module:
        escape_writes = [
            [
                [name, locations.site(_text(site))]
                for name, site in map(_list, _list(rows))
            ]
            for rows in escape_writes
        ]
    lexical_universes = _list(value["lexical_universes"])
    premises: list[PythonNamespacePremise | None] = []
    for raw in _list(value["premises"]):
        if raw is None:
            premises.append(None)
            continue
        premise = _map(raw)
        writer_sites = {
            field: locations.site(_text(premise[field]))
            for field in ("instance_class_writer", "frame_locals_writer", "cell_writer")
            if premise[field] is not None
        }
        premises.append(
            codec._decode_namespace_premise_cache_identity(
                {
                    **premise,
                    **writer_sites,
                    "foreign_universe": _table_row(
                        lexical_universes, premise["foreign_universe"]
                    ),
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
    checked_writes: set[tuple[Path, str, frozenset[str]]] = set()
    authority_rows = tuple(map(_map, _list(value["authority"])))
    if source_changed:
        authority_writers = {
            _text(row["module"]): _table_row(premises, row["namespace_premise"])
            for row in authority_rows
        }
    else:
        authority_writers = {}
    writer_names: dict[str, set[str]] = {}
    premise_views: dict[tuple[str, int], PythonNamespacePremise | None] = {}

    def premise_for(module: str, row: object) -> PythonNamespacePremise | None:
        key = (module, _int(row))
        if key not in premise_views:
            premise_views[key] = locations.premise(module, _table_row(premises, row))
        return premise_views[key]

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
        snapshot = snapshots[path]
        common = _table_row(query_contexts, context_id)
        premise = premise_for(source.module, premise_id)
        lease = source_leases[path]
        changed = lease.source_sha256 != source.sha256
        if (
            changed
            and (source.module, path) not in relocated
            and premise is not None
            and premise.foreign_universe is not None
        ):
            writer_view = (path, source.module, premise.foreign_universe)
            if writer_view not in checked_writes:
                if path not in name_universes:
                    name_universes[path] = python_namespace_name_universe(snapshot.text)
                names = name_universes[path]
                module = source.module
                if module not in writer_names:
                    prior = resolver.import_foreign_writes.get(module)
                    settled = authority_writers.get(module)
                    final = settled.foreign_writes if settled is not None else None
                    if prior is None and final is None:
                        return None
                    writer_names[module] = set(final or ()) | (prior or set())
                if names is None or any(
                    name not in premise.foreign_universe
                    for name in writer_names[module] & names
                ):
                    return None
                checked_writes.add(writer_view)
        owner = (
            None
            if raw_owner is None
            else locations.key(source.module, decode_python_source_key(raw_owner))
        )
        mode = cast(ImportScanMode, _text(raw_mode))
        if mode not in ("full", "module_init"):
            raise ValueError("invalid graph scan mode")
        purpose = SourceImportPurpose(_text(raw_purpose))
        result_digest = _table_row(result_digests, digest)
        query = ImportScanQuery(
            path,
            lease,
            None,
            source.module,
            source.package,
            mode,
            purpose,
            owner,
            decode_source_string_arguments(arguments),
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
        if changed and (source.module, path) not in relocated:
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
            # A correspondence translates coordinates only. The fresh scan
            # still has to reproduce every graph-facing result in that space.
            layout = (
                snapshot.sha256
                if (source.module, path) in corresponding
                else source.sha256
            )
            if (
                digests.digest(query, loaded.scan, result_locations[layout])
                != result_digest
            ):
                return None
        else:
            resolver.observe_persisted_import_scan(
                query, result_digest, result_locations[snapshot.sha256]
            )
    sources: list[_ModuleSourceScanAuthority] = []
    for row in authority_rows:
        name = _text(row["module"])
        path = Path(_text(row["path"]))
        snapshot = snapshots.get(path)
        if snapshot is None or row["source_sha256"] is None:
            return None
        selection = ModuleBodySelection.from_payload(row["body_selection"])
        if name in locations.by_module and selection.declaration_only:
            selection = replace(
                selection,
                declaration_only=tuple(
                    locations.key(name, key) for key in selection.declaration_only
                ),
            )
        sources.append(
            _ModuleSourceScanAuthority(
                name,
                path,
                cast(ImportScanMode, _text(row["mode"])),
                body_selection=selection,
                is_package=_bool(row["is_package"]),
                requires_runtime_package_anchor=_bool(
                    row["requires_runtime_package_anchor"]
                ),
                uses_runtime_import_protocol=_bool(row["uses_runtime_import_protocol"]),
                deferred_owners=frozenset(
                    (
                        locations.key(
                            name, decode_python_source_key(_map(item)["source"])
                        ),
                        decode_source_string_arguments(_map(item)["arguments"]),
                    )
                    for item in _list(row["deferred_bodies"])
                ),
                read_callables=frozenset(
                    locations.key(name, decode_python_source_key(item))
                    for item in _list(row["read_callables"])
                ),
                namespace_premise=premise_for(name, row["namespace_premise"]),
                namespace_own_names=None
                if row["namespace_own_names"] is None
                else frozenset(_texts(row["namespace_own_names"])),
                namespace_own_line=None
                if row["namespace_own_line"] is None
                else locations.line(name, _int(row["namespace_own_line"])),
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
        current_custody(_read_custody(value["custody"])),
        context.runtime_machinery,
        native,
        _redirects(value["redirects"], locations),
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
    if source_changed:
        try:
            # Republish from the verified query closure so result digests,
            # scoped premises and source generations advance together.
            with context.measure("publication"):
                _publish_import_graph(
                    plan,
                    context,
                    source_lines,
                    retained_publications=retained_publications,
                )
        except OSError:
            # The admitted plan remains usable when a cache write fails.
            pass
    else:
        record.refresh()
    return plan

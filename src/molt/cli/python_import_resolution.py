"""Shared local-Python import resolution for compiler cache authorities."""

from __future__ import annotations

import ast
import os
from molt.python_private_names import python_source_field
import stat
import sys
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path
from threading import RLock
from typing import Literal

from molt.cli.module_source import PythonSourceSnapshot
from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    PythonBindingIndex,
    analyze_python_bindings,
    python_dynamic_import_facts_required,
)

from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    StaticImportProjection,
    bind_static_import_call_arguments,
    dunder_globals_state_from_expression,
    metadata_value_from_expression,
    module_import_context_with_metadata_proof,
    static_import_discovery,
    source_import_requests_from_expressions,
    project_static_import_request,
    static_import_level_from_result,
    static_import_fromlist_is_empty,
)

from molt.compiler_analysis.python_lexical_scope import python_eager_nodes


# PEP 3147's bytecode cache directory; Python writes it, sources never live there.
_BYTECODE_CACHE_DIR = "__pycache__"


@dataclass(frozen=True)
class PythonImportPolicy:
    """Resolution policy for one import-closure consumer.

    Frontend semantic fingerprints intentionally follow module-level ``molt``
    edges without executing package aggregates. Executable tools need the full
    lexical graph and CPython's parent-package execution semantics. Keeping the
    distinction as data prevents separate path and relative-import resolvers.
    ``source_dependency`` may retain known source-state candidates after an
    execution proof is lost. Callback-capable transitions and possible globals
    operands retain an unknown obligation beside those candidates. ``semantic``
    never consumes discovery rows, and neither purpose treats a lexical twin as
    a known dynamic package operand. Complete tooling consumers explicitly select
    ``local_inventory`` for unknown literal-relative anchors. Their symbolic
    coverage obligations are separate from checked dynamic import manifests.
    """

    module_level_only: bool
    include_parent_packages: bool
    fail_on_nonliteral_dynamic_import: bool
    allowed_prefix: str | None = None
    purpose: Literal["semantic", "source_dependency"] = "semantic"
    # Source consumers may cover an unknown literal-relative anchor by capturing
    # every admitted local owner. This is dependency coverage, never metadata
    # proof. Candidate-only and semantic queries retain their existing contract.
    unknown_relative_sources: Literal["require_known", "local_inventory"] = (
        "require_known"
    )

    def __post_init__(self) -> None:
        if self.unknown_relative_sources == "local_inventory" and (
            self.purpose != "source_dependency"
        ):
            raise ValueError(
                "local import inventory is only a source dependency policy"
            )


def _local_import_binding_policy() -> PythonBindingPolicy:
    """Compiler/tool Python source executes on the parser's host interpreter."""
    return PythonBindingPolicy(target_python=(sys.version_info[0], sys.version_info[1]))


def local_import_analysis_identity() -> tuple[object, ...]:
    """Use the parser and binding authority's own versions in persisted graphs."""
    return (
        sys.implementation.name,
        tuple(sys.version_info),
        python_binding_flow._ANALYSIS_SCHEMA,
        asdict(_local_import_binding_policy()),
    )


@dataclass(frozen=True, slots=True)
class LocalPythonImportRequest:
    """A projected request, retaining owner/fromlist grouping until resolution."""

    kind: Literal["direct", "from", "dynamic", "manifest"]
    candidates: tuple[str, ...]
    line: int = 0
    column: int = 0


@dataclass(frozen=True, slots=True)
class LocalPythonImportDiagnostic:
    line: int
    column: int
    message: str


@dataclass(frozen=True, slots=True)
class LocalPythonRelativeImportObligation:
    """A literal request whose unknown anchor requires complete local coverage.

    Names are operands, not resolved modules. No semantic consumer may turn this
    record into an import edge; only a fresh resolver inventory discharges it.
    """

    kind: Literal["statement", "import_module", "dunder_import"]
    name: str
    level: int
    fromlist: tuple[str, ...]
    line: int
    column: int


@dataclass(frozen=True, slots=True)
class LocalPythonImportAnalysis:
    requests: tuple[LocalPythonImportRequest, ...]
    unresolved_dynamic_imports: tuple[LocalPythonImportDiagnostic, ...] = ()
    # Source-file hashing edges only. These never authorize runtime metadata,
    # product import sealing, or intrinsic-provider status.
    discovery_requests: tuple[LocalPythonImportRequest, ...] = ()
    relative_source_obligations: tuple[LocalPythonRelativeImportObligation, ...] = ()

    def requests_for(
        self, policy: PythonImportPolicy
    ) -> tuple[LocalPythonImportRequest, ...]:
        if policy.purpose == "source_dependency":
            return (*self.requests, *self.discovery_requests)
        return self.requests

    def validate_dynamic_contract(
        self, path: Path, policy: PythonImportPolicy, expected: int | None
    ) -> None:
        if self.relative_source_obligations and (
            policy.purpose != "source_dependency"
            or policy.unknown_relative_sources != "local_inventory"
        ):
            raise ValueError(f"unadmitted relative source coverage policy for {path}")
        count = len(self.unresolved_dynamic_imports)
        if expected is not None and count != expected:
            raise ValueError(
                f"dynamic Python import manifest drift in {path}: expected "
                f"{expected} unresolved imports, found {count}"
            )
        if expected is None and policy.fail_on_nonliteral_dynamic_import and count:
            diagnostic = self.unresolved_dynamic_imports[0]
            raise ValueError(
                f"{diagnostic.message} at {path}:{diagnostic.line}:{diagnostic.column}"
            )


@dataclass(frozen=True, slots=True)
class LocalPythonModuleSource:
    """An execution context; distinct import names may share captured bytes."""

    name: str
    path: Path


@dataclass(frozen=True, slots=True)
class LocalPythonSourceInventory:
    """Complete resolver-owned local source domain and namespace topology."""

    sources: tuple[LocalPythonModuleSource, ...]
    packages: tuple[tuple[str, tuple[Path, ...]], ...]


def relative_python_module_name(path: Path, root: Path) -> str:
    parts = list(path.relative_to(root).parts)
    if not parts or not parts[-1].endswith(".py"):
        raise ValueError(f"local Python source is not a .py file: {path}")
    if parts[-1] == "__init__.py":
        parts.pop()
    else:
        parts[-1] = parts[-1][:-3]
    return ".".join(parts)


@dataclass(frozen=True)
class _ResolvedLocalModule:
    source: LocalPythonModuleSource | None
    package_locations: tuple[Path, ...]
    parent_initializers: tuple[LocalPythonModuleSource, ...]


@dataclass(frozen=True, slots=True)
class _LocalPathProbe:
    path: Path
    mode: int


@dataclass(frozen=True)
class LocalPythonModuleResolver:
    search_roots: tuple[Path, ...]
    source_roots: tuple[Path, ...] | None = None
    _resolution_cache: dict[str, _ResolvedLocalModule | None] = field(
        default_factory=dict,
        compare=False,
        repr=False,
    )
    _cache_lock: RLock = field(default_factory=RLock, compare=False, repr=False)
    _inventory_cache: dict[tuple[str | None, bool], LocalPythonSourceInventory] = field(
        default_factory=dict, compare=False, repr=False
    )

    def __post_init__(self) -> None:
        resolved = tuple(root.resolve() for root in self.search_roots)
        if not resolved:
            raise ValueError("local Python module resolver requires a search root")
        object.__setattr__(self, "search_roots", resolved)
        sources = (
            resolved
            if self.source_roots is None
            else tuple(root.resolve() for root in self.source_roots)
        )
        if not sources:
            raise ValueError("local Python module resolver requires a source root")
        object.__setattr__(self, "source_roots", sources)

    def _owns_source(self, path: Path) -> bool:
        assert self.source_roots is not None
        return any(path.is_relative_to(root) for root in self.source_roots)

    def capture_source(self, path: Path) -> PythonSourceSnapshot:
        try:
            return PythonSourceSnapshot.capture(path)
        except OSError as exc:
            raise ValueError(f"cannot read local Python source {path}: {exc}") from exc

    def module_identity(self, path: Path) -> tuple[str, str]:
        """Name a source already canonicalized at the discovery boundary."""
        if not self._owns_source(path):
            raise ValueError(f"Python source is outside local source roots: {path}")
        for root in self.search_roots:
            try:
                path.relative_to(root)
            except ValueError:
                continue
            module = relative_python_module_name(path, root)
            package = (
                module if path.name == "__init__.py" else module.rpartition(".")[0]
            )
            return module, package
        raise ValueError(f"Python source is outside local search roots: {path}")

    def source_for_module(self, module: str) -> Path | None:
        resolution = self._resolve_module(module)
        return (
            resolution.source.path
            if resolution is not None and resolution.source is not None
            else None
        )

    def source_inventory(
        self, *, allowed_prefix: str | None, include_parent_packages: bool
    ) -> LocalPythonSourceInventory:
        """Enumerate the inverse of this resolver, retaining shadows and aliases.

        Walk only search locations selected by the forward PathFinder authority.
        Every regular source may execute before a literal relative suffix fails,
        so filtering successful suffix matches would omit real dependencies.
        Namespace directories are identity inputs even when they have no source.
        Directory cycles cannot yield a finite import-name inventory; reject them
        rather than publishing partial coverage. Enumeration errors also fail.
        """

        key = (allowed_prefix, include_parent_packages)
        with self._cache_lock:
            cached = self._inventory_cache.get(key)
            if cached is not None:
                return cached
            sources: set[LocalPythonModuleSource] = set()
            packages: dict[str, tuple[Path, ...]] = {}
            pending: list[tuple[str, tuple[Path, ...], frozenset[Path]]] = []
            if allowed_prefix is None:
                pending.append(("", self.search_roots, frozenset()))
            else:
                resolution = self._resolve_module(allowed_prefix)
                if resolution is not None:
                    if include_parent_packages:
                        sources.update(resolution.parent_initializers)
                    if resolution.source is not None:
                        sources.add(resolution.source)
                    pending.append(
                        (allowed_prefix, resolution.package_locations, frozenset())
                    )
            while pending:
                prefix, locations, ancestors = pending.pop()
                if not locations:
                    continue
                if ancestors.intersection(locations):
                    raise ValueError(f"cyclic local Python search topology at {prefix}")
                packages[prefix] = locations
                # The synthetic root may contain nested search roots. Entering
                # one through a second qualified name is an alias, not a cycle.
                if prefix:
                    ancestors = ancestors.union(locations)
                children: set[str] = set()
                for location in locations:
                    try:
                        with os.scandir(location) as entries:
                            for entry in entries:
                                name = entry.name
                                if entry.is_file() and name.endswith(".py"):
                                    name = name[:-3]
                                elif not entry.is_dir() or name == _BYTECODE_CACHE_DIR:
                                    # Python creates its bytecode cache on first
                                    # import; as a namespace member it would
                                    # change the domain identity mid-run.
                                    continue
                                # A dotted filename is not one PathFinder segment.
                                if name and "." not in name:
                                    children.add(name)
                    except OSError as exc:
                        raise ValueError(
                            f"cannot enumerate local Python source domain {location}: {exc}"
                        ) from exc
                for child in sorted(children):
                    name = f"{prefix}.{child}" if prefix else child
                    resolution = self._resolve_module(name)
                    if resolution is None:
                        continue
                    if resolution.source is not None:
                        sources.add(resolution.source)
                    if resolution.package_locations:
                        pending.append((name, resolution.package_locations, ancestors))
            result = LocalPythonSourceInventory(
                tuple(sorted(sources, key=lambda item: (item.name, item.path))),
                tuple(sorted(packages.items())),
            )
            self._inventory_cache[key] = result
            return result

    def _probe_path(self, path: Path) -> _LocalPathProbe | None:
        """Admit existing candidates once, retaining kind and canonical ownership."""

        try:
            mode = path.stat().st_mode
            resolved = path.resolve()
        except OSError:
            return None
        if self._owns_source(resolved) and any(
            resolved.is_relative_to(root) for root in self.search_roots
        ):
            return _LocalPathProbe(resolved, mode)
        return None

    def _resolve_module(self, module: str) -> _ResolvedLocalModule | None:
        if not module:
            return None
        parts = module.split(".")
        if any(not part or "/" in part or "\\" in part for part in parts):
            raise ValueError(f"invalid local Python module name: {module!r}")
        with self._cache_lock:
            if module in self._resolution_cache:
                return self._resolution_cache[module]

            # Mirror PathFinder one segment at a time. Namespace portions keep
            # searching; the first regular package or module stops the current
            # segment's search. A regular package then owns the next segment's
            # search path, while a regular module cannot have children.
            locations = self.search_roots
            parents: list[LocalPythonModuleSource] = []
            start_index = 0
            for prefix_length in range(len(parts) - 1, 0, -1):
                prefix = ".".join(parts[:prefix_length])
                if prefix not in self._resolution_cache:
                    continue
                cached_prefix = self._resolution_cache[prefix]
                if cached_prefix is None:
                    self._resolution_cache[module] = None
                    return None
                if not cached_prefix.package_locations:
                    self._resolution_cache[module] = None
                    return None
                locations = cached_prefix.package_locations
                parents = list(cached_prefix.parent_initializers)
                if cached_prefix.source is not None:
                    parents.append(cached_prefix.source)
                start_index = prefix_length
                break

            for index in range(start_index, len(parts)):
                part = parts[index]
                prefix = ".".join(parts[: index + 1])
                namespace_locations: list[Path] = []
                regular_source: Path | None = None
                regular_package_location: Path | None = None
                regular_is_package = False

                for location in locations:
                    package = self._probe_path(location / part)
                    if package is not None and stat.S_ISDIR(package.mode):
                        initializer = self._probe_path(package.path / "__init__.py")
                        if initializer is not None and stat.S_ISREG(initializer.mode):
                            regular_source = initializer.path
                            regular_package_location = package.path
                            regular_is_package = True
                            break
                        namespace_locations.append(package.path)

                    source = self._probe_path(location / f"{part}.py")
                    if source is not None and stat.S_ISREG(source.mode):
                        regular_source = source.path
                        regular_package_location = None
                        regular_is_package = False
                        break

                final_segment = index == len(parts) - 1
                if regular_source is not None:
                    module_source = LocalPythonModuleSource(prefix, regular_source)
                    result = _ResolvedLocalModule(
                        source=module_source,
                        package_locations=(
                            (regular_package_location,)
                            if regular_package_location is not None
                            else ()
                        ),
                        parent_initializers=tuple(parents),
                    )
                    self._resolution_cache[prefix] = result
                    if final_segment:
                        return result
                    if not regular_is_package or regular_package_location is None:
                        self._resolution_cache[module] = None
                        return None
                    parents.append(module_source)
                    locations = (regular_package_location,)
                    continue

                if not namespace_locations:
                    self._resolution_cache[prefix] = None
                    self._resolution_cache[module] = None
                    return None
                locations = tuple(namespace_locations)
                result = _ResolvedLocalModule(
                    source=None,
                    package_locations=locations,
                    parent_initializers=tuple(parents),
                )
                self._resolution_cache[prefix] = result
                if final_segment:
                    return result

            raise AssertionError("non-empty module resolution exhausted no segment")

    def resolve_import_sources(
        self, module: str, *, include_parent_packages: bool
    ) -> tuple[LocalPythonModuleSource, ...]:
        """Resolve the requested context plus executed regular parents.

        A namespace has no source of its own but may still execute a regular
        ancestor's initializer. Missing fromlist members fall back to their
        owner without discarding that namespace ancestry.
        """
        resolution = self._resolve_module(module)
        if resolution is None:
            owner, separator, _name = module.rpartition(".")
            if separator:
                resolution = self._resolve_module(owner)
        if resolution is None:
            return ()
        parents = resolution.parent_initializers if include_parent_packages else ()
        return (
            (*parents, resolution.source) if resolution.source is not None else parents
        )


def _require_import_projection(
    request: StaticImportRequest,
    projection: StaticImportProjection,
    path: Path,
) -> tuple[str, ...]:
    """Format canonical typed errors without reevaluating the projection."""
    if projection.error == "no_parent":
        if request.kind == "import_module":
            raise ValueError(f"relative import_module requires a package in {path}")
        raise ValueError(f"relative import has no known parent package in {path}")
    if projection.error == "beyond_top":
        raise ValueError(f"relative import escapes local package in {path}")
    if projection.error == "empty_name":
        raise ValueError(f"empty Python module name in {path}")
    if projection.error == "invalid_level":
        raise ValueError(f"invalid __import__ level in {path}")
    if projection.error == "negative_level":
        raise ValueError(f"negative __import__ level in {path}")
    if projection.error == "missing_globals":
        raise ValueError(f"relative __import__ requires explicit globals in {path}")
    if projection.error in {
        "invalid_package",
        "unknown_package",
        "invalid_spec",
        "unknown_spec",
    }:
        raise ValueError(f"non-literal or invalid import package in {path}")
    if projection.error == "missing_name":
        raise ValueError(f"relative import globals are missing __name__ in {path}")
    if projection.error in {"invalid_name", "unknown_name"}:
        raise ValueError(f"relative import has invalid or dynamic __name__ in {path}")
    return projection.modules


def _dynamic_import_projection(
    call: ast.Call,
    *,
    kind: Literal["import_module", "dunder_import"],
    contexts: tuple[ModuleImportContext, ...],
    binding_index: PythonBindingIndex,
    path: Path,
    source_discovery: bool = False,
    cover_unknown_relative: bool = False,
) -> tuple[
    tuple[str, ...],
    tuple[ValueError, ...],
    tuple[LocalPythonRelativeImportObligation, ...],
]:
    is_import_module = kind == "import_module"
    if kind not in {"import_module", "dunder_import"}:
        return (), (), ()
    try:
        arguments = bind_static_import_call_arguments(call, kind)
    except ValueError as exc:
        raise ValueError(f"{exc} in {path}") from exc
    if arguments is None:
        return (), (), ()
    if arguments.requires_runtime_binding:
        raise ValueError(
            f"dynamic import argument expansion requires a manifest in {path}"
        )
    name_arg = arguments.name
    if not isinstance(name_arg, ast.Constant) or not isinstance(name_arg.value, str):
        raise ValueError(f"non-literal dynamic Python import in {path}")
    name = name_arg.value
    level_arg = arguments.level if not is_import_module else None
    level, level_is_invalid = (
        static_import_level_from_result(binding_index.expression_result(level_arg))
        if level_arg is not None
        else (0, False)
    )
    if level is None and not level_is_invalid:
        raise ValueError(f"non-literal __import__ level in {path}")
    fromlist: list[str] = []
    fromlist_arg = arguments.fromlist if not is_import_module else None
    if fromlist_arg is not None and not static_import_fromlist_is_empty(
        binding_index.expression_result(fromlist_arg)
    ):
        if not isinstance(fromlist_arg, (ast.Tuple, ast.List)):
            raise ValueError(f"non-literal __import__ fromlist in {path}")
        for item in fromlist_arg.elts:
            if not isinstance(item, ast.Constant) or not isinstance(item.value, str):
                raise ValueError(f"non-literal __import__ fromlist in {path}")
            if item.value == "*":
                raise ValueError(
                    f"dynamic __import__ star fromlist requires a manifest in {path}"
                )
            fromlist.append(item.value)
    package_arg = arguments.package if is_import_module else None
    globals_arg = arguments.globals if not is_import_module else None
    modules: set[str] = set()
    errors: list[ValueError] = []
    obligations: list[LocalPythonRelativeImportObligation] = []
    for context in contexts:
        if is_import_module:
            request = StaticImportRequest.import_module(
                name,
                metadata_value_from_expression(
                    package_arg,
                    context,
                    fact_result=binding_index.expression_result,
                    expression_fact=binding_index.expression_fact,
                    call_fact=binding_index.call_fact(call),
                ),
            )
        else:
            request = StaticImportRequest(
                "dunder_import",
                name,
                level=0 if level is None else level,
                level_is_invalid=level_is_invalid,
                fromlist=tuple(fromlist),
                globals_state=dunder_globals_state_from_expression(
                    globals_arg,
                    context,
                    fact_result=binding_index.expression_result,
                    expression_fact=binding_index.expression_fact,
                    call_fact=binding_index.call_fact(call),
                ),
                globals_were_supplied=globals_arg is not None,
            )
        projected_requests = (
            source_import_requests_from_expressions(
                request,
                context,
                package_expression=package_arg,
                globals_expression=globals_arg,
                source_contexts_for_read=lambda expression: tuple(
                    context.with_state(state)
                    for state in binding_index.module_import_flow.source_states_for(
                        expression
                    )
                ),
                fact_result=binding_index.expression_result,
                expression_fact=binding_index.expression_fact,
                call_fact=binding_index.call_fact(call),
            )
            if source_discovery
            else (request,)
        )
        for projected_request in projected_requests:
            projection = project_static_import_request(projected_request, context)
            obligation = (
                _relative_source_obligation(projected_request, projection, call)
                if cover_unknown_relative
                else None
            )
            if obligation is not None:
                obligations.append(obligation)
                continue
            try:
                modules.update(
                    _require_import_projection(
                        projected_request,
                        projection,
                        path,
                    )
                )
            except ValueError as exc:
                errors.append(exc)
    return tuple(sorted(modules)), tuple(errors), tuple(dict.fromkeys(obligations))


def _relative_source_obligation(
    request: StaticImportRequest,
    projection: StaticImportProjection,
    node: ast.ImportFrom | ast.Call,
) -> LocalPythonRelativeImportObligation | None:
    """Classify only canonical unknown anchors, never invalid or dynamic names."""

    if projection.error not in {"unknown_package", "unknown_spec", "unknown_name"}:
        return None
    name = request.name
    level = request.level
    if request.kind == "import_module":
        level = len(name) - len(name.lstrip("."))
        name = name[level:]
    if level <= 0 or request.level_is_invalid:
        return None
    if name and any(
        not part or "/" in part or "\\" in part for part in name.split(".")
    ):
        return None
    return LocalPythonRelativeImportObligation(
        request.kind, name, level, request.fromlist, node.lineno, node.col_offset
    )


def analyze_local_imports(
    source: PythonSourceSnapshot,
    module_source: LocalPythonModuleSource,
    policy: PythonImportPolicy,
    *,
    expected_nonliteral_dynamic_imports: int | None = None,
    nonliteral_dynamic_import_targets: tuple[str, ...] = (),
) -> LocalPythonImportAnalysis:
    """Project source requests; demand binding facts only for semantic queries.

    Absolute statement candidates do not depend on package metadata or alias
    identity. Relative statements and sources with dynamic identity origins continue to
    use the canonical binding/import-flow authority, including deferred aliases.
    This is a conservative dependency graph, not an execution-reachability proof.
    """

    path = source.path
    tree = source.tree
    if path != module_source.path:
        raise ValueError(f"Python module context does not own captured source: {path}")
    module = module_source.name
    binding_policy = _local_import_binding_policy()
    base_context = ModuleImportContext(
        module_name=module,
        is_package=path.name == "__init__.py",
        spec_name=module,
        target_python=binding_policy.target_python,
    )
    binding_index: PythonBindingIndex | None = None

    def bindings() -> PythonBindingIndex:
        nonlocal binding_index
        if binding_index is None:
            binding_index = analyze_python_bindings(
                tree,
                source_digest=source.ast_digest,
                policy=replace(
                    binding_policy,
                    module_name=module,
                    module_spec_name=module,
                    module_is_package=path.name == "__init__.py",
                    module_execution_kind="imported",
                    analyze_deferred_bodies=not policy.module_level_only,
                    include_import_discovery=policy.purpose == "source_dependency",
                ),
            )
        return binding_index

    def contexts_for(
        node: ast.AST, *, source_discovery: bool = False
    ) -> tuple[ModuleImportContext, ...]:
        flow = bindings().module_import_flow
        return tuple(
            base_context.with_state(state)
            for state in (
                flow.source_states_for(node)
                if source_discovery
                else flow.states_for(node)
            )
        )

    nodes = (
        python_eager_nodes(
            tree,
            target_python=binding_policy.target_python,
            fact_result=lambda node: bindings().expression_result(node),
        )
        if policy.module_level_only
        else tuple(ast.walk(tree))
    )
    requests: list[LocalPythonImportRequest] = []
    discovery_requests: list[LocalPythonImportRequest] = []
    diagnostics: list[LocalPythonImportDiagnostic] = []
    relative_obligations: list[LocalPythonRelativeImportObligation] = []
    cover_unknown_relative = (
        policy.purpose == "source_dependency"
        and policy.unknown_relative_sources == "local_inventory"
    )
    for node in nodes:
        if isinstance(node, ast.Import):
            requests.extend(
                LocalPythonImportRequest(
                    "direct", (alias.name,), node.lineno, node.col_offset
                )
                for alias in node.names
            )
            continue
        if not isinstance(node, ast.ImportFrom):
            continue
        # Keep unresolved fromlist candidates in the persistent graph.
        # Resolution applies the owner fallback against the live filesystem,
        # so adding ``pkg/name.py`` later changes the edge without reparsing.
        request = StaticImportRequest.statement(
            node.module or "",
            level=node.level,
            fromlist=tuple(python_source_field(alias, "name") for alias in node.names),
        )
        projection_errors: list[ValueError] = []
        contexts = contexts_for(node) if node.level else (base_context,)
        source_contexts = (
            contexts_for(node, source_discovery=True)
            if node.level and policy.purpose == "source_dependency"
            else contexts
        )
        if node.level:
            statement_fact = bindings().statement_fact(node)
            contexts = tuple(
                module_import_context_with_metadata_proof(
                    context,
                    statement_fact.module_metadata_at_entry
                    if statement_fact is not None
                    else None,
                )
                for context in contexts
            )
        for context in contexts:
            projection = project_static_import_request(request, context)
            try:
                candidates = _require_import_projection(request, projection, path)
                if candidates:
                    requests.append(
                        LocalPythonImportRequest(
                            "from", candidates, node.lineno, node.col_offset
                        )
                    )
            except ValueError as exc:
                if projection.error not in {
                    "unknown_package",
                    "unknown_spec",
                    "unknown_name",
                    "no_parent",
                    "beyond_top",
                }:
                    raise
                projection_errors.append(exc)
        if projection_errors:
            discovery = static_import_discovery(request, source_contexts)
            if policy.purpose == "source_dependency":
                # Keep each owner/fromlist group intact for local topology
                # resolution, even when source branches use different owners.
                for context in source_contexts:
                    candidates = static_import_discovery(
                        request, (context,)
                    ).source_modules
                    if candidates:
                        discovery_requests.append(
                            LocalPythonImportRequest(
                                "from", candidates, node.lineno, node.col_offset
                            )
                        )
                if discovery.source_complete:
                    continue
                if cover_unknown_relative and source_contexts:
                    uncovered = False
                    for context in source_contexts:
                        projection = project_static_import_request(request, context)
                        obligation = _relative_source_obligation(
                            request, projection, node
                        )
                        if obligation is not None:
                            relative_obligations.append(obligation)
                        elif projection.error is not None:
                            # Definite invalid/beyond-top alternatives still fail;
                            # an unknown sibling is not permission to erase them.
                            _require_import_projection(request, projection, path)
                        elif projection.requires_runtime:
                            uncovered = True
                    if not uncovered:
                        continue
            # Eager and full-depth consumers share the same explicit policy
            # validation. Candidate-only policies retain diagnostics; closed
            # consumers still fail unless their checked manifest covers them.
            diagnostics.append(
                LocalPythonImportDiagnostic(
                    node.lineno,
                    node.col_offset,
                    str(projection_errors[0]).removesuffix(f" in {path}"),
                )
            )

    if python_dynamic_import_facts_required(tree, nodes=nodes):
        for node in nodes:
            if not isinstance(node, ast.Call):
                continue
            fact = bindings().call_fact(node)
            kinds = fact.possible_import_call_kinds() if fact is not None else ()
            if not kinds:
                continue
            targets_for_call: set[str] = set()
            discovery_targets: set[str] = set()
            errors: list[ValueError] = []
            semantic_complete = True
            source_complete = policy.purpose == "source_dependency"
            for kind in kinds:
                try:
                    targets, dynamic_projection_errors, _ = _dynamic_import_projection(
                        node,
                        kind=kind,
                        contexts=contexts_for(node),
                        binding_index=bindings(),
                        path=path,
                    )
                except ValueError as exc:
                    targets, dynamic_projection_errors = (), (exc,)
                targets_for_call.update(targets)
                if not dynamic_projection_errors:
                    discovery_targets.update(targets)
                    continue
                semantic_complete = False
                errors.extend(dynamic_projection_errors)
                if policy.purpose != "source_dependency":
                    source_complete = False
                    continue
                try:
                    candidates, source_errors, obligations = _dynamic_import_projection(
                        node,
                        kind=kind,
                        contexts=contexts_for(node, source_discovery=True),
                        binding_index=bindings(),
                        path=path,
                        source_discovery=True,
                        cover_unknown_relative=cover_unknown_relative,
                    )
                except ValueError as exc:
                    candidates, source_errors, obligations = (), (exc,), ()
                discovery_targets.update(candidates)
                relative_obligations.extend(obligations)
                source_complete &= not source_errors
            if semantic_complete:
                requests.extend(
                    LocalPythonImportRequest(
                        "dynamic", (target,), node.lineno, node.col_offset
                    )
                    for target in sorted(targets_for_call)
                )
                continue
            if policy.purpose == "source_dependency":
                discovery_requests.extend(
                    LocalPythonImportRequest(
                        "dynamic", (target,), node.lineno, node.col_offset
                    )
                    for target in sorted(discovery_targets)
                )
            if not source_complete:
                diagnostics.append(
                    LocalPythonImportDiagnostic(
                        node.lineno,
                        node.col_offset,
                        str(errors[0]).removesuffix(f" in {path}"),
                    )
                )

    if not policy.module_level_only:
        requests.extend(
            LocalPythonImportRequest("manifest", (target,))
            for target in nonliteral_dynamic_import_targets
        )

    analysis = LocalPythonImportAnalysis(
        tuple(dict.fromkeys(requests)),
        tuple(diagnostics),
        tuple(dict.fromkeys(discovery_requests)),
        tuple(dict.fromkeys(relative_obligations)),
    )
    analysis.validate_dynamic_contract(
        path, policy, expected_nonliteral_dynamic_imports
    )
    return analysis


def resolve_local_import_requests(
    analysis: LocalPythonImportAnalysis,
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
) -> set[LocalPythonModuleSource]:
    """Resolve grouped requests against this traversal's live local topology."""

    dependencies: set[LocalPythonModuleSource] = set()
    for request in analysis.requests_for(policy):
        candidates = request.candidates
        if (
            request.kind == "from"
            and not policy.include_parent_packages
            and len(candidates) > 1
        ):
            # A named submodule wins over its aggregate package. Each unresolved
            # member still independently falls back to its owner; do not discard
            # an attribute provider just because another member is a submodule.
            candidates = candidates[1:]
        for target in candidates:
            if policy.allowed_prefix is not None and not (
                target == policy.allowed_prefix
                or target.startswith(f"{policy.allowed_prefix}.")
            ):
                continue
            dependencies.update(
                resolver.resolve_import_sources(
                    target, include_parent_packages=policy.include_parent_packages
                )
            )
    return dependencies


def local_import_targets(
    path: Path,
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
    *,
    expected_nonliteral_dynamic_imports: int | None = None,
    nonliteral_dynamic_import_targets: tuple[str, ...] = (),
) -> set[str]:
    """Expose source candidates, including explicit inventory coverage if selected.

    Semantic policy never admits that coverage. The graph closure retains its
    symbolic provenance separately instead of serializing inventory names here.
    """

    path = path.resolve()
    analysis = analyze_local_imports(
        resolver.capture_source(path),
        LocalPythonModuleSource(resolver.module_identity(path)[0], path),
        policy,
        expected_nonliteral_dynamic_imports=expected_nonliteral_dynamic_imports,
        nonliteral_dynamic_import_targets=nonliteral_dynamic_import_targets,
    )
    targets = {
        candidate
        for request in analysis.requests_for(policy)
        for candidate in request.candidates
    }
    if analysis.relative_source_obligations:
        targets.update(
            source.name
            for source in resolver.source_inventory(
                allowed_prefix=policy.allowed_prefix,
                include_parent_packages=policy.include_parent_packages,
            ).sources
        )
    return targets

"""Shared local-Python import resolution for compiler cache authorities."""

from __future__ import annotations

import ast
import hashlib
import sys
from dataclasses import asdict, dataclass, field, replace
from functools import cached_property
from pathlib import Path
from threading import RLock
from typing import Literal

from molt.compiler_analysis import python_binding_flow
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    PythonBindingIndex,
    analyze_python_bindings,
    python_ast_digest,
)

from molt.compiler_analysis.python_imports import (
    ModuleImportContext,
    StaticImportRequest,
    bind_static_import_call_arguments,
    dunder_globals_state_from_expression,
    metadata_value_from_expression,
    project_static_import_request,
)


@dataclass(frozen=True)
class PythonImportPolicy:
    """Resolution policy for one import-closure consumer.

    Frontend semantic fingerprints intentionally follow module-level ``molt``
    edges without executing package aggregates. Executable tools need the full
    lexical graph and CPython's parent-package execution semantics. Keeping the
    distinction as data prevents the two consumers from growing separate path
    and relative-import resolvers again.
    """

    module_level_only: bool
    include_parent_packages: bool
    fail_on_nonliteral_dynamic_import: bool
    allowed_prefix: str | None = None


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


@dataclass(frozen=True)
class PythonSourceSnapshot:
    """One captured byte generation supplies both the graph key and its AST."""

    path: Path
    content: bytes

    @cached_property
    def sha256(self) -> str:
        return hashlib.sha256(self.content).hexdigest()

    @cached_property
    def tree(self) -> ast.Module:
        try:
            # Parsing bytes honors PEP 263 without a second file read.
            return ast.parse(self.content, filename=str(self.path))
        except (SyntaxError, UnicodeError, ValueError) as exc:
            raise ValueError(
                f"cannot parse local Python source {self.path}: {exc}"
            ) from exc

    @cached_property
    def ast_digest(self) -> str:
        return python_ast_digest(self.tree)


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
class LocalPythonImportAnalysis:
    requests: tuple[LocalPythonImportRequest, ...]
    unresolved_dynamic_imports: tuple[LocalPythonImportDiagnostic, ...] = ()

    def validate_dynamic_contract(
        self, path: Path, policy: PythonImportPolicy, expected: int | None
    ) -> None:
        if policy.module_level_only:
            return
        count = len(self.unresolved_dynamic_imports)
        if expected is not None and count != expected:
            raise ValueError(
                f"dynamic Python import manifest drift in {path}: expected "
                f"{expected} non-literal calls, found {count}"
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


@dataclass(frozen=True)
class LocalPythonModuleResolver:
    search_roots: tuple[Path, ...]
    _resolution_cache: dict[str, _ResolvedLocalModule | None] = field(
        default_factory=dict,
        compare=False,
        repr=False,
    )
    _cache_lock: RLock = field(default_factory=RLock, compare=False, repr=False)

    def __post_init__(self) -> None:
        resolved = tuple(root.resolve() for root in self.search_roots)
        if not resolved:
            raise ValueError("local Python module resolver requires a search root")
        object.__setattr__(self, "search_roots", resolved)

    def capture_source(self, path: Path) -> PythonSourceSnapshot:
        try:
            return PythonSourceSnapshot(path, path.read_bytes())
        except OSError as exc:
            raise ValueError(f"cannot read local Python source {path}: {exc}") from exc

    def module_identity(self, path: Path) -> tuple[str, str]:
        resolved = path.resolve()
        for root in self.search_roots:
            try:
                resolved.relative_to(root)
            except ValueError:
                continue
            module = relative_python_module_name(resolved, root)
            package = (
                module if resolved.name == "__init__.py" else module.rpartition(".")[0]
            )
            return module, package
        raise ValueError(f"Python source is outside local search roots: {resolved}")

    def source_for_module(self, module: str) -> Path | None:
        resolution = self._resolve_module(module)
        return (
            resolution.source.path
            if resolution is not None and resolution.source is not None
            else None
        )

    def _owned_path(self, path: Path) -> Path | None:
        """Resolve a candidate only when it remains inside a search root."""

        try:
            resolved = path.resolve()
        except OSError:
            return None
        if any(resolved.is_relative_to(root) for root in self.search_roots):
            return resolved
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
                    package_location = self._owned_path(location / part)
                    if package_location is not None and package_location.is_dir():
                        initializer = self._owned_path(package_location / "__init__.py")
                        if initializer is not None and initializer.is_file():
                            regular_source = initializer
                            regular_package_location = package_location
                            regular_is_package = True
                            break
                        namespace_locations.append(package_location)

                    source = self._owned_path(location / f"{part}.py")
                    if source is not None and source.is_file():
                        regular_source = source
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


def _project_import_request(
    request: StaticImportRequest,
    context: ModuleImportContext,
    path: Path,
) -> tuple[str, ...]:
    projection = project_static_import_request(request, context)
    if projection.error == "no_parent":
        if request.kind == "import_module":
            raise ValueError(f"relative import_module requires a package in {path}")
        raise ValueError(f"relative import has no known parent package in {path}")
    if projection.error == "beyond_top":
        raise ValueError(f"relative import escapes local package in {path}")
    if projection.error == "empty_name":
        raise ValueError(f"empty Python module name in {path}")
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


def _dynamic_import_target(
    call: ast.Call,
    *,
    kind: Literal["import_module", "dunder_import"],
    contexts: tuple[ModuleImportContext, ...],
    path: Path,
) -> tuple[str, ...] | None:
    is_import_module = kind == "import_module"
    if kind not in {"import_module", "dunder_import"}:
        return None
    try:
        arguments = bind_static_import_call_arguments(call, kind)
    except ValueError as exc:
        raise ValueError(f"{exc} in {path}") from exc
    name_arg = arguments.name
    if not isinstance(name_arg, ast.Constant) or not isinstance(name_arg.value, str):
        raise ValueError(f"non-literal dynamic Python import in {path}")
    name = name_arg.value
    level = 0
    level_arg = arguments.level if not is_import_module else None
    if level_arg is not None:
        if isinstance(level_arg, ast.Constant) and isinstance(level_arg.value, int):
            level = level_arg.value
        elif (
            isinstance(level_arg, ast.UnaryOp)
            and isinstance(level_arg.op, ast.USub)
            and isinstance(level_arg.operand, ast.Constant)
            and isinstance(level_arg.operand.value, int)
        ):
            level = -level_arg.operand.value
        else:
            raise ValueError(f"non-literal __import__ level in {path}")
        if level < 0:
            raise ValueError(f"negative __import__ level in {path}")
    fromlist: list[str] = []
    fromlist_arg = arguments.fromlist if not is_import_module else None
    if fromlist_arg is not None:
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
    for context in contexts:
        if is_import_module:
            request = StaticImportRequest.import_module(
                name,
                metadata_value_from_expression(package_arg, context),
            )
        else:
            request = StaticImportRequest(
                "dunder_import",
                name,
                level=level,
                fromlist=tuple(fromlist),
                globals_state=dunder_globals_state_from_expression(
                    globals_arg, context
                ),
                globals_were_supplied=globals_arg is not None,
            )
        try:
            modules.update(_project_import_request(request, context, path))
        except ValueError as exc:
            errors.append(exc)
    if errors:
        raise errors[0]
    if modules:
        return tuple(sorted(modules))
    return ()


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
    identity. Relative statements and every possible dynamic call continue to
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
                ),
            )
        return binding_index

    def contexts_for(node: ast.AST) -> tuple[ModuleImportContext, ...]:
        return tuple(
            base_context.with_state(state)
            for state in bindings().module_import_flow.states_for(node)
        )

    nodes: list[ast.AST] = (
        list(tree.body) if policy.module_level_only else list(ast.walk(tree))
    )
    requests: list[LocalPythonImportRequest] = []
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
            fromlist=tuple(alias.name for alias in node.names),
        )
        projection_errors: list[ValueError] = []
        contexts = contexts_for(node) if node.level else (base_context,)
        for context in contexts:
            try:
                candidates = _project_import_request(request, context, path)
                if candidates:
                    requests.append(
                        LocalPythonImportRequest(
                            "from", candidates, node.lineno, node.col_offset
                        )
                    )
            except ValueError as exc:
                projection_errors.append(exc)
        if projection_errors:
            raise projection_errors[0]

    diagnostics: list[LocalPythonImportDiagnostic] = []
    if not policy.module_level_only:
        for node in nodes:
            if not isinstance(node, ast.Call):
                continue
            fact = bindings().call_fact(node)
            kinds = fact.possible_import_call_kinds() if fact is not None else ()
            if not kinds:
                continue
            targets_for_call: set[str] = set()
            errors: list[ValueError] = []
            projection_succeeded = False
            for kind in kinds:
                try:
                    target = _dynamic_import_target(
                        node,
                        kind=kind,
                        contexts=contexts_for(node),
                        path=path,
                    )
                except ValueError as exc:
                    errors.append(exc)
                    continue
                projection_succeeded = True
                if target is not None:
                    targets_for_call.update(target)
            if not projection_succeeded:
                diagnostics.append(
                    LocalPythonImportDiagnostic(
                        node.lineno,
                        node.col_offset,
                        str(errors[0]).removesuffix(f" in {path}"),
                    )
                )
                continue
            requests.extend(
                LocalPythonImportRequest(
                    "dynamic", (target,), node.lineno, node.col_offset
                )
                for target in sorted(targets_for_call)
            )
        requests.extend(
            LocalPythonImportRequest("manifest", (target,))
            for target in nonliteral_dynamic_import_targets
        )

    analysis = LocalPythonImportAnalysis(
        tuple(dict.fromkeys(requests)), tuple(diagnostics)
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
    for request in analysis.requests:
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
    """Expose canonical candidate names without discarding graph grouping."""

    analysis = analyze_local_imports(
        resolver.capture_source(path),
        LocalPythonModuleSource(resolver.module_identity(path)[0], path),
        policy,
        expected_nonliteral_dynamic_imports=expected_nonliteral_dynamic_imports,
        nonliteral_dynamic_import_targets=nonliteral_dynamic_import_targets,
    )
    return {
        candidate for request in analysis.requests for candidate in request.candidates
    }

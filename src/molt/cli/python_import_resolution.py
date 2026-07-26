"""Shared local-Python import resolution for compiler cache authorities."""

from __future__ import annotations

import ast
import tokenize
from dataclasses import dataclass, field
from pathlib import Path
from threading import RLock
from typing import Literal

from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
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


@dataclass(frozen=True)
class _ResolvedLocalModule:
    source: Path | None
    package_locations: tuple[Path, ...]
    parent_initializers: tuple[Path, ...]


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

    def read_ast(self, path: Path) -> ast.Module:
        try:
            with tokenize.open(path) as stream:
                source = stream.read()
            return ast.parse(source, filename=str(path))
        except (OSError, SyntaxError, UnicodeError, ValueError) as exc:
            raise ValueError(f"cannot parse local Python source {path}: {exc}") from exc

    def module_identity(self, path: Path) -> tuple[str, str]:
        resolved = path.resolve()
        for root in self.search_roots:
            try:
                relative = resolved.relative_to(root)
            except ValueError:
                continue
            parts = list(relative.parts)
            is_package = parts[-1] == "__init__.py"
            if is_package:
                parts.pop()
            elif parts[-1].endswith(".py"):
                parts[-1] = parts[-1][: -len(".py")]
            else:
                raise ValueError(f"local Python source is not a .py file: {resolved}")
            module = ".".join(parts)
            package = module if is_package else module.rpartition(".")[0]
            return module, package
        raise ValueError(f"Python source is outside local search roots: {resolved}")

    def source_for_module(self, module: str) -> Path | None:
        resolution = self._resolve_module(module)
        return resolution.source if resolution is not None else None

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
            parents: list[Path] = []
            for index, part in enumerate(parts):
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
                    if final_segment:
                        result = _ResolvedLocalModule(
                            source=regular_source,
                            package_locations=(
                                (regular_package_location,)
                                if regular_package_location is not None
                                else ()
                            ),
                            parent_initializers=tuple(parents),
                        )
                        self._resolution_cache[module] = result
                        return result
                    if not regular_is_package or regular_package_location is None:
                        self._resolution_cache[module] = None
                        return None
                    parents.append(regular_source)
                    locations = (regular_package_location,)
                    continue

                if not namespace_locations:
                    self._resolution_cache[module] = None
                    return None
                locations = tuple(namespace_locations)
                if final_segment:
                    result = _ResolvedLocalModule(
                        source=None,
                        package_locations=locations,
                        parent_initializers=tuple(parents),
                    )
                    self._resolution_cache[module] = result
                    return result

            raise AssertionError("non-empty module resolution exhausted no segment")

    def resolve_with_owner_fallback(self, module: str) -> tuple[str, Path] | None:
        source = self.source_for_module(module)
        if source is not None:
            return module, source
        owner, separator, _name = module.rpartition(".")
        if not separator:
            return None
        source = self.source_for_module(owner)
        return (owner, source) if source is not None else None

    def parent_package_sources(self, module: str) -> tuple[Path, ...]:
        """Existing parent ``__init__`` files in CPython execution order.

        A missing initializer is a PEP 420 namespace package, not an error.
        The requested module/package itself is excluded; its source is returned
        separately by ``source_for_module``.
        """

        resolution = self._resolve_module(module)
        return resolution.parent_initializers if resolution is not None else ()


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


def local_import_targets(
    path: Path,
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
    *,
    expected_nonliteral_dynamic_imports: int | None = None,
    nonliteral_dynamic_import_targets: tuple[str, ...] = (),
) -> set[str]:
    """Analyze one source into import targets according to ``policy``."""

    tree = resolver.read_ast(path)
    module, _package = resolver.module_identity(path)
    base_context = ModuleImportContext(
        module_name=module,
        is_package=path.name == "__init__.py",
        spec_name=module,
    )
    binding_index = analyze_python_bindings(
        tree,
        source_digest=python_ast_digest(tree),
        policy=PythonBindingPolicy(
            module_name=module,
            module_spec_name=module,
            module_is_package=path.name == "__init__.py",
            module_execution_kind="imported",
            analyze_deferred_bodies=not policy.module_level_only,
        ),
    )
    import_flow = binding_index.module_import_flow

    def contexts_for(node: ast.AST) -> tuple[ModuleImportContext, ...]:
        return tuple(
            base_context.with_state(state) for state in import_flow.states_for(node)
        )
    nodes: list[ast.AST] = (
        list(tree.body) if policy.module_level_only else list(ast.walk(tree))
    )
    targets: set[str] = set()
    for node in nodes:
        if isinstance(node, ast.Import):
            targets.update(alias.name for alias in node.names)
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
        projected: set[str] = set()
        projection_errors: list[ValueError] = []
        for context in contexts_for(node):
            try:
                projected.update(_project_import_request(request, context, path))
            except ValueError as exc:
                projection_errors.append(exc)
        if projection_errors:
            raise projection_errors[0]
        targets.update(projected)

    nonliteral_dynamic_imports = 0
    if not policy.module_level_only:
        for node in nodes:
            if not isinstance(node, ast.Call):
                continue
            fact = binding_index.call_fact(node)
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
                nonliteral_dynamic_imports += 1
                if (
                    policy.fail_on_nonliteral_dynamic_import
                    and expected_nonliteral_dynamic_imports is None
                ):
                    raise errors[0]
                continue
            targets.update(targets_for_call)
        if (
            expected_nonliteral_dynamic_imports is not None
            and nonliteral_dynamic_imports != expected_nonliteral_dynamic_imports
        ):
            raise ValueError(
                f"dynamic Python import manifest drift in {path}: expected "
                f"{expected_nonliteral_dynamic_imports} non-literal calls, found "
                f"{nonliteral_dynamic_imports}"
            )
        targets.update(nonliteral_dynamic_import_targets)

    return targets


def resolve_local_import_targets(
    targets: set[str],
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
) -> set[Path]:
    """Resolve analyzed targets against the live local module topology."""

    dependencies: set[Path] = set()
    for target in targets:
        if policy.allowed_prefix is not None and not (
            target == policy.allowed_prefix
            or target.startswith(f"{policy.allowed_prefix}.")
        ):
            continue
        resolved = resolver.resolve_with_owner_fallback(target)
        if resolved is None:
            continue
        resolved_module, source = resolved
        if policy.include_parent_packages:
            dependencies.update(resolver.parent_package_sources(resolved_module))
        dependencies.add(source)
    return dependencies


def local_import_dependencies(
    path: Path,
    resolver: LocalPythonModuleResolver,
    policy: PythonImportPolicy,
    *,
    expected_nonliteral_dynamic_imports: int | None = None,
    nonliteral_dynamic_import_targets: tuple[str, ...] = (),
) -> set[Path]:
    """Analyze and resolve one source's local import edges."""

    targets = local_import_targets(
        path,
        resolver,
        policy,
        expected_nonliteral_dynamic_imports=expected_nonliteral_dynamic_imports,
        nonliteral_dynamic_import_targets=nonliteral_dynamic_import_targets,
    )
    return resolve_local_import_targets(targets, resolver, policy)
